import { invoke } from "@tauri-apps/api/core";
import type { OperationResult, PlannedImport } from "../install/api";

export type SkillUpdateStatus = "current" | "available" | "conflict" | "unavailable";

export interface SkillUpdateSummary {
  skill_id: string;
  display_name: string;
  source_type: string;
  status: SkillUpdateStatus;
  installed_commit?: string;
  available_commit?: string;
  checked_at_ms: number;
  reason: string;
  changed_files: string[];
}

export const updatesApi = {
  check: () => invoke<SkillUpdateSummary[]>("check_skill_updates"),
  plan: (skillId: string) => invoke<PlannedImport>("plan_skill_update", { skillId }),
  execute: (confirmationToken: string) =>
    invoke<OperationResult>("execute_skill_update", { confirmationToken }),
};
