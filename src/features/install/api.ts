import { invoke } from "@tauri-apps/api/core";

export type ImportSourceKind = "file" | "directory";
export type OperationPlanStatus = "ready" | "conflict";

export interface SelectionToken {
  token: string;
  expires_at_ms: number;
}

export interface ConfirmationToken {
  token: string;
  expires_at_ms: number;
}

export interface OperationImpact {
  target_provider_id: string;
  skill_name: string;
  file_count: number;
  total_size_bytes: number;
}

export interface OperationPlan {
  id: string;
  operation: "skill_import";
  status: OperationPlanStatus;
  impact: OperationImpact;
  source?: OperationSource;
}

export interface OperationSource {
  source_type: "github";
  repository_url: string;
  repo_ref: string;
  commit_sha: string;
  subdirectory: string;
}

export interface PlannedImport {
  plan: OperationPlan;
  confirmation_token?: ConfirmationToken;
}

export interface OperationResult {
  operation_id: string;
  status: "succeeded";
  skill_id: string;
  installed_hash: string;
}

export interface OperationError {
  code: string;
  message: string;
  recovery: string;
}

export const installApi = {
  selectImportSource: (kind: ImportSourceKind) =>
    invoke<SelectionToken>("select_import_source", { kind }),
  planSkillImport: (selectionToken: string) =>
    invoke<PlannedImport>("plan_skill_import", { selectionToken }),
  planGithubImport: (repositoryUrl: string, repoRef: string, subdirectory: string) =>
    invoke<PlannedImport>("plan_github_import", {
      repositoryUrl,
      repoRef,
      subdirectory,
    }),
  executeSkillImport: (confirmationToken: string) =>
    invoke<OperationResult>("execute_skill_import", { confirmationToken }),
  cancelSkillImport: (confirmationToken: string) =>
    invoke<void>("cancel_skill_import", { confirmationToken }),
};

export function safeOperationError(failure: unknown): OperationError {
  if (
    typeof failure === "object" &&
    failure !== null &&
    "code" in failure &&
    "message" in failure &&
    "recovery" in failure &&
    typeof failure.code === "string" &&
    typeof failure.message === "string" &&
    typeof failure.recovery === "string"
  ) {
    return {
      code: failure.code,
      message: failure.message,
      recovery: failure.recovery,
    };
  }
  // Unknown bridge failures are never serialized into user-visible UI.
  return {
    code: "operation_unavailable",
    message: "导入操作暂时不可用。",
    recovery: "请重新检查 Skill 来源后再试。",
  };
}
