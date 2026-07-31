// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { installApi, type PlannedImport } from "../install/api";
import { updatesApi, type SkillUpdateSummary } from "./api";
import { UpdatesPage } from "./UpdatesPage";

vi.mock("../install/api", async () => {
  const actual = await vi.importActual<typeof import("../install/api")>("../install/api");
  return {
    ...actual,
    installApi: { ...actual.installApi, cancelSkillImport: vi.fn() },
  };
});

vi.mock("./api", () => ({
  updatesApi: { check: vi.fn(), plan: vi.fn(), execute: vi.fn() },
}));

const available: SkillUpdateSummary = {
  skill_id: "skill:one",
  display_name: "reviewer",
  source_type: "github",
  status: "available",
  installed_commit: "11111111",
  available_commit: "22222222",
  checked_at_ms: 1,
  reason: "发现可安全预览的新版本。",
  changed_files: ["SKILL.md", "references/guide.md"],
};

const plan: PlannedImport = {
  plan: {
    id: "operation-id",
    operation: "skill_update",
    status: "ready",
    impact: {
      target_provider_id: "user_global",
      skill_name: "reviewer",
      file_count: 2,
      total_size_bytes: 100,
      relative_files: ["SKILL.md"],
    },
  },
  confirmation_token: { token: "update-secret", expires_at_ms: 2 },
};

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(updatesApi.check).mockResolvedValue([available]);
  vi.mocked(updatesApi.plan).mockResolvedValue(plan);
  vi.mocked(updatesApi.execute).mockResolvedValue({
    operation_id: "operation-id",
    status: "succeeded",
    skill_id: "skill:one",
    installed_hash: "hidden",
  });
  vi.mocked(installApi.cancelSkillImport).mockResolvedValue(undefined);
});

afterEach(cleanup);

describe("UpdatesPage", () => {
  it("does not check remotely until the user asks", () => {
    render(<UpdatesPage />);
    expect(screen.getByText("尚未检查更新")).not.toBeNull();
    expect(updatesApi.check).not.toHaveBeenCalled();
  });

  it("renders current, conflict, unavailable, and available states", async () => {
    vi.mocked(updatesApi.check).mockResolvedValue([
      available,
      { ...available, skill_id: "current", display_name: "current", status: "current", reason: "已是来源中的最新内容。", changed_files: [] },
      { ...available, skill_id: "conflict", display_name: "conflict", status: "conflict", reason: "检测到本地修改，已保留且不会覆盖。", changed_files: [] },
      { ...available, skill_id: "offline", display_name: "offline", status: "unavailable", reason: "当前无法连接更新来源。", changed_files: [] },
    ]);
    render(<UpdatesPage />);
    fireEvent.click(screen.getByRole("button", { name: "检查更新" }));

    expect(await screen.findByText("reviewer")).not.toBeNull();
    expect(screen.getByText("已是最新")).not.toBeNull();
    expect(screen.getByText("本地修改")).not.toBeNull();
    expect(screen.getByText("暂不可用")).not.toBeNull();
    expect(screen.getAllByRole("button", { name: "查看计划" })).toHaveLength(1);
  });

  it("renders an empty receipt state", async () => {
    vi.mocked(updatesApi.check).mockResolvedValue([]);
    render(<UpdatesPage />);
    fireEvent.click(screen.getByRole("button", { name: "检查更新" }));
    expect(await screen.findByText("没有可检查的安装记录")).not.toBeNull();
  });

  it("plans and executes an available update before refreshing", async () => {
    render(<UpdatesPage />);
    fireEvent.click(screen.getByRole("button", { name: "检查更新" }));
    await screen.findByText("reviewer");
    fireEvent.click(screen.getByRole("button", { name: "查看计划" }));
    expect(await screen.findByRole("dialog", { name: "更新计划" })).not.toBeNull();
    expect(screen.getByText("references/guide.md")).not.toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "确认更新" }));

    await waitFor(() => expect(updatesApi.execute).toHaveBeenCalledWith("update-secret"));
    expect(updatesApi.check).toHaveBeenCalledTimes(2);
    expect(await screen.findByText("reviewer 已更新")).not.toBeNull();
  });

  it("cancels backend staging when the update plan closes", async () => {
    render(<UpdatesPage />);
    fireEvent.click(screen.getByRole("button", { name: "检查更新" }));
    await screen.findByText("reviewer");
    fireEvent.click(screen.getByRole("button", { name: "查看计划" }));
    await screen.findByRole("dialog", { name: "更新计划" });
    fireEvent.click(screen.getByRole("button", { name: "关闭更新计划" }));

    await waitFor(() => expect(installApi.cancelSkillImport).toHaveBeenCalledWith("update-secret"));
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("cancels backend staging when navigation unmounts the page", async () => {
    const view = render(<UpdatesPage />);
    fireEvent.click(screen.getByRole("button", { name: "检查更新" }));
    await screen.findByText("reviewer");
    fireEvent.click(screen.getByRole("button", { name: "查看计划" }));
    await screen.findByRole("dialog", { name: "更新计划" });
    view.unmount();

    await waitFor(() => expect(installApi.cancelSkillImport).toHaveBeenCalledWith("update-secret"));
  });

  it("shows a safe recovery error", async () => {
    vi.mocked(updatesApi.check).mockRejectedValue({
      code: "database_unavailable",
      message: "unavailable",
      recovery: "恢复应用本地存储后重试。",
    });
    render(<UpdatesPage />);
    fireEvent.click(screen.getByRole("button", { name: "检查更新" }));
    expect(await screen.findByText("database_unavailable")).not.toBeNull();
    expect(screen.getByText("恢复应用本地存储后重试。")).not.toBeNull();
  });
});
