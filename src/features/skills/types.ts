export type ProviderAvailability = "available" | "unavailable";
export type SkillScope =
  | "user"
  | "repository"
  | "legacy_user"
  | "system"
  | "plugin"
  | "bundled"
  | "additional";
export type SkillValidity = "valid" | "needs_attention";
export type SkillSort = "name" | "updated" | "size";

export interface ProviderCapabilities {
  can_read: boolean;
  can_import: boolean;
  can_quarantine: boolean;
  can_restore: boolean;
  can_update: boolean;
  can_delete: boolean;
}

export interface ProviderView {
  id: string;
  kind: string;
  display_name: string;
  capabilities: ProviderCapabilities;
  availability: ProviderAvailability;
}

export interface CatalogDiagnostic {
  code: string;
  provider_id?: string;
  relative_path?: string;
}

export interface SkillSummary {
  id: string;
  display_name: string;
  description?: string;
  provider: ProviderView;
  scope: SkillScope;
  validity: SkillValidity;
  analysis_status: "not_configured";
  size_bytes: number;
  updated_at_ms?: number;
  diagnostics: CatalogDiagnostic[];
}

export interface SkillListQuery {
  query?: string;
  providerId?: string;
  scope?: SkillScope;
  validity?: SkillValidity;
  sort: SkillSort;
}

export interface SkillList {
  skills: SkillSummary[];
  diagnostics: CatalogDiagnostic[];
}

export interface ProviderList {
  providers: ProviderView[];
  diagnostics: CatalogDiagnostic[];
}

export interface CatalogScan {
  providers: ProviderView[];
  skills: SkillSummary[];
  diagnostics: CatalogDiagnostic[];
}

export interface MarkdownHeading {
  level: number;
  text: string;
  line_start: number;
  line_end: number;
}

export interface ResourceEntry {
  relative_path: string;
  size_bytes: number;
  content_hash: string;
}

export interface SkillDetail {
  summary: SkillSummary;
  headings: MarkdownHeading[];
  resources: ResourceEntry[];
  diagnostics: CatalogDiagnostic[];
  source?: string;
}

export type ManagementOperation =
  | "skill_import"
  | "skill_quarantine"
  | "skill_restore"
  | "quarantine_purge"
  | "quarantine_keep_active"
  | "quarantine_complete";

export type OperationPlanStatus = "ready" | "conflict" | "partial";

export interface OperationImpact {
  target_provider_id: string;
  skill_name: string;
  file_count: number;
  total_size_bytes: number;
  relative_files: string[];
  entry_id?: string;
  requires_acknowledgement: boolean;
}

export interface ConfirmationToken {
  token: string;
  expires_at_ms: number;
}

export interface OperationPlan {
  id: string;
  operation: ManagementOperation;
  status: OperationPlanStatus;
  impact: OperationImpact;
}

export interface PlannedOperation {
  plan: OperationPlan;
  confirmation_token?: ConfirmationToken;
}

export interface OperationResult {
  operation_id: string;
  status: "succeeded" | "partial";
  skill_id: string;
  entry_id?: string;
}

export interface QuarantineEntry {
  id: string;
  operation_id: string;
  skill_id: string;
  provider_id: string;
  display_name: string;
  file_count: number;
  total_size_bytes: number;
  status: "pending" | "quarantined" | "partial" | "purging" | "restored";
  quarantined_at: number;
  restored_at?: number;
}

export interface CatalogError {
  code: string;
  message: string;
}

export type AnalysisRunStatus =
  | "not_requested"
  | "not_configured"
  | "ready"
  | "stale"
  | "failed"
  | "degraded";

export interface RedactionCounts {
  api_keys: number;
  authorization_headers: number;
  private_keys: number;
  secret_fields: number;
  home_paths: number;
}

export interface ResourceSummary {
  relativePath: string;
  kind: string;
  summary: string;
}

export interface RiskItem {
  category: string;
  severity: "low" | "medium" | "high";
  description: string;
}

export interface EvidenceRef {
  sectionId: string;
  relativePath: string;
  lineStart: number;
  lineEnd: number;
}

export interface SkillPassport {
  summary: string;
  capabilities: string[];
  triggerExamples: string[];
  suitableWhen: string[];
  avoidWhen: string[];
  workflow: string[];
  prerequisites: string[];
  resources: ResourceSummary[];
  sideEffects: string[];
  risks: RiskItem[];
  relatedHints: string[];
  confidence: "high" | "medium" | "low";
  evidenceRefs: EvidenceRef[];
  uncertainties: string[];
}

export interface SentSection {
  id: string;
  relative_path: string;
  line_start: number;
  line_end: number;
  title: string;
}

export interface EvidenceLink {
  id: string;
  relative_path: string;
  line_start: number;
  line_end: number;
}

export interface AnalysisView {
  skill_id: string;
  analysis_key?: string;
  status: AnalysisRunStatus;
  passport?: SkillPassport;
  provider?: string;
  model?: string;
  language?: string;
  analyzed_at_ms?: number;
  cache_hit: boolean;
  stale: boolean;
  degraded: boolean;
  redactions: RedactionCounts;
  sent_sections: SentSection[];
  evidence: EvidenceLink[];
  diagnostics: string[];
}

export interface EvidenceLine {
  number: number;
  text: string;
}

export interface EvidenceExcerpt {
  evidence_id: string;
  relative_path: string;
  line_start: number;
  line_end: number;
  lines: EvidenceLine[];
}

export interface AnalysisEnqueueResult {
  job_id?: string;
  status:
    | "queued"
    | "running"
    | "ready"
    | "stale"
    | "failed"
    | "degraded"
    | "not_configured";
  deduplicated: boolean;
}

export interface AnalysisProgress {
  jobs: Array<{
    job_id: string;
    skill_id: string;
    analysis_key?: string;
    status:
      | "queued"
      | "running"
      | "ready"
      | "stale"
      | "failed"
      | "degraded"
      | "not_configured";
  }>;
}

export interface ComparisonSkill {
  id: string;
  display_name: string;
  provider: string;
}

export interface ComparisonRow {
  key: string;
  label: string;
  left: string[];
  right: string[];
  different: boolean;
}

export interface SkillComparison {
  left: ComparisonSkill;
  right: ComparisonSkill;
  rows: ComparisonRow[];
}
