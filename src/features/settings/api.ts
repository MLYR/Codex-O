import { invoke } from "@tauri-apps/api/core";

export interface ScanPreferences {
  include_plugin_cache: boolean;
  initial_scan_notice_seen: boolean;
}

export const settingsApi = {
  getScanPreferences: () => invoke<ScanPreferences>("get_scan_preferences"),
  updateScanPreferences: (includePluginCache: boolean) =>
    invoke<ScanPreferences>("update_scan_preferences", { includePluginCache }),
  acknowledgeInitialScanNotice: () =>
    invoke<ScanPreferences>("acknowledge_initial_scan_notice"),
};
