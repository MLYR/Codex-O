// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { settingsApi } from "../settings/api";
import { skillCatalogApi } from "./api";
import { SkillsPage } from "./SkillsPage";
import type { CatalogScan, SkillSummary } from "./types";

vi.mock("./api", () => ({
  skillCatalogApi: {
    listProviders: vi.fn(),
    loadCatalog: vi.fn(),
    scanSkills: vi.fn(),
    listSkills: vi.fn(),
    getSkillDetail: vi.fn(),
  },
}));

vi.mock("../settings/api", () => ({
  settingsApi: {
    getScanPreferences: vi.fn(),
    acknowledgeInitialScanNotice: vi.fn(),
  },
}));

const provider = {
  id: "user_global",
  kind: "user_global",
  display_name: "User Global",
  capabilities: {
    can_read: true,
    can_import: false,
    can_quarantine: false,
    can_restore: false,
    can_update: false,
    can_delete: false,
  },
  availability: "available" as const,
};

const summary: SkillSummary = {
  id: "skill:11:user_globalreview",
  display_name: "Review",
  description: "Review code safely",
  provider,
  scope: "user",
  validity: "valid",
  analysis_status: "not_configured",
  size_bytes: 128,
  updated_at_ms: 1_700_000_000_000,
  diagnostics: [],
};

const catalog: CatalogScan = { providers: [provider], skills: [summary], diagnostics: [] };

function renderPage() {
  return render(
    <MemoryRouter>
      <SkillsPage />
    </MemoryRouter>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(skillCatalogApi.loadCatalog).mockResolvedValue(catalog);
  vi.mocked(skillCatalogApi.scanSkills).mockResolvedValue(catalog);
  vi.mocked(settingsApi.getScanPreferences).mockResolvedValue({
    include_plugin_cache: false,
    include_bundled_cache: false,
    initial_scan_notice_seen: true,
  });
  vi.mocked(settingsApi.acknowledgeInitialScanNotice).mockResolvedValue({
    include_plugin_cache: false,
    include_bundled_cache: false,
    initial_scan_notice_seen: true,
  });
});

afterEach(() => {
  cleanup();
});

describe("SkillsPage", () => {
  it("renders a persisted catalog without scanning", async () => {
    renderPage();

    await screen.findByRole("heading", { name: "Review" });

    expect(skillCatalogApi.loadCatalog).toHaveBeenCalledTimes(1);
    expect(skillCatalogApi.scanSkills).not.toHaveBeenCalled();
  });

  it("renders catalog skills and a stable encoded detail link", async () => {
    renderPage();

    await screen.findByRole("heading", { name: "Review" });
    const link = screen.getByRole("link", { name: /Review/ });

    expect(link.getAttribute("href")).toBe(`/skills/${encodeURIComponent(summary.id)}`);
    expect(screen.getByText("1 个 Skill")).not.toBeNull();
  });

  it("filters the cached catalog without sending a request per search input", async () => {
    renderPage();
    await screen.findByRole("heading", { name: "Review" });

    fireEvent.change(screen.getByRole("textbox", { name: "搜索 Skill" }), {
      target: { value: "audit" },
    });

    await waitFor(() => {
      expect(screen.getByRole("heading", { name: "没有匹配的 Skill" })).not.toBeNull();
    });
    expect(skillCatalogApi.scanSkills).not.toHaveBeenCalled();
    expect(skillCatalogApi.listSkills).not.toHaveBeenCalled();
  });

  it("filters provider scope and validity from the cached catalog", async () => {
    renderPage();
    await screen.findByRole("heading", { name: "Review" });

    fireEvent.change(screen.getByRole("combobox", { name: "来源筛选" }), {
      target: { value: "user_global" },
    });
    fireEvent.change(screen.getByRole("combobox", { name: "作用域筛选" }), {
      target: { value: "user" },
    });
    fireEvent.change(screen.getByRole("combobox", { name: "状态筛选" }), {
      target: { value: "valid" },
    });

    await waitFor(() => {
      expect(screen.getByRole("heading", { name: "Review" })).not.toBeNull();
    });
    expect(skillCatalogApi.scanSkills).not.toHaveBeenCalled();
    expect(skillCatalogApi.listSkills).not.toHaveBeenCalled();
  });

  it("sorts the cached catalog without rescanning", async () => {
    renderPage();
    await screen.findByRole("heading", { name: "Review" });

    fireEvent.change(screen.getByRole("combobox", { name: "排序" }), {
      target: { value: "size" },
    });

    await waitFor(() => {
      expect(screen.getByRole("heading", { name: "Review" })).not.toBeNull();
    });
    expect(skillCatalogApi.scanSkills).not.toHaveBeenCalled();
    expect(skillCatalogApi.listSkills).not.toHaveBeenCalled();
  });

  it("shows the no-filter-result state from the cached catalog", async () => {
    renderPage();
    await screen.findByRole("heading", { name: "Review" });

    fireEvent.change(screen.getByRole("textbox", { name: "搜索 Skill" }), {
      target: { value: "missing" },
    });

    expect(await screen.findByRole("heading", { name: "没有匹配的 Skill" })).not.toBeNull();
  });

  it("rescans only when the user explicitly requests it", async () => {
    renderPage();
    await screen.findByRole("heading", { name: "Review" });

    fireEvent.click(screen.getByRole("button", { name: "重新扫描" }));

    await waitFor(() => {
      expect(skillCatalogApi.scanSkills).toHaveBeenCalledTimes(1);
    });
    expect(skillCatalogApi.listSkills).not.toHaveBeenCalled();
  });

  it("does not scan automatically when no index exists", async () => {
    vi.mocked(skillCatalogApi.loadCatalog).mockResolvedValue(null);
    renderPage();

    expect(await screen.findByRole("heading", { name: "尚未扫描 Skills" })).not.toBeNull();
    expect(skillCatalogApi.scanSkills).not.toHaveBeenCalled();
  });

  it("shows the first scan notice and allows postponing it", async () => {
    vi.mocked(skillCatalogApi.loadCatalog).mockResolvedValue(null);
    vi.mocked(settingsApi.getScanPreferences).mockResolvedValue({
      include_plugin_cache: false,
      include_bundled_cache: false,
      initial_scan_notice_seen: false,
    });
    renderPage();

    await screen.findByRole("dialog", { name: "扫描本地 Skills" });
    fireEvent.click(screen.getByRole("button", { name: "暂不扫描" }));

    await waitFor(() => {
      expect(settingsApi.acknowledgeInitialScanNotice).toHaveBeenCalledTimes(1);
    });
    expect(screen.queryByRole("dialog", { name: "扫描本地 Skills" })).toBeNull();
    expect(skillCatalogApi.scanSkills).not.toHaveBeenCalled();
  });

  it("starts the first scan only after explicit confirmation", async () => {
    vi.mocked(skillCatalogApi.loadCatalog).mockResolvedValue(null);
    vi.mocked(settingsApi.getScanPreferences).mockResolvedValue({
      include_plugin_cache: false,
      include_bundled_cache: false,
      initial_scan_notice_seen: false,
    });
    renderPage();

    const dialog = await screen.findByRole("dialog", { name: "扫描本地 Skills" });
    fireEvent.click(within(dialog).getByRole("button", { name: "开始扫描" }));

    await screen.findByRole("heading", { name: "Review" });
    expect(settingsApi.acknowledgeInitialScanNotice).toHaveBeenCalledTimes(1);
    expect(skillCatalogApi.scanSkills).toHaveBeenCalledTimes(1);
  });

  it("keeps the page responsive while an explicit scan is pending", async () => {
    vi.mocked(skillCatalogApi.loadCatalog).mockResolvedValue(null);
    vi.mocked(settingsApi.getScanPreferences).mockResolvedValue({
      include_plugin_cache: false,
      include_bundled_cache: false,
      initial_scan_notice_seen: true,
    });
    let resolveScan: (value: CatalogScan) => void;
    vi.mocked(skillCatalogApi.scanSkills).mockReturnValueOnce(
      new Promise<CatalogScan>((resolve) => {
        resolveScan = resolve;
      }),
    );
    renderPage();

    await screen.findByRole("heading", { name: "尚未扫描 Skills" });
    fireEvent.click(screen.getByRole("button", { name: "扫描 Skills" }));

    expect(await screen.findByRole("heading", { name: "正在后台扫描 Skills" })).not.toBeNull();
    expect((screen.getByRole("button", { name: "扫描 Skills" }) as HTMLButtonElement).disabled).toBe(true);

    resolveScan!(catalog);
    expect(await screen.findByRole("heading", { name: "Review" })).not.toBeNull();
  });
});
