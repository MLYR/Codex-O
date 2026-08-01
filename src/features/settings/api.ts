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
export type LogCategory = "system" | "diagnostic" | "ai" | "skill_mcp";
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

export interface DiagnosticRecord {
  schema_version: number;
  event_id: string;
  occurred_at: number;
  level: DiagnosticLevel;
  category: LogCategory;
  domain: DiagnosticDomain;
  event_code: string;
  result: DiagnosticResult;
  module: string;
  submodule?: string;
  duration_ms?: number;
  trace_id?: string;
  request_ref?: string;
  provider?: string;
  model?: string;
  http_status?: number;
  item_count?: number;
  error_code?: DiagnosticErrorCode;
  retryable: boolean;
  recovery_code?: string;
  redaction_version: number;
}

export interface LogQuery {
  level?: DiagnosticLevel;
  category?: LogCategory;
  module?: string;
  result?: DiagnosticResult;
  fromOccurredAt?: number;
  toOccurredAt?: number;
  traceId?: string;
  eventId?: string;
  requestRef?: string;
  cursor?: string;
  limit?: number;
}

export interface LogStats {
  total: number;
  errors: number;
  warnings: number;
  ai_calls: number;
}

export interface LogCoverage {
  oldest_occurred_at?: number;
  newest_occurred_at?: number;
  historical_comparison_available: boolean;
}

export interface LogSnapshot {
  records: DiagnosticRecord[];
  stats: LogStats;
  filters: { modules: string[]; categories: LogCategory[] };
  coverage: LogCoverage;
  cursor?: string;
  storage_status: "available" | "unavailable";
  invalid_line_count: number;
}

export interface DiagnosticBundleResult {
  record_count: number;
  file_name: string;
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
  readLogSnapshot: (query: LogQuery) =>
    invoke<LogSnapshot>("read_log_snapshot", { query }),
  clearLogLogical: () => invoke<string>("clear_log_logical"),
  setLogPhysicalCleanupOnStart: (requested: boolean) =>
    invoke<void>("set_log_physical_cleanup_on_start", { requested }),
  exportDiagnosticBundle: (query: LogQuery) =>
    invoke<DiagnosticBundleResult>("export_diagnostic_bundle", { query }),
};
