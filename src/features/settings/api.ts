import { invoke } from "@tauri-apps/api/core";

export interface ScanPreferences {
  include_plugin_cache: boolean;
  include_bundled_cache: boolean;
  initial_scan_notice_seen: boolean;
}

export type AiProviderKind = "open_ai_compatible" | "anthropic" | "ollama";
export type AiSecretAction = "keep" | "replace" | "clear";

export interface AiConfigView {
  configured: boolean;
  kind: AiProviderKind;
  base_url: string;
  model: string;
  language: string;
  timeout_seconds: number;
  privacy_mode: boolean;
  has_api_key: boolean;
}

export interface AiConfigInput {
  kind: AiProviderKind;
  baseUrl: string;
  model: string;
  language: string;
  timeoutSeconds: number;
  privacyMode: boolean;
  secretAction: AiSecretAction;
  apiKey?: string;
}

export interface ConnectionTestResult {
  status: "ready" | "failed" | "blocked";
  code: string;
  latency_ms: number;
  recommendation: string;
}

export interface AdditionalRootView {
  id: string;
  display_name: string;
  read_only: boolean;
}

export interface HealthItem {
  id: string;
  status: "ready" | "warning" | "error" | "unavailable";
  code: string;
  recommendation: string;
}

export interface EnvironmentHealth {
  items: HealthItem[];
}

export type DiagnosticLevel = "info" | "warning" | "error";
export type DiagnosticDomain =
  | "app"
  | "database"
  | "catalog"
  | "skill_scan"
  | "analysis"
  | "settings"
  | "environment"
  | "diagnostics";
export type DiagnosticResult = "started" | "succeeded" | "failed" | "degraded";
export type DiagnosticErrorCode =
  | "developer_mode_required"
  | "log_store_unavailable"
  | "log_export_failed"
  | "selection_unavailable"
  | "database_unavailable"
  | "database_schema_incompatible"
  | "scan_failed"
  | "scan_in_progress"
  | "analysis_not_configured"
  | "analysis_failed"
  | "settings_unavailable"
  | "invalid_configuration"
  | "privacy_remote_blocked"
  | "ai_not_configured"
  | "secret_unavailable"
  | "path_not_allowed";

export interface DeveloperSettingsView {
  developer_mode_enabled: boolean;
  store_status: "available" | "memory_only";
  memory_capacity: number;
  file_limit: number;
  total_bytes_limit: number;
}

export interface DiagnosticRecord {
  id: string;
  occurred_at: number;
  level: DiagnosticLevel;
  domain: DiagnosticDomain;
  event_code: string;
  result: DiagnosticResult;
  duration_ms?: number;
  error_code?: DiagnosticErrorCode;
  retryable: boolean;
  recovery_code?: string;
  provider_kind?: string;
  item_count?: number;
  byte_count?: number;
  dropped_count?: number;
  entity_ref?: string;
}

export interface DiagnosticQuery {
  level?: DiagnosticLevel;
  domain?: DiagnosticDomain;
  result?: DiagnosticResult;
  errorCode?: DiagnosticErrorCode;
  eventId?: string;
  limit?: number;
}

export interface DiagnosticPage {
  records: DiagnosticRecord[];
  total: number;
  store_status: "available" | "memory_only";
  dropped_count: number;
}

export interface DiagnosticExportResult {
  record_count: number;
  file_name: string;
}

export interface DiagnosticClearResult {
  memory_records_cleared: number;
  files_cleared: number;
}

export const settingsApi = {
  getScanPreferences: () => invoke<ScanPreferences>("get_scan_preferences"),
  updateScanPreferences: (includePluginCache: boolean, includeBundledCache: boolean) =>
    invoke<ScanPreferences>("update_scan_preferences", {
      includePluginCache,
      includeBundledCache,
    }),
  acknowledgeInitialScanNotice: () =>
    invoke<ScanPreferences>("acknowledge_initial_scan_notice"),
  getAiConfig: () => invoke<AiConfigView>("get_ai_config"),
  saveAiConfig: (input: AiConfigInput) => invoke<AiConfigView>("save_ai_config", { input }),
  testAiConnection: () => invoke<ConnectionTestResult>("test_ai_connection"),
  getEnvironmentHealth: () => invoke<EnvironmentHealth>("get_environment_health"),
  listAdditionalRoots: () => invoke<AdditionalRootView[]>("list_additional_roots"),
  selectAdditionalRoot: () => invoke<AdditionalRootView[]>("select_additional_root"),
  removeAdditionalRoot: (rootId: string) =>
    invoke<AdditionalRootView[]>("remove_additional_root", { rootId }),
  getDeveloperSettings: () =>
    invoke<DeveloperSettingsView>("get_developer_settings"),
  setDeveloperMode: (enabled: boolean) =>
    invoke<DeveloperSettingsView>("set_developer_mode", { enabled }),
  listDiagnostics: (query: DiagnosticQuery) =>
    invoke<DiagnosticPage>("list_diagnostics", { query }),
  exportDiagnostics: (query: DiagnosticQuery) =>
    invoke<DiagnosticExportResult>("export_diagnostics", { query }),
  clearDiagnostics: () =>
    invoke<DiagnosticClearResult>("clear_diagnostics"),
};
