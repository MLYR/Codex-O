import {
  AlertCircle,
  CheckCircle2,
  CircleDot,
  GitCompareArrows,
  LoaderCircle,
  RefreshCw,
  ShieldAlert,
} from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { installApi, safeOperationError, type OperationError, type PlannedImport } from "../install/api";
import { OperationPlanModal } from "../install/OperationPlanModal";
import { updatesApi, type SkillUpdateStatus, type SkillUpdateSummary } from "./api";
import "../install/InstallPage.css";
import "./UpdatesPage.css";

const statusCopy: Record<SkillUpdateStatus, string> = {
  current: "已是最新",
  available: "可更新",
  conflict: "本地修改",
  unavailable: "暂不可用",
};

export function UpdatesPage() {
  const [items, setItems] = useState<SkillUpdateSummary[]>();
  const [checking, setChecking] = useState(false);
  const [planningId, setPlanningId] = useState<string>();
  const [plannedUpdate, setPlannedUpdate] = useState<PlannedImport>();
  const [executing, setExecuting] = useState(false);
  const [error, setError] = useState<OperationError>();
  const [completedName, setCompletedName] = useState<string>();
  const activeConfirmation = useRef<string | undefined>(undefined);

  const cancelPlan = useCallback(() => {
    if (executing) return;
    const token = activeConfirmation.current;
    activeConfirmation.current = undefined;
    setPlannedUpdate(undefined);
    // The backend owns staging, so UI disposal always sends the opaque token back for cleanup.
    if (token) void installApi.cancelSkillImport(token).catch(() => undefined);
  }, [executing]);

  useEffect(() => () => cancelPlan(), [cancelPlan]);

  const check = async () => {
    cancelPlan();
    setChecking(true);
    setError(undefined);
    setCompletedName(undefined);
    try {
      setItems(await updatesApi.check());
    } catch (failure) {
      setError(safeOperationError(failure));
    } finally {
      setChecking(false);
    }
  };

  const plan = async (item: SkillUpdateSummary) => {
    setPlanningId(item.skill_id);
    setError(undefined);
    try {
      const planned = await updatesApi.plan(item.skill_id);
      activeConfirmation.current = planned.confirmation_token?.token;
      setPlannedUpdate(planned);
    } catch (failure) {
      setError(safeOperationError(failure));
    } finally {
      setPlanningId(undefined);
    }
  };

  const execute = async () => {
    const token = activeConfirmation.current;
    if (!token) return;
    const name = plannedUpdate?.plan.impact.skill_name;
    activeConfirmation.current = undefined;
    setExecuting(true);
    setError(undefined);
    try {
      await updatesApi.execute(token);
      setPlannedUpdate(undefined);
      setCompletedName(name);
      setItems(await updatesApi.check());
    } catch (failure) {
      setPlannedUpdate(undefined);
      setError(safeOperationError(failure));
    } finally {
      setExecuting(false);
    }
  };

  return (
    <section className="updates-page" aria-labelledby="updates-title">
      <header className="updates-header">
        <div>
          <h1 id="updates-title">更新中心</h1>
          <p>受管 User Skill · 仅按需检查</p>
        </div>
        <button className="icon-text-button" type="button" disabled={checking || executing} onClick={() => void check()}>
          <RefreshCw size={16} aria-hidden="true" className={checking ? "is-spinning" : ""} />
          {checking ? "正在检查" : "检查更新"}
        </button>
      </header>

      {error ? (
        <div className="updates-notice is-error" role="alert">
          <AlertCircle size={17} aria-hidden="true" />
          <div><strong>{error.code}</strong><span>{error.recovery}</span></div>
        </div>
      ) : completedName ? (
        <div className="updates-notice is-success" role="status">
          <CheckCircle2 size={17} aria-hidden="true" />
          <div><strong>{completedName} 已更新</strong><span>Receipt 与 Catalog 已同步。</span></div>
        </div>
      ) : null}

      {!items ? (
        <div className="updates-empty">
          <GitCompareArrows size={28} aria-hidden="true" />
          <h2>尚未检查更新</h2>
          <p>点击“检查更新”后才会连接 GitHub 或官方市场。</p>
        </div>
      ) : items.length === 0 ? (
        <div className="updates-empty">
          <CircleDot size={28} aria-hidden="true" />
          <h2>没有可检查的安装记录</h2>
          <p>从 GitHub 或官方市场安装受管 Skill 后会显示在这里。</p>
        </div>
      ) : (
        <div className="updates-list" aria-live="polite">
          {items.map((item) => (
            <article className={`updates-item is-${item.status}`} key={item.skill_id}>
              <div className="updates-item-main">
                <div className="updates-item-heading">
                  <h2>{item.display_name}</h2>
                  <span>{statusCopy[item.status]}</span>
                </div>
                <p>{item.reason}</p>
                <div className="updates-meta">
                  <span>{item.source_type === "market" ? "官方市场" : "GitHub"}</span>
                  {item.installed_commit ? <code>当前 {item.installed_commit}</code> : null}
                  {item.available_commit ? <code>来源 {item.available_commit}</code> : null}
                </div>
                {item.changed_files.length ? (
                  <ul className="updates-files">
                    {item.changed_files.map((path) => <li key={path}>{path}</li>)}
                  </ul>
                ) : null}
              </div>
              {item.status === "available" ? (
                <button className="updates-action" type="button" disabled={Boolean(planningId) || executing} onClick={() => void plan(item)}>
                  {planningId === item.skill_id ? <LoaderCircle className="is-spinning" size={15} aria-hidden="true" /> : <GitCompareArrows size={15} aria-hidden="true" />}
                  {planningId === item.skill_id ? "准备中" : "查看计划"}
                </button>
              ) : item.status === "conflict" ? <ShieldAlert size={18} aria-label="已保留本地修改" /> : null}
            </article>
          ))}
        </div>
      )}

      {plannedUpdate ? (
        <OperationPlanModal plannedImport={plannedUpdate} busy={executing} onClose={cancelPlan} onConfirm={() => void execute()} />
      ) : null}
    </section>
  );
}
