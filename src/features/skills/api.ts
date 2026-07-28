import { invoke } from "@tauri-apps/api/core";
import type {
  CatalogScan,
  ProviderList,
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
};
