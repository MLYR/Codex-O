// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { installApi, type PlannedImport } from "./api";
import { InstallPage } from "./InstallPage";

vi.mock("./api", async () => {
  const actual = await vi.importActual<typeof import("./api")>("./api");
  return {
    ...actual,
    installApi: {
      selectImportSource: vi.fn(),
      planSkillImport: vi.fn(),
      planGithubImport: vi.fn(),
      executeSkillImport: vi.fn(),
      cancelSkillImport: vi.fn(),
    },
  };
});

const readyPlan: PlannedImport = {
  plan: {
    id: "operation-safe-id",
    operation: "skill_import",
    status: "ready",
    impact: {
      target_provider_id: "user_global",
      skill_name: "fixture-skill",
      file_count: 3,
      total_size_bytes: 2048,
    },
  },
  confirmation_token: {
    token: "confirmation-secret",
    expires_at_ms: 1_800_000_000_000,
  },
};

const githubPlan: PlannedImport = {
  ...readyPlan,
  plan: {
    ...readyPlan.plan,
    source: {
      source_type: "github",
      repository_url: "https://github.com/openai/codex",
      repo_ref: "main",
      commit_sha: "0123456789abcdef0123456789abcdef01234567",
      subdirectory: "skills/fixture-skill",
    },
  },
};

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(installApi.selectImportSource).mockResolvedValue({
    token: "selection-secret",
    expires_at_ms: 1_800_000_000_000,
  });
  vi.mocked(installApi.planSkillImport).mockResolvedValue(readyPlan);
  vi.mocked(installApi.planGithubImport).mockResolvedValue(githubPlan);
  vi.mocked(installApi.cancelSkillImport).mockResolvedValue(undefined);
  vi.mocked(installApi.executeSkillImport).mockResolvedValue({
    operation_id: "operation-safe-id",
    status: "succeeded",
    skill_id: "opaque-skill-id",
    installed_hash: "safe-hash",
  });
});

afterEach(() => cleanup());

describe("InstallPage", () => {
  it("offers real file and directory source selectors", () => {
    render(<InstallPage />);

    expect(screen.getByRole("button", { name: /选择 SKILL.md/ })).not.toBeNull();
    expect(screen.getByRole("button", { name: /选择 Skill 目录/ })).not.toBeNull();
  });

  it("selects a file and opens a safe operation plan", async () => {
    render(<InstallPage />);

    fireEvent.click(screen.getByRole("button", { name: /选择 SKILL.md/ }));

    expect(await screen.findByRole("dialog", { name: "导入计划" })).not.toBeNull();
    expect(installApi.selectImportSource).toHaveBeenCalledWith("file");
    expect(installApi.planSkillImport).toHaveBeenCalledWith("selection-secret");
    expect(screen.queryByText("selection-secret")).toBeNull();
    expect(screen.queryByText("confirmation-secret")).toBeNull();
  });

  it("uses the directory selector for a directory import", async () => {
    render(<InstallPage />);

    fireEvent.click(screen.getByRole("button", { name: /选择 Skill 目录/ }));

    await screen.findByRole("dialog", { name: "导入计划" });
    expect(installApi.selectImportSource).toHaveBeenCalledWith("directory");
  });

  it("executes only after explicit confirmation and shows success", async () => {
    render(<InstallPage />);
    fireEvent.click(screen.getByRole("button", { name: /选择 SKILL.md/ }));
    fireEvent.click(await screen.findByRole("button", { name: "确认导入" }));

    await waitFor(() => {
      expect(installApi.executeSkillImport).toHaveBeenCalledWith("confirmation-secret");
    });
    expect(await screen.findByRole("heading", { name: "Skill 已安装" })).not.toBeNull();
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("renders conflict plans without an executable confirmation", async () => {
    vi.mocked(installApi.planSkillImport).mockResolvedValue({
      plan: { ...readyPlan.plan, status: "conflict" },
    });
    render(<InstallPage />);

    fireEvent.click(screen.getByRole("button", { name: /选择 SKILL.md/ }));

    expect(await screen.findByText("conflict_detected")).not.toBeNull();
    expect(screen.queryByRole("button", { name: "确认导入" })).toBeNull();
    expect(installApi.executeSkillImport).not.toHaveBeenCalled();
  });

  it("shows only stable operation errors and recovery guidance", async () => {
    vi.mocked(installApi.selectImportSource).mockRejectedValue({
      code: "selection_unavailable",
      message: "无法完成来源选择。",
      recovery: "请重新选择本地 Skill。",
      raw: "/private/source/path",
    });
    render(<InstallPage />);

    fireEvent.click(screen.getByRole("button", { name: /选择 SKILL.md/ }));

    expect(await screen.findByText("selection_unavailable")).not.toBeNull();
    expect(screen.getByText("请重新选择本地 Skill。")).not.toBeNull();
    expect(screen.queryByText("/private/source/path")).toBeNull();
  });

  it("closes an idle plan with Escape without executing", async () => {
    render(<InstallPage />);
    fireEvent.click(screen.getByRole("button", { name: /选择 SKILL.md/ }));
    await screen.findByRole("dialog", { name: "导入计划" });

    fireEvent.keyDown(window, { key: "Escape" });

    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(installApi.executeSkillImport).not.toHaveBeenCalled();
    expect(installApi.cancelSkillImport).toHaveBeenCalledWith("confirmation-secret");
  });

  it("cancels the backend plan before switching installation modes", async () => {
    render(<InstallPage />);
    fireEvent.click(screen.getByRole("button", { name: /选择 SKILL.md/ }));
    await screen.findByRole("dialog", { name: "导入计划" });

    fireEvent.click(screen.getByRole("button", { name: "GitHub" }));

    await waitFor(() => {
      expect(installApi.cancelSkillImport).toHaveBeenCalledWith("confirmation-secret");
    });
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(screen.getByLabelText("仓库 URL")).not.toBeNull();
  });

  it("cancels the backend plan when leaving the install route", async () => {
    const view = render(<InstallPage />);
    fireEvent.click(screen.getByRole("button", { name: /选择 SKILL.md/ }));
    await screen.findByRole("dialog", { name: "导入计划" });

    view.unmount();

    await waitFor(() => {
      expect(installApi.cancelSkillImport).toHaveBeenCalledWith("confirmation-secret");
    });
  });

  it("switches to the GitHub source form", () => {
    render(<InstallPage />);

    fireEvent.click(screen.getByRole("button", { name: "GitHub" }));

    expect(screen.getByLabelText("仓库 URL")).not.toBeNull();
    expect((screen.getByLabelText("Ref") as HTMLInputElement).value).toBe("main");
    expect(screen.getByLabelText("子目录")).not.toBeNull();
  });

  it("plans a GitHub import with separate normalized fields", async () => {
    render(<InstallPage />);
    fireEvent.click(screen.getByRole("button", { name: "GitHub" }));
    fireEvent.change(screen.getByLabelText("仓库 URL"), {
      target: { value: " https://github.com/OpenAI/Codex.git " },
    });
    fireEvent.change(screen.getByLabelText("Ref"), {
      target: { value: " feature/install " },
    });
    fireEvent.change(screen.getByLabelText("子目录"), {
      target: { value: " skills/fixture-skill " },
    });

    fireEvent.click(screen.getByRole("button", { name: "检查安装计划" }));

    expect(await screen.findByRole("dialog", { name: "导入计划" })).not.toBeNull();
    expect(installApi.planGithubImport).toHaveBeenCalledWith(
      "https://github.com/OpenAI/Codex.git",
      "feature/install",
      "skills/fixture-skill",
    );
  });

  it("shows fixed GitHub provenance without exposing confirmation tokens", async () => {
    render(<InstallPage />);
    fireEvent.click(screen.getByRole("button", { name: "GitHub" }));
    fireEvent.change(screen.getByLabelText("仓库 URL"), {
      target: { value: "https://github.com/openai/codex" },
    });
    fireEvent.click(screen.getByRole("button", { name: "检查安装计划" }));

    expect(await screen.findByText("0123456789abcdef0123456789abcdef01234567")).not.toBeNull();
    expect(screen.getByText("skills/fixture-skill")).not.toBeNull();
    expect(screen.queryByText("confirmation-secret")).toBeNull();
  });

  it("renders GitHub rate-limit recovery guidance", async () => {
    vi.mocked(installApi.planGithubImport).mockRejectedValue({
      code: "github_rate_limited",
      message: "GitHub 暂时限制了请求。",
      recovery: "请稍后重新检查安装计划。",
    });
    render(<InstallPage />);
    fireEvent.click(screen.getByRole("button", { name: "GitHub" }));
    fireEvent.change(screen.getByLabelText("仓库 URL"), {
      target: { value: "https://github.com/openai/codex" },
    });

    fireEvent.click(screen.getByRole("button", { name: "检查安装计划" }));

    expect(await screen.findByText("github_rate_limited")).not.toBeNull();
    expect(screen.getByText("请稍后重新检查安装计划。")).not.toBeNull();
  });

  it("shows GitHub planning progress while the request is pending", async () => {
    let resolvePlan: ((plan: PlannedImport) => void) | undefined;
    vi.mocked(installApi.planGithubImport).mockImplementation(
      () => new Promise((resolve) => (resolvePlan = resolve)),
    );
    render(<InstallPage />);
    fireEvent.click(screen.getByRole("button", { name: "GitHub" }));
    fireEvent.change(screen.getByLabelText("仓库 URL"), {
      target: { value: "https://github.com/openai/codex" },
    });

    fireEvent.click(screen.getByRole("button", { name: "检查安装计划" }));

    expect(await screen.findByText("正在固定版本并检查仓库内容")).not.toBeNull();
    resolvePlan?.(githubPlan);
    expect(await screen.findByRole("dialog", { name: "导入计划" })).not.toBeNull();
  });
});
