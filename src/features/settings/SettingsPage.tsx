import {
  AlertCircle,
  BrainCircuit,
  Database,
  FolderPlus,
  HeartPulse,
  KeyRound,
  LoaderCircle,
  PackageOpen,
  Puzzle,
  RefreshCw,
  ShieldCheck,
  Trash2,
} from "lucide-react";
import { useEffect, useRef, useState, type ReactNode } from "react";
import {
  settingsApi,
  type AdditionalRootView,
  type AiConfigView,
  type AiProviderKind,
  type AiSecretAction,
  type EnvironmentHealth,
  type ScanPreferences,
} from "./api";

const providerDefaults: Record<AiProviderKind, string> = {
  open_ai_compatible: "https://api.openai.com/v1/",
  anthropic: "https://api.anthropic.com/",
  ollama: "http://127.0.0.1:11434/",
};

export function SettingsPage() {
  const [preferences, setPreferences] = useState<ScanPreferences>();
  const [aiConfig, setAiConfig] = useState<AiConfigView>();
  const [draft, setDraft] = useState<AiConfigView>();
  const [secretAction, setSecretAction] = useState<AiSecretAction>("keep");
  const [roots, setRoots] = useState<AdditionalRootView[]>([]);
  const [health, setHealth] = useState<EnvironmentHealth>();
  const [error, setError] = useState<string>();
  const [message, setMessage] = useState<string>();
  const [busyAction, setBusyAction] = useState<string>();
  const apiKeyRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    let active = true;
    void Promise.allSettled([
      settingsApi.getScanPreferences(),
      settingsApi.getAiConfig(),
      settingsApi.listAdditionalRoots(),
      settingsApi.getEnvironmentHealth(),
    ]).then(([preferencesResult, aiResult, rootsResult, healthResult]) => {
      if (!active) {
        return;
      }
      if (preferencesResult.status === "fulfilled") {
        setPreferences(preferencesResult.value);
      }
      if (aiResult.status === "fulfilled") {
        setAiConfig(aiResult.value);
        setDraft(aiResult.value);
        setSecretAction(aiResult.value.has_api_key ? "keep" : "replace");
      }
      if (rootsResult.status === "fulfilled") {
        setRoots(rootsResult.value);
      }
      if (healthResult.status === "fulfilled") {
        setHealth(healthResult.value);
      }
      if (
        [preferencesResult, aiResult, rootsResult, healthResult].some(
          (result) => result.status === "rejected",
        )
      ) {
        setError("部分设置无法读取，未受影响的功能仍可使用。");
      }
    });
    return () => {
      active = false;
    };
  }, []);

  const runAction = async (action: string, operation: () => Promise<void>) => {
    setBusyAction(action);
    setError(undefined);
    setMessage(undefined);
    try {
      await operation();
    } catch {
      setError("操作未完成，原设置保持不变。");
    } finally {
      setBusyAction(undefined);
    }
  };

  const updateScanning = (next: Partial<ScanPreferences>) => {
    if (!preferences) {
      return;
    }
    const updated = { ...preferences, ...next };
    void runAction("scan", async () => {
      setPreferences(
        await settingsApi.updateScanPreferences(
          updated.include_plugin_cache,
          updated.include_bundled_cache,
        ),
      );
      setMessage("扫描来源已保存，将在下次扫描时生效。");
    });
  };

  const saveAiConfig = () => {
    if (!draft) {
      return;
    }
    void runAction("save-ai", async () => {
      const apiKey = secretAction === "replace" ? apiKeyRef.current?.value : undefined;
      const saved = await settingsApi.saveAiConfig({
        kind: draft.kind,
        baseUrl: draft.base_url,
        model: draft.model,
        language: draft.language,
        timeoutSeconds: draft.timeout_seconds,
        privacyMode: draft.privacy_mode,
        secretAction,
        apiKey,
      });
      if (apiKeyRef.current) {
        apiKeyRef.current.value = "";
      }
      setAiConfig(saved);
      setDraft(saved);
      setSecretAction(saved.has_api_key ? "keep" : "replace");
      setMessage("AI 配置已保存并立即生效。");
      setHealth(await settingsApi.getEnvironmentHealth());
    });
  };

  const testConnection = () => {
    void runAction("test-ai", async () => {
      const result = await settingsApi.testAiConnection();
      setMessage(
        result.status === "ready"
          ? `连接可用 · ${result.latency_ms} ms`
          : `${result.code} · ${result.recommendation}`,
      );
    });
  };

  const addRoot = () => {
    void runAction("add-root", async () => {
      setRoots(await settingsApi.selectAdditionalRoot());
      setMessage("只读扫描根目录已更新。");
    });
  };

  const removeRoot = (rootId: string) => {
    void runAction(`remove-root:${rootId}`, async () => {
      setRoots(await settingsApi.removeAdditionalRoot(rootId));
      setMessage("扫描根目录已移除。");
    });
  };

  const refreshHealth = () => {
    void runAction("health", async () => {
      setHealth(await settingsApi.getEnvironmentHealth());
      setMessage("环境状态已刷新。");
    });
  };

  return (
    <section className="settings-page" aria-labelledby="settings-title">
      <header className="settings-header">
        <div>
          <h1 id="settings-title">设置</h1>
          <p>AI、Skill 来源与本机环境</p>
        </div>
      </header>

      <div className="settings-feedback" aria-live="polite">
        {error ? (
          <>
            <AlertCircle size={15} aria-hidden="true" />
            <span>{error}</span>
          </>
        ) : message ? (
          <>
            <ShieldCheck size={15} aria-hidden="true" />
            <span>{message}</span>
          </>
        ) : null}
      </div>

      <section className="settings-section" aria-labelledby="ai-settings-title">
        <div className="settings-section-heading">
          <BrainCircuit size={18} aria-hidden="true" />
          <div>
            <h2 id="ai-settings-title">AI 分析</h2>
            <p>仅在详情页明确请求时调用</p>
          </div>
        </div>

        {draft ? (
          <div className="settings-form">
            <label>
              <span>Provider</span>
              <select
                aria-label="AI Provider"
                value={draft.kind}
                onChange={(event) => {
                  const kind = event.target.value as AiProviderKind;
                  setDraft({ ...draft, kind, base_url: providerDefaults[kind] });
                }}
              >
                <option value="open_ai_compatible">OpenAI-compatible</option>
                <option value="anthropic">Anthropic</option>
                <option value="ollama">Ollama</option>
              </select>
            </label>
            <label className="settings-field-wide">
              <span>Base URL</span>
              <input
                aria-label="AI Base URL"
                value={draft.base_url}
                onChange={(event) => setDraft({ ...draft, base_url: event.target.value })}
              />
            </label>
            <label>
              <span>Model</span>
              <input
                aria-label="AI Model"
                value={draft.model}
                onChange={(event) => setDraft({ ...draft, model: event.target.value })}
              />
            </label>
            <label>
              <span>Language</span>
              <select
                aria-label="分析语言"
                value={draft.language}
                onChange={(event) => setDraft({ ...draft, language: event.target.value })}
              >
                <option value="zh-CN">简体中文</option>
                <option value="en">English</option>
              </select>
            </label>
            <label>
              <span>Timeout</span>
              <input
                aria-label="连接超时秒数"
                type="number"
                min="1"
                max="300"
                value={draft.timeout_seconds}
                onChange={(event) =>
                  setDraft({ ...draft, timeout_seconds: Number(event.target.value) })
                }
              />
            </label>

            <div className="settings-key-row settings-field-wide">
              <div className="settings-key-heading">
                <KeyRound size={16} aria-hidden="true" />
                <span>API Key</span>
                <small>{aiConfig?.has_api_key ? "系统安全存储中已有密钥" : "尚未保存密钥"}</small>
              </div>
              <div className="segmented-control" aria-label="API Key 操作">
                <button
                  type="button"
                  className={secretAction === "keep" ? "is-active" : ""}
                  disabled={!aiConfig?.has_api_key}
                  onClick={() => setSecretAction("keep")}
                >
                  保持
                </button>
                <button
                  type="button"
                  className={secretAction === "replace" ? "is-active" : ""}
                  onClick={() => setSecretAction("replace")}
                >
                  替换
                </button>
                <button
                  type="button"
                  className={secretAction === "clear" ? "is-active" : ""}
                  disabled={!aiConfig?.has_api_key}
                  onClick={() => setSecretAction("clear")}
                >
                  清除
                </button>
              </div>
              {secretAction === "replace" ? (
                <input
                  ref={apiKeyRef}
                  aria-label="新的 API Key"
                  type="password"
                  autoComplete="off"
                  placeholder="输入后仅发送到 Rust Keyring 边界"
                />
              ) : null}
            </div>

            <div className="settings-row settings-row-compact settings-field-wide">
              <span className="settings-icon" aria-hidden="true">
                <ShieldCheck size={18} />
              </span>
              <div className="settings-copy">
                <strong>隐私模式</strong>
                <span>远程 Provider 停止请求，仅允许 loopback Ollama 与已有缓存</span>
              </div>
              <Toggle
                label="隐私模式"
                checked={draft.privacy_mode}
                disabled={busyAction === "save-ai"}
                onChange={(enabled) => setDraft({ ...draft, privacy_mode: enabled })}
              />
            </div>

            <div className="settings-actions settings-field-wide">
              <button
                className="primary-button"
                type="button"
                disabled={busyAction === "save-ai"}
                onClick={saveAiConfig}
              >
                {busyAction === "save-ai" ? <LoaderCircle className="is-spinning" size={15} /> : null}
                保存配置
              </button>
              <button
                className="secondary-button"
                type="button"
                disabled={!aiConfig?.configured || busyAction === "test-ai"}
                onClick={testConnection}
              >
                {busyAction === "test-ai" ? <LoaderCircle className="is-spinning" size={15} /> : null}
                测试连接
              </button>
            </div>
          </div>
        ) : (
          <div className="settings-loading">
            <LoaderCircle className="is-spinning" size={18} />
            正在读取 AI 配置
          </div>
        )}
      </section>

      <section className="settings-section" aria-labelledby="source-settings-title">
        <div className="settings-section-heading">
          <Puzzle size={18} aria-hidden="true" />
          <div>
            <h2 id="source-settings-title">Skill 来源</h2>
            <p>来源设置在下次手动扫描时生效</p>
          </div>
        </div>

        <div className="settings-source-rows">
          <SourceToggle
            icon={<Puzzle size={18} />}
            title="Plugin Skills"
            detail="插件缓存 · 只读"
            label="扫描 Plugin Skills"
            checked={preferences?.include_plugin_cache}
            disabled={!preferences || busyAction === "scan"}
            onChange={(enabled) => updateScanning({ include_plugin_cache: enabled })}
          />
          <SourceToggle
            icon={<PackageOpen size={18} />}
            title="Bundled Skills"
            detail="OpenAI bundled · 只读"
            label="扫描 Bundled Skills"
            checked={preferences?.include_bundled_cache}
            disabled={!preferences || busyAction === "scan"}
            onChange={(enabled) => updateScanning({ include_bundled_cache: enabled })}
          />
        </div>

        <div className="additional-roots">
          <div className="additional-roots-heading">
            <div>
              <strong>Additional Roots</strong>
              <span>由 Rust 原生目录选择器管理</span>
            </div>
            <button
              className="secondary-button icon-button-label"
              type="button"
              disabled={busyAction === "add-root"}
              onClick={addRoot}
            >
              <FolderPlus size={16} aria-hidden="true" />
              添加
            </button>
          </div>
          {roots.length ? (
            <ul className="additional-root-list">
              {roots.map((root) => (
                <li key={root.id}>
                  <div>
                    <strong>{root.display_name}</strong>
                    <span>Additional Root · 只读</span>
                  </div>
                  <button
                    className="icon-only-button"
                    type="button"
                    aria-label={`移除 ${root.display_name}`}
                    title={`移除 ${root.display_name}`}
                    disabled={busyAction === `remove-root:${root.id}`}
                    onClick={() => removeRoot(root.id)}
                  >
                    <Trash2 size={16} aria-hidden="true" />
                  </button>
                </li>
              ))}
            </ul>
          ) : (
            <p className="settings-empty">未添加额外扫描根目录。</p>
          )}
        </div>
      </section>

      <section className="settings-section" aria-labelledby="health-title">
        <div className="settings-section-heading settings-section-heading-action">
          <HeartPulse size={18} aria-hidden="true" />
          <div>
            <h2 id="health-title">环境健康</h2>
            <p>只返回稳定状态码与恢复建议</p>
          </div>
          <button
            className="icon-only-button"
            type="button"
            aria-label="刷新环境健康"
            title="刷新环境健康"
            disabled={busyAction === "health"}
            onClick={refreshHealth}
          >
            <RefreshCw
              className={busyAction === "health" ? "is-spinning" : ""}
              size={16}
              aria-hidden="true"
            />
          </button>
        </div>
        {health ? (
          <ul className="health-list">
            {health.items.map((item) => (
              <li key={item.id} data-status={item.status}>
                <span className="health-icon" aria-hidden="true">
                  {item.id === "app_database" || item.id === "codex_data_source" ? (
                    <Database size={16} />
                  ) : item.id === "skill_catalog" ? (
                    <Puzzle size={16} />
                  ) : (
                    <ShieldCheck size={16} />
                  )}
                </span>
                <div>
                  <strong>{healthLabel(item.id)}</strong>
                  <code>{item.code}</code>
                  <span>{item.recommendation}</span>
                </div>
                <b>{healthStatusLabel(item.status)}</b>
              </li>
            ))}
          </ul>
        ) : (
          <div className="settings-loading">
            <LoaderCircle className="is-spinning" size={18} />
            正在检查本机环境
          </div>
        )}
      </section>
    </section>
  );
}

function Toggle({
  label,
  checked,
  disabled,
  onChange,
}: {
  label: string;
  checked: boolean;
  disabled?: boolean;
  onChange: (enabled: boolean) => void;
}) {
  return (
    <label className="toggle-control">
      <input
        type="checkbox"
        role="switch"
        aria-label={label}
        checked={checked}
        disabled={disabled}
        onChange={(event) => onChange(event.target.checked)}
      />
      <span className="toggle-track" aria-hidden="true">
        <span className="toggle-thumb" />
      </span>
    </label>
  );
}

function SourceToggle({
  icon,
  title,
  detail,
  label,
  checked,
  disabled,
  onChange,
}: {
  icon: ReactNode;
  title: string;
  detail: string;
  label: string;
  checked?: boolean;
  disabled: boolean;
  onChange: (enabled: boolean) => void;
}) {
  return (
    <div className="settings-row">
      <span className="settings-icon" aria-hidden="true">
        {icon}
      </span>
      <div className="settings-copy">
        <strong>{title}</strong>
        <span>{detail}</span>
      </div>
      {checked === undefined ? (
        <LoaderCircle className="is-spinning" size={18} aria-label="正在读取设置" />
      ) : (
        <Toggle label={label} checked={checked} disabled={disabled} onChange={onChange} />
      )}
    </div>
  );
}

function healthLabel(id: string) {
  const labels: Record<string, string> = {
    app_database: "应用数据库",
    keyring: "系统安全存储",
    ai: "AI Provider",
    skill_catalog: "Skill Catalog",
    codex_data_source: "Codex 数据源",
  };
  return labels[id] ?? id;
}

function healthStatusLabel(status: string) {
  const labels: Record<string, string> = {
    ready: "正常",
    warning: "注意",
    error: "错误",
    unavailable: "不可用",
  };
  return labels[status] ?? status;
}
