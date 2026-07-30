// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { installApi, type PlannedImport } from "../install/api";
import { marketApi, type MarketCatalog } from "./api";
import { MarketPage } from "./MarketPage";

vi.mock("./api", () => ({
  marketApi: {
    getCatalog: vi.fn(),
    refreshCatalog: vi.fn(),
    planImport: vi.fn(),
  },
}));

vi.mock("../install/api", async () => {
  const actual = await vi.importActual<typeof import("../install/api")>("../install/api");
  return {
    ...actual,
    installApi: {
      executeSkillImport: vi.fn(),
      cancelSkillImport: vi.fn(),
    },
  };
});

const catalog: MarketCatalog = {
  status: "ready",
  provider_name: "openai-curated",
  commit_sha: "0123456789abcdef0123456789abcdef01234567",
  synced_at_ms: 1_800_000_000_000,
  items: [
    {
      id: "market:first",
      plugin_name: "documents-plugin",
      skill_name: "documents",
      category: "Productivity",
      description: "Create and review documents.",
      installed: false,
    },
    {
      id: "market:second",
      plugin_name: "developer-plugin",
      skill_name: "api-testing",
      category: "Developer Tools",
      description: "Test HTTP APIs.",
      installed: true,
    },
  ],
};

const plan: PlannedImport = {
  plan: {
    id: "operation-safe-id",
    operation: "skill_import",
    status: "ready",
    impact: {
      target_provider_id: "user_global",
      skill_name: "documents",
      file_count: 2,
      total_size_bytes: 2048,
    },
    source: {
      source_type: "market",
      repository_url: "https://github.com/openai/plugins",
      repo_ref: "main",
      commit_sha: "0123456789abcdef0123456789abcdef01234567",
      subdirectory: "plugins/documents-plugin/skills/documents",
    },
  },
  confirmation_token: {
    token: "confirmation-secret",
    expires_at_ms: 1_800_000_000_000,
  },
};

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(marketApi.getCatalog).mockResolvedValue(catalog);
  vi.mocked(marketApi.refreshCatalog).mockResolvedValue(catalog);
  vi.mocked(marketApi.planImport).mockResolvedValue(plan);
  vi.mocked(installApi.cancelSkillImport).mockResolvedValue(undefined);
  vi.mocked(installApi.executeSkillImport).mockResolvedValue({
    operation_id: "operation-safe-id",
    status: "succeeded",
    skill_id: "opaque-skill-id",
    installed_hash: "safe-hash",
  });
});

afterEach(() => cleanup());

function renderPage() {
  return render(
    <MemoryRouter>
      <MarketPage />
    </MemoryRouter>,
  );
}

describe("MarketPage", () => {
  it("renders the backend count, provider, and fixed commit", async () => {
    renderPage();

    expect(await screen.findByRole("heading", { name: "documents" })).not.toBeNull();
    expect(screen.getByText("openai-curated")).not.toBeNull();
    expect(screen.getByText("2 个 Skill")).not.toBeNull();
    expect(screen.getByText("commit 01234567")).not.toBeNull();
    expect(marketApi.refreshCatalog).toHaveBeenCalledTimes(1);
  });

  it("filters by search and category without changing backend facts", async () => {
    renderPage();
    await screen.findByRole("heading", { name: "documents" });

    fireEvent.change(screen.getByLabelText("搜索市场 Skill"), {
      target: { value: "api" },
    });
    expect(screen.queryByRole("heading", { name: "documents" })).toBeNull();
    expect(screen.getByRole("heading", { name: "api-testing" })).not.toBeNull();

    fireEvent.change(screen.getByLabelText("市场分类"), {
      target: { value: "Productivity" },
    });
    expect(screen.getByText("没有匹配的 Skill")).not.toBeNull();
  });

  it("disables install for an exact installed market item", async () => {
    renderPage();
    await screen.findByRole("heading", { name: "api-testing" });

    expect((screen.getByRole("button", { name: "已安装" }) as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByRole("button", { name: "安装" }) as HTMLButtonElement).disabled).toBe(false);
  });

  it("shows stale cache guidance while keeping items usable", async () => {
    const stale = {
      ...catalog,
      status: "stale" as const,
      issue: {
        code: "market_offline",
        message: "offline",
        recovery: "检查网络后重新同步。",
      },
    };
    vi.mocked(marketApi.getCatalog).mockResolvedValue(stale);
    vi.mocked(marketApi.refreshCatalog).mockResolvedValue(stale);
    renderPage();

    expect(await screen.findByText("正在使用上次同步的市场快照")).not.toBeNull();
    expect(screen.getByRole("heading", { name: "documents" })).not.toBeNull();
  });

  it("renders a genuine empty state", async () => {
    const empty = { ...catalog, items: [] };
    vi.mocked(marketApi.getCatalog).mockResolvedValue(empty);
    vi.mocked(marketApi.refreshCatalog).mockResolvedValue(empty);
    renderPage();

    expect(await screen.findByText("市场暂时没有可用 Skill")).not.toBeNull();
    expect(screen.getByText("0 个 Skill")).not.toBeNull();
  });

  it("degrades to local and GitHub install when no snapshot exists", async () => {
    const unavailable: MarketCatalog = {
      status: "unavailable",
      items: [],
      issue: {
        code: "market_cache_missing",
        message: "missing",
        recovery: "连接网络后重试。",
      },
    };
    vi.mocked(marketApi.getCatalog).mockResolvedValue(unavailable);
    vi.mocked(marketApi.refreshCatalog).mockResolvedValue(unavailable);
    renderPage();

    expect(await screen.findByText("官方市场暂时不可用")).not.toBeNull();
    expect(
      (screen.getByRole("link", { name: /使用本地或 GitHub 安装/ }) as HTMLAnchorElement)
        .getAttribute("href"),
    ).toBe("/install");
  });

  it("plans and executes a market install only after confirmation", async () => {
    renderPage();
    await screen.findByRole("heading", { name: "documents" });
    fireEvent.click(screen.getByRole("button", { name: "安装" }));

    expect(await screen.findByRole("dialog", { name: "导入计划" })).not.toBeNull();
    expect(marketApi.planImport).toHaveBeenCalledWith("market:first");
    expect(screen.queryByText("confirmation-secret")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "确认导入" }));

    await waitFor(() => {
      expect(installApi.executeSkillImport).toHaveBeenCalledWith("confirmation-secret");
    });
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("cancels staging when the plan is closed", async () => {
    renderPage();
    await screen.findByRole("heading", { name: "documents" });
    fireEvent.click(screen.getByRole("button", { name: "安装" }));
    await screen.findByRole("dialog", { name: "导入计划" });
    fireEvent.click(screen.getByRole("button", { name: "关闭导入计划" }));

    await waitFor(() => {
      expect(installApi.cancelSkillImport).toHaveBeenCalledWith("confirmation-secret");
    });
  });

  it("cancels staging when navigation unmounts the page", async () => {
    const view = renderPage();
    await screen.findByRole("heading", { name: "documents" });
    fireEvent.click(screen.getByRole("button", { name: "安装" }));
    await screen.findByRole("dialog", { name: "导入计划" });
    view.unmount();

    await waitFor(() => {
      expect(installApi.cancelSkillImport).toHaveBeenCalledWith("confirmation-secret");
    });
  });
});
