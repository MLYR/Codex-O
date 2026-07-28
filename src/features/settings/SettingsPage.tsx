import { AlertCircle, LoaderCircle, Puzzle, ShieldCheck } from "lucide-react";
import { useEffect, useState } from "react";
import { settingsApi, type ScanPreferences } from "./api";

export function SettingsPage() {
  const [preferences, setPreferences] = useState<ScanPreferences>();
  const [error, setError] = useState<string>();
  const [isSaving, setIsSaving] = useState(false);

  useEffect(() => {
    let active = true;
    void settingsApi
      .getScanPreferences()
      .then((response) => {
        if (active) {
          setPreferences(response);
        }
      })
      .catch(() => {
        if (active) {
          setError("无法读取扫描设置。");
        }
      });
    return () => {
      active = false;
    };
  }, []);

  const updatePluginScanning = async (enabled: boolean) => {
    setIsSaving(true);
    setError(undefined);
    try {
      setPreferences(await settingsApi.updateScanPreferences(enabled));
    } catch {
      setError("扫描设置未保存，原设置保持不变。");
    } finally {
      setIsSaving(false);
    }
  };

  return (
    <section className="settings-page" aria-labelledby="settings-title">
      <header className="settings-header">
        <div>
          <h1 id="settings-title">设置</h1>
          <p>Skill 来源</p>
        </div>
      </header>

      <div className="settings-group" aria-label="Skill 扫描设置">
        <div className="settings-row">
          <span className="settings-icon" aria-hidden="true">
            <Puzzle size={18} />
          </span>
          <div className="settings-copy">
            <strong>扫描插件与内置 Skill</strong>
            <span>Plugin / Bundled · 只读</span>
          </div>
          {preferences ? (
            <label className="toggle-control">
              <input
                type="checkbox"
                role="switch"
                aria-label="扫描插件与内置 Skill"
                checked={preferences.include_plugin_cache}
                disabled={isSaving}
                onChange={(event) => void updatePluginScanning(event.target.checked)}
              />
              <span className="toggle-track" aria-hidden="true">
                <span className="toggle-thumb" />
              </span>
            </label>
          ) : (
            <LoaderCircle className="is-spinning" size={18} aria-label="正在读取设置" />
          )}
        </div>

        <div className="settings-status" aria-live="polite">
          {error ? (
            <>
              <AlertCircle size={15} aria-hidden="true" />
              <span>{error}</span>
            </>
          ) : (
            <>
              <ShieldCheck size={15} aria-hidden="true" />
              <span>
                {isSaving
                  ? "正在保存设置"
                  : preferences?.include_plugin_cache
                    ? "已开启，将在下次扫描时生效"
                    : "已关闭"}
              </span>
            </>
          )}
        </div>
      </div>
    </section>
  );
}
