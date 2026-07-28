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

export interface CatalogError {
  code: string;
  message: string;
}
