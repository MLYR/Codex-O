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
};
