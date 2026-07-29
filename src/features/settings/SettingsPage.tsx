import {
  AlertCircle,
  BrainCircuit,
  Bug,
  ClipboardCopy,
  Database,
  Download,
  FileWarning,
  FolderPlus,
  HeartPulse,
  KeyRound,
  ListFilter,
  LoaderCircle,
  PackageOpen,
  Puzzle,
  RefreshCw,
  ScrollText,
  Settings2,
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
  type DeveloperSettingsView,
  type DiagnosticDomain,
  type DiagnosticErrorCode,
  type DiagnosticLevel,
  type DiagnosticPage,
  type DiagnosticQuery,
  type DiagnosticRecord,
  type DiagnosticResult,
  type EnvironmentHealth,
  type ScanPreferences,
} from "./api";

const providerDefaults: Record<AiProviderKind, string> = {
  open_ai_compatible: "https://api.openai.com/v1/",
  anthropic: "https://api.anthropic.com/",
  ollama: "http://127.0.0.1:11434/",
};

export function SettingsPage() {
  const [activeView, setActiveView] = useState<"general" | "diagnostics">("general");
  const [preferences, setPreferences] = useState<ScanPreferences>();
  const [aiConfig, setAiConfig] = useState<AiConfigView>();
  const [draft, setDraft] = useState<AiConfigView>();
  const [secretAction, setSecretAction] = useState<AiSecretAction>("keep");
  const [roots, setRoots] = useState<AdditionalRootView[]>([]);
  const [health, setHealth] = useState<EnvironmentHealth>();
  const [developerSettings, setDeveloperSettings] = useState<DeveloperSettingsView>();
  const [diagnostics, setDiagnostics] = useState<DiagnosticPage>();
  const [diagnosticQuery, setDiagnosticQuery] = useState<DiagnosticQuery>({ limit: 200 });
  const [selectedEventId, setSelectedEventId] = useState<string>();
  const [diagnosticLoading, setDiagnosticLoading] = useState(false);
  const [clearArmed, setClearArmed] = useState(false);
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
      settingsApi.getDeveloperSettings(),
    ]).then(([preferencesResult, aiResult, rootsResult, healthResult, developerResult]) => {
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
      if (developerResult.status === "fulfilled") {
        setDeveloperSettings(developerResult.value);
      }
      if (
        [preferencesResult, aiResult, rootsResult, healthResult, developerResult].some(
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

  useEffect(() => {
    if (activeView !== "diagnostics" || !developerSettings?.developer_mode_enabled) {
      return;
    }
    let active = true;
    setDiagnosticLoading(true);
    setError(undefined);
    void settingsApi
      .listDiagnostics(diagnosticQuery)
      .then((page) => {
        if (!active) {
          return;
        }
        setDiagnostics(page);
        setSelectedEventId((current) =>
          current && page.records.some((record) => record.id === current)
            ? current
            : page.records[0]?.id,
        );
      })
      .catch((failure: unknown) => {
        if (active) {
          setError(formatOperationError(failure));
        }
      })
      .finally(() => {
        if (active) {
          setDiagnosticLoading(false);
        }
      });
    return () => {
      active = false;
    };
  }, [activeView, developerSettings?.developer_mode_enabled, diagnosticQuery]);

  const runAction = async (action: string, operation: () => Promise<void>) => {
    setBusyAction(action);
    setError(undefined);
    setMessage(undefined);
    try {
      await operation();
    } catch (failure) {
      setError(formatOperationError(failure));
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

  const updateDeveloperMode = (enabled: boolean) => {
    void runAction("developer-mode", async () => {
      const saved = await settingsApi.setDeveloperMode(enabled);
      setDeveloperSettings(saved);
      if (!enabled) {
        setActiveView("general");
        setDiagnostics(undefined);
        setSelectedEventId(undefined);
      }
      setMessage(enabled ? "开发者模式已开启。" : "开发者模式已关闭。");
    });
  };

  const refreshDiagnostics = () => {
    setDiagnosticQuery((current) => ({ ...current }));
  };

  const exportDiagnostics = () => {
    void runAction("export-diagnostics", async () => {
      const result = await settingsApi.exportDiagnostics(diagnosticQuery);
      setMessage(`已导出 ${result.record_count} 条脱敏诊断记录。`);
    });
  };

  const clearDiagnostics = () => {
    if (!clearArmed) {
      setClearArmed(true);
      return;
    }
    void runAction("clear-diagnostics", async () => {
      const result = await settingsApi.clearDiagnostics();
      setClearArmed(false);
      setDiagnostics({
        records: [],
        total: 0,
        store_status: developerSettings?.store_status ?? "memory_only",
        dropped_count: 0,
      });
      setSelectedEventId(undefined);
      setMessage(
        `已清空 ${result.memory_records_cleared} 条内存记录和 ${result.files_cleared} 个日志文件。`,
      );
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

      <div className="settings-subnav" role="tablist" aria-label="设置视图">
        <button
          type="button"
          role="tab"
          aria-selected={activeView === "general"}
          className={activeView === "general" ? "is-active" : ""}
          onClick={() => setActiveView("general")}
        >
          <Settings2 size={15} aria-hidden="true" />
          常规设置
        </button>
        {developerSettings?.developer_mode_enabled ? (
          <button
            type="button"
            role="tab"
            aria-selected={activeView === "diagnostics"}
            className={activeView === "diagnostics" ? "is-active" : ""}
            onClick={() => setActiveView("diagnostics")}
          >
            <ScrollText size={15} aria-hidden="true" />
            诊断与日志
          </button>
        ) : null}
      </div>

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

      {activeView === "general" ? (
        <>
          <section className="settings-section" aria-labelledby="developer-mode-title">
            <div className="settings-row settings-row-compact">
              <span className="settings-icon" aria-hidden="true">
                <Bug size={18} />
              </span>
              <div className="settings-copy">
                <strong id="developer-mode-title">开发者模式</strong>
                <span>开启后可查看、导出和清空始终脱敏的本机诊断记录</span>
              </div>
              {developerSettings ? (
                <Toggle
                  label="开发者模式"
                  checked={developerSettings.developer_mode_enabled}
                  disabled={busyAction === "developer-mode"}
                  onChange={updateDeveloperMode}
                />
              ) : (
                <LoaderCircle className="is-spinning" size={18} aria-label="正在读取开发者设置" />
              )}
            </div>
          </section>

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
        </>
      ) : developerSettings?.developer_mode_enabled ? (
        <DiagnosticsView
          page={diagnostics}
          query={diagnosticQuery}
          selectedEventId={selectedEventId}
          loading={diagnosticLoading}
          busyAction={busyAction}
          clearArmed={clearArmed}
          onQueryChange={setDiagnosticQuery}
          onSelect={setSelectedEventId}
          onRefresh={refreshDiagnostics}
          onExport={exportDiagnostics}
          onClear={clearDiagnostics}
          onCancelClear={() => setClearArmed(false)}
        />
      ) : null}
    </section>
  );
}

function DiagnosticsView({
  page,
  query,
  selectedEventId,
  loading,
  busyAction,
  clearArmed,
  onQueryChange,
  onSelect,
  onRefresh,
  onExport,
  onClear,
  onCancelClear,
}: {
  page?: DiagnosticPage;
  query: DiagnosticQuery;
  selectedEventId?: string;
  loading: boolean;
  busyAction?: string;
  clearArmed: boolean;
  onQueryChange: (query: DiagnosticQuery) => void;
  onSelect: (eventId: string) => void;
  onRefresh: () => void;
  onExport: () => void;
  onClear: () => void;
  onCancelClear: () => void;
}) {
  const selected = page?.records.find((record) => record.id === selectedEventId);

  return (
    <section className="settings-section diagnostics-section" aria-labelledby="diagnostics-title">
      <div className="settings-section-heading diagnostics-heading">
        <ScrollText size={18} aria-hidden="true" />
        <div>
          <h2 id="diagnostics-title">诊断与日志</h2>
          <p>仅展示 Rust allowlist 生成的结构化安全字段</p>
        </div>
        <div className="diagnostics-actions">
          <button
            className="icon-only-button"
            type="button"
            aria-label="刷新诊断日志"
            title="刷新诊断日志"
            disabled={loading}
            onClick={onRefresh}
          >
            <RefreshCw className={loading ? "is-spinning" : ""} size={16} aria-hidden="true" />
          </button>
          <button
            className="secondary-button icon-button-label"
            type="button"
            disabled={busyAction === "export-diagnostics"}
            onClick={onExport}
          >
            <Download size={15} aria-hidden="true" />
            导出
          </button>
          <button
            className={clearArmed ? "danger-button icon-button-label" : "secondary-button icon-button-label"}
            type="button"
            disabled={busyAction === "clear-diagnostics"}
            onClick={onClear}
          >
            <Trash2 size={15} aria-hidden="true" />
            {clearArmed ? "确认清空" : "清空"}
          </button>
          {clearArmed ? (
            <button className="secondary-button" type="button" onClick={onCancelClear}>
              取消
            </button>
          ) : null}
        </div>
      </div>

      {page?.store_status === "memory_only" ? (
        <div className="diagnostics-store-warning">
          <FileWarning size={16} aria-hidden="true" />
          <span>日志目录不可用，当前仅保留本进程最近 500 条内存诊断。</span>
        </div>
      ) : null}

      <div className="diagnostics-filters" aria-label="诊断筛选">
        <ListFilter size={16} aria-hidden="true" />
        <label>
          <span>级别</span>
          <select
            aria-label="诊断级别"
            value={query.level ?? ""}
            onChange={(event) =>
              onQueryChange({
                ...query,
                level: (event.target.value || undefined) as DiagnosticLevel | undefined,
              })
            }
          >
            <option value="">全部</option>
            <option value="info">Info</option>
            <option value="warning">Warning</option>
            <option value="error">Error</option>
          </select>
        </label>
        <label>
          <span>模块</span>
          <select
            aria-label="诊断模块"
            value={query.domain ?? ""}
            onChange={(event) =>
              onQueryChange({
                ...query,
                domain: (event.target.value || undefined) as DiagnosticDomain | undefined,
              })
            }
          >
            <option value="">全部</option>
            <option value="app">应用</option>
            <option value="database">数据库</option>
            <option value="catalog">Catalog</option>
            <option value="skill_scan">Skill 扫描</option>
            <option value="analysis">AI 分析</option>
            <option value="settings">设置</option>
            <option value="environment">环境健康</option>
            <option value="diagnostics">诊断服务</option>
          </select>
        </label>
        <label>
          <span>结果</span>
          <select
            aria-label="诊断结果"
            value={query.result ?? ""}
            onChange={(event) =>
              onQueryChange({
                ...query,
                result: (event.target.value || undefined) as DiagnosticResult | undefined,
              })
            }
          >
            <option value="">全部</option>
            <option value="started">进行中</option>
            <option value="succeeded">成功</option>
            <option value="failed">失败</option>
            <option value="degraded">降级</option>
          </select>
        </label>
        <label>
          <span>错误码</span>
          <select
            aria-label="诊断错误码"
            value={query.errorCode ?? ""}
            onChange={(event) =>
              onQueryChange({
                ...query,
                errorCode: (event.target.value || undefined) as DiagnosticErrorCode | undefined,
              })
            }
          >
            <option value="">全部</option>
            <option value="database_unavailable">database_unavailable</option>
            <option value="database_schema_incompatible">
              database_schema_incompatible
            </option>
            <option value="scan_failed">scan_failed</option>
            <option value="scan_in_progress">scan_in_progress</option>
            <option value="analysis_not_configured">analysis_not_configured</option>
            <option value="analysis_failed">analysis_failed</option>
            <option value="settings_unavailable">settings_unavailable</option>
            <option value="invalid_configuration">invalid_configuration</option>
            <option value="privacy_remote_blocked">privacy_remote_blocked</option>
            <option value="ai_not_configured">ai_not_configured</option>
            <option value="secret_unavailable">secret_unavailable</option>
            <option value="path_not_allowed">path_not_allowed</option>
          </select>
        </label>
      </div>

      {loading && !page ? (
        <div className="settings-loading">
          <LoaderCircle className="is-spinning" size={18} />
          正在读取脱敏诊断
        </div>
      ) : page?.records.length ? (
        <div className="diagnostics-layout">
          <div className="diagnostics-list-pane">
            <div className="diagnostics-summary">
              <span>{page.total} 条匹配记录</span>
              {page.dropped_count > 0 ? <b>{page.dropped_count} 条待聚合写入</b> : null}
            </div>
            <ol className="diagnostics-list">
              {page.records.map((record) => (
                <li key={record.id}>
                  <button
                    type="button"
                    className={record.id === selectedEventId ? "is-selected" : ""}
                    aria-pressed={record.id === selectedEventId}
                    onClick={() => onSelect(record.id)}
                  >
                    <span className="diagnostic-level" data-level={record.level}>
                      {record.level}
                    </span>
                    <strong>{eventLabel(record.event_code)}</strong>
                    <time>{formatDiagnosticTime(record.occurred_at)}</time>
                    <code>{record.id}</code>
                  </button>
                </li>
              ))}
            </ol>
          </div>
          {selected ? <DiagnosticDetail record={selected} /> : null}
        </div>
      ) : (
        <div className="diagnostics-empty">
          <ShieldCheck size={22} aria-hidden="true" />
          <strong>没有匹配的诊断记录</strong>
          <span>应用会继续采集经过脱敏的 Info、Warning 和 Error 事件。</span>
        </div>
      )}
    </section>
  );
}

function DiagnosticDetail({ record }: { record: DiagnosticRecord }) {
  const copyRecord = () => {
    const serialized = JSON.stringify(record, null, 2);
    if (navigator.clipboard?.writeText) {
      void navigator.clipboard.writeText(serialized);
    }
  };
  return (
    <aside className="diagnostics-detail" aria-label="诊断详情">
      <div className="diagnostics-detail-heading">
        <div>
          <strong>{eventLabel(record.event_code)}</strong>
          <code>{record.id}</code>
        </div>
        <button
          className="icon-only-button"
          type="button"
          aria-label="复制诊断详情"
          title="复制诊断详情"
          onClick={copyRecord}
        >
          <ClipboardCopy size={15} aria-hidden="true" />
        </button>
      </div>
      <dl>
        <DiagnosticField label="时间" value={formatDiagnosticTime(record.occurred_at)} />
        <DiagnosticField label="级别" value={record.level} />
        <DiagnosticField label="模块" value={domainLabel(record.domain)} />
        <DiagnosticField label="结果" value={resultLabel(record.result)} />
        <DiagnosticField label="耗时" value={record.duration_ms === undefined ? "未记录" : `${record.duration_ms} ms`} />
        <DiagnosticField label="错误码" value={record.error_code ?? "无"} />
        <DiagnosticField label="可重试" value={record.retryable ? "是" : "否"} />
        <DiagnosticField label="恢复建议" value={recoveryLabel(record.recovery_code)} />
        <DiagnosticField label="Provider" value={record.provider_kind ?? "未关联"} />
        <DiagnosticField label="项目数" value={record.item_count?.toString() ?? "未记录"} />
        <DiagnosticField label="字节数" value={record.byte_count?.toString() ?? "未记录"} />
        <DiagnosticField label="丢弃聚合" value={record.dropped_count?.toString() ?? "0"} />
        <DiagnosticField label="实体引用" value={record.entity_ref ?? "未关联"} />
      </dl>
    </aside>
  );
}

function DiagnosticField({ label, value }: { label: string; value: string }) {
  return (
    <div>
      <dt>{label}</dt>
      <dd>{value}</dd>
    </div>
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

function eventLabel(eventCode: string) {
  const labels: Record<string, string> = {
    app_started: "应用启动",
    database_initialized: "数据库初始化",
    catalog_cache_loaded: "Catalog 缓存加载",
    frontend_ready: "前端就绪",
    skill_scan_started: "Skill 扫描开始",
    skill_scan_completed: "Skill 扫描完成",
    skill_scan_failed: "Skill 扫描失败",
    analysis_queued: "AI 分析已排队",
    analysis_retried: "AI 分析重试",
    analysis_completed: "AI 分析完成",
    analysis_failed: "AI 分析失败",
    settings_loaded: "设置读取",
    settings_saved: "设置保存",
    ai_connection_tested: "AI 连接测试",
    environment_health_checked: "环境健康检查",
    diagnostic_queue_dropped: "诊断队列降级",
    diagnostic_access_denied: "诊断访问被拒绝",
    diagnostics_exported: "诊断导出",
    diagnostics_cleared: "诊断清空",
  };
  return labels[eventCode] ?? eventCode;
}

function domainLabel(domain: string) {
  const labels: Record<string, string> = {
    app: "应用",
    database: "数据库",
    catalog: "Catalog",
    skill_scan: "Skill 扫描",
    analysis: "AI 分析",
    settings: "设置",
    environment: "环境健康",
    diagnostics: "诊断服务",
  };
  return labels[domain] ?? domain;
}

function resultLabel(result: string) {
  const labels: Record<string, string> = {
    started: "进行中",
    succeeded: "成功",
    failed: "失败",
    degraded: "降级",
  };
  return labels[result] ?? result;
}

function recoveryLabel(recoveryCode?: string) {
  const labels: Record<string, string> = {
    retry: "重试当前操作",
    check_settings: "检查相关设置",
    rescan: "重新扫描",
    restart_application: "重启应用",
    continue_with_memory_diagnostics: "继续使用内存诊断",
  };
  return recoveryCode ? (labels[recoveryCode] ?? recoveryCode) : "无";
}

function formatDiagnosticTime(occurredAt: number) {
  return new Intl.DateTimeFormat("zh-CN", {
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    hour12: false,
  }).format(new Date(occurredAt));
}

function formatOperationError(failure: unknown) {
  if (failure && typeof failure === "object") {
    const candidate = failure as Record<string, unknown>;
    const code =
      typeof candidate.code === "string" && /^[a-z0-9_]{1,64}$/.test(candidate.code)
        ? candidate.code
        : undefined;
    const eventId =
      typeof candidate.event_id === "string" &&
      /^evt-[a-f0-9]{16}-[a-f0-9]{16}$/.test(candidate.event_id)
        ? candidate.event_id
        : undefined;
    if (code) {
      return eventId ? `操作未完成 · ${code} · ${eventId}` : `操作未完成 · ${code}`;
    }
  }
  return "操作未完成，原设置保持不变。";
}
