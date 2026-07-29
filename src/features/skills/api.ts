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
  OperationResult,
  PlannedOperation,
  QuarantineEntry,
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
  planQuarantine: (skillId: string) =>
    invoke<PlannedOperation>("plan_skill_quarantine", { skillId }),
  executeQuarantine: (confirmationToken: string, acknowledgement?: string) =>
    invoke<OperationResult>("execute_skill_quarantine", { confirmationToken, acknowledgement }),
  listQuarantineEntries: () => invoke<QuarantineEntry[]>("list_quarantine_entries"),
  planRestore: (entryId: string) =>
    invoke<PlannedOperation>("plan_skill_restore", { entryId }),
  executeRestore: (confirmationToken: string) =>
    invoke<OperationResult>("execute_skill_restore", { confirmationToken }),
  planPurge: (entryId: string) =>
    invoke<PlannedOperation>("plan_quarantine_purge", { entryId }),
  executePurge: (confirmationToken: string, acknowledgement: string) =>
    invoke<OperationResult>("execute_quarantine_purge", { confirmationToken, acknowledgement }),
};
