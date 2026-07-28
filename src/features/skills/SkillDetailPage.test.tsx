// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { skillCatalogApi } from "./api";
import { SkillDetailPage } from "./SkillDetailPage";
import type { SkillDetail } from "./types";

vi.mock("./api", () => ({
  skillCatalogApi: {
    listProviders: vi.fn(),
    scanSkills: vi.fn(),
    listSkills: vi.fn(),
    getSkillDetail: vi.fn(),
  },
}));

const detail: SkillDetail = {
  summary: {
    id: "skill:11:user_globalreview",
    display_name: "Review",
    description: "Review code safely",
    provider: {
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
      availability: "available",
    },
    scope: "user",
    validity: "valid",
    analysis_status: "not_configured",
    size_bytes: 128,
    updated_at_ms: 1_700_000_000_000,
    diagnostics: [],
  },
  headings: [{ level: 1, text: "Review workflow", line_start: 1, line_end: 1 }],
  resources: [{ relative_path: "references/checklist.md", size_bytes: 64, content_hash: "hash" }],
  diagnostics: [],
};

function renderPage(initialPath = `/skills/${encodeURIComponent(detail.summary.id)}`) {
  return render(
    <MemoryRouter initialEntries={[initialPath]}>
      <Routes>
        <Route path="/skills/:skillId" element={<SkillDetailPage />} />
        <Route path="/skills" element={<p>列表页</p>} />
      </Routes>
    </MemoryRouter>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(skillCatalogApi.getSkillDetail).mockResolvedValue(detail);
});

afterEach(() => {
  cleanup();
});

describe("SkillDetailPage", () => {
  it("loads safe static detail without requesting source", async () => {
    renderPage();

    await screen.findByRole("heading", { name: "Review" });

    expect(skillCatalogApi.getSkillDetail).toHaveBeenCalledWith(detail.summary.id);
    expect(screen.queryByTestId("skill-source")).toBeNull();
    expect(screen.getByText("AI 分析")).not.toBeNull();
    expect(screen.getByText("未配置")).not.toBeNull();
  });

  it("requests source only when the user expands it", async () => {
    vi.mocked(skillCatalogApi.getSkillDetail)
      .mockResolvedValueOnce(detail)
      .mockResolvedValueOnce({ ...detail, source: "# Review\nprivate source" });
    renderPage();
    await screen.findByRole("heading", { name: "Review" });

    expect(screen.queryByText("private source")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "查看原文" }));

    await waitFor(() => {
      expect(skillCatalogApi.getSkillDetail).toHaveBeenLastCalledWith(detail.summary.id, true);
    });
    expect(await screen.findByTestId("skill-source")).not.toBeNull();
    expect(screen.getByText(/private source/)).not.toBeNull();
  });

  it("renders a safe unavailable state for a rejected stable id", async () => {
    vi.mocked(skillCatalogApi.getSkillDetail).mockRejectedValue({
      code: "skill_not_found",
      message: "The requested Skill is unavailable.",
    });
    renderPage("/skills/not-a-path");

    expect(await screen.findByRole("alert")).not.toBeNull();
    expect(screen.getByRole("heading", { name: "无法打开 Skill" })).not.toBeNull();
    expect(screen.queryByTestId("skill-source")).toBeNull();
  });
});
