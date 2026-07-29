// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { skillCatalogApi } from "./api";
import { OperationDialog, QuarantinePanel } from "./QuarantinePanel";
import type { PlannedOperation, QuarantineEntry } from "./types";

vi.mock("./api", () => ({
  skillCatalogApi: {
    listQuarantineEntries: vi.fn(),
    planRestore: vi.fn(),
    executeRestore: vi.fn(),
    planKeepActive: vi.fn(),
    executeKeepActive: vi.fn(),
    planCompleteQuarantine: vi.fn(),
    executeCompleteQuarantine: vi.fn(),
    planPurge: vi.fn(),
    executePurge: vi.fn(),
  },
}));

const entry: QuarantineEntry = {
  id: "a".repeat(64), operation_id: "a".repeat(64), skill_id: "skill", provider_id: "user_global",
  display_name: "Review", file_count: 2,
  total_size_bytes: 128, status: "quarantined", quarantined_at: 1_700_000_000_000,
};
const restorePlan: PlannedOperation = {
  plan: { id: "b".repeat(64), operation: "skill_restore", status: "ready", impact: { target_provider_id: "user_global", skill_name: "Review", file_count: 2, total_size_bytes: 128, relative_files: ["SKILL.md", "resources/check.md"], entry_id: entry.id, requires_acknowledgement: false } },
  confirmation_token: { token: "token", expires_at_ms: 1 },
};
const purgePlan: PlannedOperation = { ...restorePlan, plan: { ...restorePlan.plan, operation: "quarantine_purge", impact: { ...restorePlan.plan.impact, requires_acknowledgement: true } } };
const keepActivePlan: PlannedOperation = { ...restorePlan, plan: { ...restorePlan.plan, operation: "quarantine_keep_active" } };
const completePlan: PlannedOperation = { ...restorePlan, plan: { ...restorePlan.plan, operation: "quarantine_complete" } };

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(skillCatalogApi.listQuarantineEntries).mockResolvedValue([]);
  vi.mocked(skillCatalogApi.planRestore).mockResolvedValue(restorePlan);
  vi.mocked(skillCatalogApi.planKeepActive).mockResolvedValue(keepActivePlan);
  vi.mocked(skillCatalogApi.planCompleteQuarantine).mockResolvedValue(completePlan);
  vi.mocked(skillCatalogApi.planPurge).mockResolvedValue(purgePlan);
  vi.mocked(skillCatalogApi.executeRestore).mockResolvedValue({ operation_id: "x", status: "succeeded", skill_id: "skill" });
  vi.mocked(skillCatalogApi.executeKeepActive).mockResolvedValue({ operation_id: "x", status: "succeeded", skill_id: "skill" });
  vi.mocked(skillCatalogApi.executeCompleteQuarantine).mockResolvedValue({ operation_id: "x", status: "succeeded", skill_id: "skill" });
  vi.mocked(skillCatalogApi.executePurge).mockResolvedValue({ operation_id: "x", status: "succeeded", skill_id: "skill" });
});
afterEach(cleanup);

describe("QuarantinePanel", () => {
  it("shows an empty quarantine state", async () => {
    render(<QuarantinePanel />);
    expect(await screen.findByRole("heading", { name: "隔离区为空" })).not.toBeNull();
  });

  it("renders a recorded entry without exposing its original path", async () => {
    vi.mocked(skillCatalogApi.listQuarantineEntries).mockResolvedValue([entry]);
    render(<QuarantinePanel />);
    await screen.findByText("Review");
    expect(screen.queryByText("review")).toBeNull();
  });

  it("requests a restore plan from the stable entry id", async () => {
    vi.mocked(skillCatalogApi.listQuarantineEntries).mockResolvedValue([entry]);
    render(<QuarantinePanel />);
    fireEvent.click(await screen.findByRole("button", { name: "恢复 Review" }));
    await waitFor(() => expect(skillCatalogApi.planRestore).toHaveBeenCalledWith(entry.id));
    expect(screen.getByRole("dialog", { name: "恢复 Skill" })).not.toBeNull();
  });

  it("reports a restore conflict without displaying a confirmation token", async () => {
    vi.mocked(skillCatalogApi.listQuarantineEntries).mockResolvedValue([entry]);
    vi.mocked(skillCatalogApi.planRestore).mockResolvedValue({
      ...restorePlan,
      plan: { ...restorePlan.plan, status: "conflict" },
      confirmation_token: undefined,
    });
    render(<QuarantinePanel />);
    fireEvent.click(await screen.findByRole("button", { name: "恢复 Review" }));
    expect((await screen.findByRole("alert")).textContent).toContain("conflict_detected");
    expect(screen.queryByRole("dialog", { name: "恢复 Skill" })).toBeNull();
  });

  it("shows the complete relative-file plan", async () => {
    render(<OperationDialog planned={restorePlan} acknowledgement="" onAcknowledgement={() => undefined} busy={false} onClose={() => undefined} onConfirm={() => undefined} />);
    expect(screen.getByText("SKILL.md")).not.toBeNull();
    expect(screen.getByText("resources/check.md")).not.toBeNull();
  });

  it("does not enable a destructive confirmation without matching acknowledgement", () => {
    render(<OperationDialog planned={purgePlan} acknowledgement="wrong" onAcknowledgement={() => undefined} busy={false} onClose={() => undefined} onConfirm={() => undefined} />);
    expect((screen.getByRole("button", { name: "确认" }) as HTMLButtonElement).disabled).toBe(true);
  });

  it("enables a destructive confirmation for the displayed name", () => {
    render(<OperationDialog planned={purgePlan} acknowledgement="Review" onAcknowledgement={() => undefined} busy={false} onClose={() => undefined} onConfirm={() => undefined} />);
    expect((screen.getByRole("button", { name: "确认" }) as HTMLButtonElement).disabled).toBe(false);
  });

  it("executes restore with only its confirmation token", async () => {
    vi.mocked(skillCatalogApi.listQuarantineEntries).mockResolvedValue([entry]);
    render(<QuarantinePanel />);
    fireEvent.click(await screen.findByRole("button", { name: "恢复 Review" }));
    fireEvent.click(await screen.findByRole("button", { name: "确认" }));
    await waitFor(() => expect(skillCatalogApi.executeRestore).toHaveBeenCalledWith("token"));
  });

  it("does not show purge controls for partial entries", async () => {
    vi.mocked(skillCatalogApi.listQuarantineEntries).mockResolvedValue([{ ...entry, status: "partial" }]);
    render(<QuarantinePanel />);
    await screen.findByText("需处理");
    expect(screen.queryByRole("button", { name: "永久清理 Review" })).toBeNull();
  });

  it("opens the keep-active confirmation for a partial entry", async () => {
    vi.mocked(skillCatalogApi.listQuarantineEntries).mockResolvedValue([{ ...entry, status: "partial" }]);
    render(<QuarantinePanel />);
    fireEvent.click(await screen.findByRole("button", { name: "保留活动副本 Review" }));
    await waitFor(() => expect(skillCatalogApi.planKeepActive).toHaveBeenCalledWith(entry.id));
    expect(screen.getByRole("dialog", { name: "保留活动副本" })).not.toBeNull();
  });

  it("opens the complete-quarantine confirmation for a partial entry", async () => {
    vi.mocked(skillCatalogApi.listQuarantineEntries).mockResolvedValue([{ ...entry, status: "partial" }]);
    render(<QuarantinePanel />);
    fireEvent.click(await screen.findByRole("button", { name: "完成隔离 Review" }));
    await waitFor(() => expect(skillCatalogApi.planCompleteQuarantine).toHaveBeenCalledWith(entry.id));
    expect(screen.getByRole("dialog", { name: "完成隔离" })).not.toBeNull();
  });

  it("executes keep-active with its confirmation token", async () => {
    vi.mocked(skillCatalogApi.listQuarantineEntries).mockResolvedValue([{ ...entry, status: "partial" }]);
    render(<QuarantinePanel />);
    fireEvent.click(await screen.findByRole("button", { name: "保留活动副本 Review" }));
    fireEvent.click(await screen.findByRole("button", { name: "确认" }));
    await waitFor(() => expect(skillCatalogApi.executeKeepActive).toHaveBeenCalledWith("token"));
  });

  it("shows a stable error when partial convergence planning fails", async () => {
    vi.mocked(skillCatalogApi.listQuarantineEntries).mockResolvedValue([{ ...entry, status: "partial" }]);
    vi.mocked(skillCatalogApi.planCompleteQuarantine).mockRejectedValue(new Error("changed"));
    render(<QuarantinePanel />);
    fireEvent.click(await screen.findByRole("button", { name: "完成隔离 Review" }));
    expect((await screen.findByRole("alert")).textContent).toContain("quarantine_content_changed");
  });

  it("disables partial controls while a convergence plan is loading", async () => {
    vi.mocked(skillCatalogApi.listQuarantineEntries).mockResolvedValue([{ ...entry, status: "partial" }]);
    vi.mocked(skillCatalogApi.planKeepActive).mockImplementation(() => new Promise(() => undefined));
    render(<QuarantinePanel />);
    const keepActive = await screen.findByRole("button", { name: "保留活动副本 Review" });
    fireEvent.click(keepActive);
    await waitFor(() => expect((keepActive as HTMLButtonElement).disabled).toBe(true));
    expect((screen.getByRole("button", { name: "完成隔离 Review" }) as HTMLButtonElement).disabled).toBe(true);
  });
});
