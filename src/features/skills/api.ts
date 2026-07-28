import { invoke } from "@tauri-apps/api/core";
import type {
  AnalysisEnqueueResult,
  AnalysisView,
  CatalogScan,
  EvidenceExcerpt,
  ProviderList,
  SkillComparison,
  SkillDetail,
  SkillList,
  SkillListQuery,
} from "./types";

export const skillCatalogApi = {
  listProviders: () => invoke<ProviderList>("list_providers"),
  loadCatalog: () => invoke<CatalogScan | null>("load_catalog"),
  scanSkills: () => invoke<CatalogScan>("scan_skills"),
  listSkills: (query: SkillListQuery) => invoke<SkillList>("list_skills", { query }),
  getSkillDetail: (skillId: string, includeSource = false) =>
    invoke<SkillDetail>("get_skill_detail", {
      skillId,
      includeSource,
    }),
  getSkillAnalysis: (skillId: string) =>
    invoke<AnalysisView>("get_skill_analysis", { skillId }),
  analyzeSkill: (skillId: string, force = false) =>
    invoke<AnalysisEnqueueResult>("analyze_skill", { skillId, force }),
  readEvidenceExcerpt: (evidenceId: string) =>
    invoke<EvidenceExcerpt>("read_evidence_excerpt", { evidenceId }),
  compareSkills: (skillIds: [string, string]) =>
    invoke<SkillComparison>("compare_skills", { skillIds }),
};
