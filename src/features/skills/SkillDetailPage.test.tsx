// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { skillCatalogApi } from "./api";
import { SkillDetailPage } from "./SkillDetailPage";
import type {
  AnalysisView,
  SkillComparison,
  SkillDetail,
  SkillPassport,
  SkillSummary,
} from "./types";

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn().mockResolvedValue(() => undefined),
}));

vi.mock("./api", () => ({
  skillCatalogApi: {
    listProviders: vi.fn(),
    loadCatalog: vi.fn(),
    scanSkills: vi.fn(),
    listSkills: vi.fn(),
    getSkillDetail: vi.fn(),
    getSkillAnalysis: vi.fn(),
    analyzeSkill: vi.fn(),
    readEvidenceExcerpt: vi.fn(),
    compareSkills: vi.fn(),
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

const comparisonSkill: SkillSummary = {
  ...detail.summary,
  id: "skill:4:repo-review",
  display_name: "Review",
  provider: {
    ...detail.summary.provider,
    id: "repo",
    kind: "repo",
    display_name: "Repository",
  },
  scope: "repository",
};

const passport: SkillPassport = {
  summary: "Reviews a patch using a deterministic checklist.",
  capabilities: ["Review code"],
  triggerExamples: ["Review this patch"],
  suitableWhen: ["A patch needs review"],
  avoidWhen: ["There is no source"],
  workflow: ["Read facts", "Report findings"],
  prerequisites: ["Parsed Skill"],
  resources: [
    {
      relativePath: "references/checklist.md",
      kind: "reference",
      summary: "Review checklist",
    },
  ],
  sideEffects: ["No writes"],
  risks: [{ category: "privacy", severity: "medium", description: "May send selected text" }],
  relatedHints: ["Compare with repository Review"],
  confidence: "high",
  evidenceRefs: [
    {
      sectionId: "overview",
      relativePath: "SKILL.md",
      lineStart: 1,
      lineEnd: 2,
    },
  ],
  uncertainties: ["Runtime behavior was not executed"],
};

const emptyAnalysis: AnalysisView = {
  skill_id: detail.summary.id,
  status: "not_configured",
  cache_hit: false,
  stale: false,
  degraded: false,
  redactions: {
    api_keys: 0,
    authorization_headers: 0,
    private_keys: 0,
    secret_fields: 0,
    home_paths: 0,
  },
  sent_sections: [],
  evidence: [],
  diagnostics: [],
};

const readyAnalysis: AnalysisView = {
  ...emptyAnalysis,
  analysis_key: "analysis-key",
  status: "ready",
  passport,
  provider: "openai_compatible",
  model: "model",
  language: "zh-CN",
  analyzed_at_ms: 1_700_000_000_000,
  cache_hit: true,
  redactions: { ...emptyAnalysis.redactions, api_keys: 1 },
  sent_sections: [
    {
      id: "overview",
      relative_path: "SKILL.md",
      line_start: 1,
      line_end: 2,
      title: "Overview",
    },
  ],
  evidence: [
    {
      id: "evidence:abc",
      relative_path: "SKILL.md",
      line_start: 1,
      line_end: 2,
    },
  ],
};

const comparison: SkillComparison = {
  left: { id: detail.summary.id, display_name: "Review", provider: "User Global" },
  right: { id: comparisonSkill.id, display_name: "Review", provider: "Repository" },
  rows: [
    {
      key: "provider",
      label: "Provider",
      left: ["User Global"],
      right: ["Repository"],
      different: true,
    },
    {
      key: "purpose",
      label: "用途",
      left: ["Reviews code"],
      right: ["尚未分析"],
      different: true,
    },
  ],
};

function renderPage(initialPath = `/skills/${encodeURIComponent(detail.summary.id)}`) {
  return render(
    <MemoryRouter initialEntries={[initialPath]}>
      <Routes>
        <Route path="/skills/:skillId" element={<SkillDetailPage />} />
        <Route path="/skills" element={<p>列表页</p>} />
        <Route path="/settings" element={<p>设置页</p>} />
      </Routes>
    </MemoryRouter>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(skillCatalogApi.getSkillDetail).mockResolvedValue(detail);
  vi.mocked(skillCatalogApi.getSkillAnalysis).mockResolvedValue(emptyAnalysis);
  vi.mocked(skillCatalogApi.listSkills).mockResolvedValue({
    skills: [detail.summary, comparisonSkill],
    diagnostics: [],
  });
  vi.mocked(skillCatalogApi.analyzeSkill).mockResolvedValue({
    job_id: "analysis-job",
    status: "queued",
    deduplicated: false,
  });
  vi.mocked(skillCatalogApi.readEvidenceExcerpt).mockResolvedValue({
    evidence_id: "evidence:abc",
    relative_path: "SKILL.md",
    line_start: 1,
    line_end: 2,
    lines: [
      { number: 1, text: "# Overview" },
      { number: 2, text: "Review code" },
    ],
  });
  vi.mocked(skillCatalogApi.compareSkills).mockResolvedValue(comparison);
});

afterEach(() => {
  cleanup();
});

describe("SkillDetailPage", () => {
  it("loads safe static detail without requesting source or AI", async () => {
    renderPage();

    await screen.findByRole("heading", { name: "Review" });

    expect(skillCatalogApi.getSkillDetail).toHaveBeenCalledWith(detail.summary.id);
    expect(skillCatalogApi.analyzeSkill).not.toHaveBeenCalled();
    expect(screen.queryByTestId("skill-source")).toBeNull();
    expect(screen.getByText("未配置")).not.toBeNull();
  });

  it("requests source only when the user expands it", async () => {
    vi.mocked(skillCatalogApi.getSkillDetail)
      .mockResolvedValueOnce(detail)
      .mockResolvedValueOnce({ ...detail, source: "# Review\nprivate source" });
    renderPage();
    await screen.findByRole("heading", { name: "Review" });

    fireEvent.click(screen.getByRole("button", { name: "查看原文" }));

    await waitFor(() => {
      expect(skillCatalogApi.getSkillDetail).toHaveBeenLastCalledWith(detail.summary.id, true);
    });
    expect(await screen.findByTestId("skill-source")).not.toBeNull();
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

  it("keeps static detail available when cached analysis cannot be read", async () => {
    vi.mocked(skillCatalogApi.getSkillAnalysis).mockRejectedValue({
      code: "provider_unavailable",
      message: "AI provider unavailable.",
    });
    renderPage();

    expect(await screen.findByRole("heading", { name: "Review" })).not.toBeNull();
    expect(screen.getByText("Review code safely")).not.toBeNull();
    expect(screen.getByText("无法读取缓存护照，静态详情仍可使用。")).not.toBeNull();
    expect(screen.queryByTestId("skill-source")).toBeNull();
  });

  it("renders a complete cached passport with risk and uncertainty states", async () => {
    vi.mocked(skillCatalogApi.getSkillAnalysis).mockResolvedValue(readyAnalysis);
    renderPage();

    expect(await screen.findByText(passport.summary)).not.toBeNull();
    expect(screen.getByText("缓存命中")).not.toBeNull();
    expect(screen.getByText("May send selected text")).not.toBeNull();
    expect(screen.getByText("Runtime behavior was not executed")).not.toBeNull();
    expect(screen.getByText("1 处")).not.toBeNull();
  });

  it("opens evidence only through the backend-generated evidence id", async () => {
    vi.mocked(skillCatalogApi.getSkillAnalysis).mockResolvedValue(readyAnalysis);
    renderPage();
    const evidenceButton = await screen.findByRole("button", {
      name: "SKILL.md · 1-2 行",
    });

    fireEvent.click(evidenceButton);

    await waitFor(() => {
      expect(skillCatalogApi.readEvidenceExcerpt).toHaveBeenCalledWith("evidence:abc");
    });
    expect(await screen.findByTestId("evidence-excerpt")).not.toBeNull();
    expect(screen.getByText("# Overview")).not.toBeNull();
  });

  it("starts analysis only after an explicit click", async () => {
    vi.mocked(skillCatalogApi.getSkillAnalysis).mockResolvedValue({
      ...emptyAnalysis,
      status: "not_requested",
    });
    renderPage();
    const analyzeButton = await screen.findByRole("button", { name: "分析" });

    fireEvent.click(analyzeButton);

    await waitFor(() => {
      expect(skillCatalogApi.analyzeSkill).toHaveBeenCalledWith(detail.summary.id, false);
    });
    expect(screen.getByRole("button", { name: "分析中" })).not.toBeNull();
  });

  it("compares same-name Skills with Provider labels and highlighted differences", async () => {
    renderPage();
    const select = await screen.findByLabelText("选择对比 Skill");
    fireEvent.change(select, { target: { value: comparisonSkill.id } });
    fireEvent.click(screen.getByRole("button", { name: "比较" }));

    await waitFor(() => {
      expect(skillCatalogApi.compareSkills).toHaveBeenCalledWith([
        detail.summary.id,
        comparisonSkill.id,
      ]);
    });
    const table = await screen.findByTestId("skill-comparison");
    expect(table.textContent).toContain("User Global");
    expect(table.textContent).toContain("Repository");
    expect(table.textContent).toContain("尚未分析");
  });
});
