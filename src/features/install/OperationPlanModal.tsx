import { useEffect } from "react";
import { AlertTriangle, LoaderCircle, ShieldCheck, X } from "lucide-react";
import type { PlannedImport } from "./api";

interface OperationPlanModalProps {
  plannedImport: PlannedImport;
  busy: boolean;
  onClose: () => void;
  onConfirm: () => void;
}

export function OperationPlanModal({
  plannedImport,
  busy,
  onClose,
  onConfirm,
}: OperationPlanModalProps) {
  const conflict = plannedImport.plan.status === "conflict";
  const impact = plannedImport.plan.impact;

  useEffect(() => {
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape" && !busy) {
        onClose();
      }
    };
    window.addEventListener("keydown", closeOnEscape);
    return () => window.removeEventListener("keydown", closeOnEscape);
  }, [busy, onClose]);

  return (
    <div className="dialog-backdrop" role="presentation">
      <section
        className="operation-plan-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="operation-plan-title"
        aria-describedby="operation-plan-description"
      >
        <header className="operation-plan-header">
          <span className={conflict ? "operation-plan-icon is-conflict" : "operation-plan-icon"}>
            {conflict ? (
              <AlertTriangle size={20} aria-hidden="true" />
            ) : (
              <ShieldCheck size={20} aria-hidden="true" />
            )}
          </span>
          <div>
            <h2 id="operation-plan-title">导入计划</h2>
            <p id="operation-plan-description">
              {conflict ? "目标已存在，计划不能执行。" : "确认后才会写入受管 User Provider。"}
            </p>
          </div>
          <button
            className="icon-only-button"
            type="button"
            aria-label="关闭导入计划"
            title="关闭"
            disabled={busy}
            onClick={onClose}
          >
            <X size={16} aria-hidden="true" />
          </button>
        </header>

        <dl className="operation-impact-list">
          <div>
            <dt>Skill</dt>
            <dd>{impact.skill_name}</dd>
          </div>
          <div>
            <dt>目标来源</dt>
            <dd>User · 受管</dd>
          </div>
          <div>
            <dt>文件</dt>
            <dd>{impact.file_count} 个</dd>
          </div>
          <div>
            <dt>总大小</dt>
            <dd>{formatBytes(impact.total_size_bytes)}</dd>
          </div>
          {plannedImport.plan.source ? (
            <>
              <div className="is-wide">
                <dt>{plannedImport.plan.source.source_type === "market" ? "官方市场来源" : "GitHub 仓库"}</dt>
                <dd>{plannedImport.plan.source.repository_url}</dd>
              </div>
              <div>
                <dt>Ref</dt>
                <dd>{plannedImport.plan.source.repo_ref}</dd>
              </div>
              <div>
                <dt>Commit</dt>
                <dd className="operation-source-sha">{plannedImport.plan.source.commit_sha}</dd>
              </div>
              <div className="is-wide">
                <dt>子目录</dt>
                <dd>{plannedImport.plan.source.subdirectory || "仓库内唯一 Skill"}</dd>
              </div>
            </>
          ) : null}
        </dl>

        {conflict ? (
          <p className="operation-conflict" role="alert">
            <strong>conflict_detected</strong>
            同名 Skill 不会被覆盖或自动改名。请关闭计划并调整来源名称。
          </p>
        ) : null}

        <footer className="operation-plan-actions">
          <button className="secondary-button" type="button" disabled={busy} onClick={onClose}>
            取消
          </button>
          {!conflict ? (
            <button
              className="primary-button"
              type="button"
              autoFocus
              disabled={busy || !plannedImport.confirmation_token}
              onClick={onConfirm}
            >
              {busy ? <LoaderCircle className="is-spinning" size={15} aria-hidden="true" /> : null}
              {busy ? "正在导入" : "确认导入"}
            </button>
          ) : null}
        </footer>
      </section>
    </div>
  );
}

function formatBytes(bytes: number) {
  if (bytes < 1024) {
    return `${bytes} B`;
  }
  if (bytes < 1024 * 1024) {
    return `${(bytes / 1024).toFixed(1)} KiB`;
  }
  return `${(bytes / (1024 * 1024)).toFixed(1)} MiB`;
}
