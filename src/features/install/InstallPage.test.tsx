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
      executeSkillImport: vi.fn(),
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

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(installApi.selectImportSource).mockResolvedValue({
    token: "selection-secret",
    expires_at_ms: 1_800_000_000_000,
  });
  vi.mocked(installApi.planSkillImport).mockResolvedValue(readyPlan);
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
    expect(await screen.findByRole("heading", { name: "Skill 已导入" })).not.toBeNull();
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
  });
});
