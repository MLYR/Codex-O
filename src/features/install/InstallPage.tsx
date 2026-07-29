import { useState } from "react";
import {
  AlertCircle,
  CheckCircle2,
  FileUp,
  FolderUp,
  LoaderCircle,
  ShieldCheck,
} from "lucide-react";
import {
  installApi,
  safeOperationError,
  type ImportSourceKind,
  type OperationError,
  type OperationResult,
  type PlannedImport,
} from "./api";
import { OperationPlanModal } from "./OperationPlanModal";

export function InstallPage() {
  const [busy, setBusy] = useState<"select" | "execute">();
  const [plannedImport, setPlannedImport] = useState<PlannedImport>();
  const [result, setResult] = useState<OperationResult>();
  const [error, setError] = useState<OperationError>();

  const selectAndPlan = async (kind: ImportSourceKind) => {
    setBusy("select");
    setError(undefined);
    setResult(undefined);
    try {
      const selection = await installApi.selectImportSource(kind);
      setPlannedImport(await installApi.planSkillImport(selection.token));
    } catch (failure) {
      setError(safeOperationError(failure));
    } finally {
      setBusy(undefined);
    }
  };

  const executeImport = async () => {
    const confirmation = plannedImport?.confirmation_token;
    if (!confirmation) {
      return;
    }
    setBusy("execute");
    setError(undefined);
    try {
      const completed = await installApi.executeSkillImport(confirmation.token);
      setResult(completed);
      setPlannedImport(undefined);
    } catch (failure) {
      setError(safeOperationError(failure));
      setPlannedImport(undefined);
    } finally {
      setBusy(undefined);
    }
  };

  return (
    <section className="install-page" aria-labelledby="install-title">
      <header className="install-header">
        <div>
          <h1 id="install-title">本地导入</h1>
          <p>选择一个 Skill，检查计划后写入受管 User Provider</p>
        </div>
        <span className="install-safety-note">
          <ShieldCheck size={16} aria-hidden="true" />
          不覆盖现有 Skill
        </span>
      </header>

      <section className="install-source-section" aria-labelledby="install-source-title">
        <div className="install-section-heading">
          <h2 id="install-source-title">选择来源</h2>
          <p>文件选择仅接受 SKILL.md；目录选择必须指向单个 Skill 根目录。</p>
        </div>
        <div className="install-source-actions">
          <button
            type="button"
            className="install-source-button"
            disabled={busy !== undefined}
            onClick={() => void selectAndPlan("file")}
          >
            <FileUp size={20} aria-hidden="true" />
            <span>
              <strong>选择 SKILL.md</strong>
              <small>导入文件所在的完整 Skill 目录</small>
            </span>
          </button>
          <button
            type="button"
            className="install-source-button"
            disabled={busy !== undefined}
            onClick={() => void selectAndPlan("directory")}
          >
            <FolderUp size={20} aria-hidden="true" />
            <span>
              <strong>选择 Skill 目录</strong>
              <small>校验目录中的清单、资源与路径</small>
            </span>
          </button>
        </div>
        {busy === "select" ? (
          <div className="install-feedback" aria-live="polite">
            <LoaderCircle className="is-spinning" size={16} aria-hidden="true" />
            正在检查本地 Skill 并生成计划
          </div>
        ) : null}
      </section>

      {error ? (
        <section className="install-result is-error" role="alert" aria-labelledby="install-error-title">
          <AlertCircle size={20} aria-hidden="true" />
          <div>
            <h2 id="install-error-title">{error.message}</h2>
            <code>{error.code}</code>
            <p>{error.recovery}</p>
          </div>
        </section>
      ) : result ? (
        <section className="install-result is-success" aria-labelledby="install-success-title">
          <CheckCircle2 size={20} aria-hidden="true" />
          <div>
            <h2 id="install-success-title">Skill 已导入</h2>
            <p>Catalog 已刷新。AI 分析仍需在详情页明确发起。</p>
          </div>
          <button className="secondary-button" type="button" onClick={() => setResult(undefined)}>
            继续导入
          </button>
        </section>
      ) : null}

      {plannedImport ? (
        <OperationPlanModal
          plannedImport={plannedImport}
          busy={busy === "execute"}
          onClose={() => setPlannedImport(undefined)}
          onConfirm={() => void executeImport()}
        />
      ) : null}
    </section>
  );
}
