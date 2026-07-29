import { AlertCircle, ArchiveRestore, LoaderCircle, Trash2 } from "lucide-react";
import { useEffect, useState } from "react";
import { skillCatalogApi } from "./api";
import { formatBytes, formatUpdatedAt } from "./format";
import type { PlannedOperation, QuarantineEntry } from "./types";

export function QuarantinePanel() {
  const [entries, setEntries] = useState<QuarantineEntry[]>();
  const [planned, setPlanned] = useState<PlannedOperation>();
  const [acknowledgement, setAcknowledgement] = useState("");
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(false);

  const refresh = async () => {
    setBusy(true);
    try {
      setEntries(await skillCatalogApi.listQuarantineEntries());
      setError(undefined);
    } catch {
      setError("quarantine_unavailable");
    } finally {
      setBusy(false);
    }
  };

  useEffect(() => {
    void refresh();
  }, []);

  const plan = async (entry: QuarantineEntry, action: "restore" | "purge") => {
    setBusy(true);
    try {
      const next = action === "restore"
        ? await skillCatalogApi.planRestore(entry.id)
        : await skillCatalogApi.planPurge(entry.id);
      if (next.plan.status === "conflict") {
        setError("conflict_detected");
        return;
      }
      setPlanned(next);
      setAcknowledgement("");
      setError(undefined);
    } catch {
      setError(action === "restore" ? "quarantine_content_changed" : "quarantine_partial");
    } finally {
      setBusy(false);
    }
  };

  const execute = async () => {
    if (!planned?.confirmation_token) {
      return;
    }
    setBusy(true);
    try {
      if (planned.plan.operation === "skill_restore") {
        await skillCatalogApi.executeRestore(planned.confirmation_token.token);
      } else {
        await skillCatalogApi.executePurge(planned.confirmation_token.token, acknowledgement);
      }
      setPlanned(undefined);
      await refresh();
    } catch {
      setError("operation_failed");
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="quarantine-panel" aria-labelledby="quarantine-title">
      <header className="skills-page-header">
        <div>
          <h1 id="quarantine-title">隔离区</h1>
          <p>仅保留可恢复的隔离 Skill，不显示本机路径。</p>
        </div>
        <button className="icon-text-button" type="button" disabled={busy} onClick={() => void refresh()}>
          {busy ? <LoaderCircle className="is-spinning" size={16} aria-hidden="true" /> : <ArchiveRestore size={16} aria-hidden="true" />}
          刷新
        </button>
      </header>
      {error ? <p className="scan-message scan-message-error" role="alert"><AlertCircle size={15} aria-hidden="true" />{error}</p> : null}
      {entries === undefined ? <p className="detail-empty">正在读取隔离区。</p> : entries.length === 0 ? (
        <div className="skills-state"><ArchiveRestore size={22} aria-hidden="true" /><h2>隔离区为空</h2><p>隔离后的 Skill 会显示在这里。</p></div>
      ) : (
        <ul className="quarantine-list">
          {entries.map((entry) => (
            <li key={entry.id}>
              <div>
                <strong>{entry.display_name}</strong>
                <span>{entry.status === "partial" ? "需处理" : entry.status === "restored" ? "已恢复" : "已隔离"}</span>
                <small>{entry.file_count} 个文件 · {formatBytes(entry.total_size_bytes)} · {formatUpdatedAt(entry.quarantined_at)}</small>
              </div>
              {entry.status === "quarantined" ? <div className="quarantine-actions">
                <button className="icon-button" type="button" aria-label={`恢复 ${entry.display_name}`} title="恢复" disabled={busy} onClick={() => void plan(entry, "restore")}><ArchiveRestore size={16} aria-hidden="true" /></button>
                <button className="icon-button danger-button" type="button" aria-label={`永久清理 ${entry.display_name}`} title="永久清理" disabled={busy} onClick={() => void plan(entry, "purge")}><Trash2 size={16} aria-hidden="true" /></button>
              </div> : null}
            </li>
          ))}
        </ul>
      )}
      {planned ? <OperationDialog planned={planned} acknowledgement={acknowledgement} onAcknowledgement={setAcknowledgement} busy={busy} onClose={() => setPlanned(undefined)} onConfirm={() => void execute()} /> : null}
    </section>
  );
}

export function OperationDialog({ planned, acknowledgement, onAcknowledgement, busy, onClose, onConfirm }: {
  planned: PlannedOperation;
  acknowledgement: string;
  onAcknowledgement: (value: string) => void;
  busy: boolean;
  onClose: () => void;
  onConfirm: () => void;
}) {
  const { plan } = planned;
  const needsAck = plan.impact.requires_acknowledgement;
  return <div className="dialog-backdrop"><div className="scan-dialog operation-dialog" role="dialog" aria-modal="true" aria-labelledby="operation-title">
    <h2 id="operation-title">{plan.operation === "quarantine_purge" ? "永久清理" : plan.operation === "skill_restore" ? "恢复 Skill" : "隔离 Skill"}</h2>
    <p>{plan.impact.skill_name} · {plan.impact.file_count} 个文件 · {formatBytes(plan.impact.total_size_bytes)}</p>
    <ul className="operation-file-list">{plan.impact.relative_files.map((file) => <li key={file}><code>{file}</code></li>)}</ul>
    {needsAck ? <label className="acknowledgement-field">输入 “{plan.impact.skill_name}” 继续<input autoFocus value={acknowledgement} onChange={(event) => onAcknowledgement(event.target.value)} /></label> : null}
    <div className="scan-dialog-actions"><button className="secondary-button" type="button" autoFocus={!needsAck} disabled={busy} onClick={onClose}>取消</button><button className="danger-button" type="button" disabled={busy || (needsAck && acknowledgement.trim() !== plan.impact.skill_name)} onClick={onConfirm}>{busy ? "处理中" : "确认"}</button></div>
  </div></div>;
}
