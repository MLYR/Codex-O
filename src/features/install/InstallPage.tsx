import { useEffect, useRef, useState } from "react";
import {
  AlertCircle,
  CheckCircle2,
  FileUp,
  FolderUp,
  GitBranch,
  HardDrive,
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
import "./InstallPage.css";

type InstallMode = "local" | "github";

export function InstallPage() {
  const [mode, setMode] = useState<InstallMode>("local");
  const [busy, setBusy] = useState<"select" | "github" | "execute">();
  const [repositoryUrl, setRepositoryUrl] = useState("");
  const [repoRef, setRepoRef] = useState("main");
  const [subdirectory, setSubdirectory] = useState("");
  const [plannedImport, setPlannedImport] = useState<PlannedImport>();
  const [result, setResult] = useState<OperationResult>();
  const [error, setError] = useState<OperationError>();
  const pendingConfirmationRef = useRef<string | undefined>(undefined);

  useEffect(
    () => () => {
      const confirmationToken = pendingConfirmationRef.current;
      pendingConfirmationRef.current = undefined;
      // Route changes must release backend-owned GitHub staging even though this UI is gone.
      if (confirmationToken) {
        void installApi.cancelSkillImport(confirmationToken);
      }
    },
    [],
  );

  const showPlannedImport = (plan: PlannedImport) => {
    pendingConfirmationRef.current = plan.confirmation_token?.token;
    setPlannedImport(plan);
  };

  const selectAndPlan = async (kind: ImportSourceKind) => {
    setBusy("select");
    setError(undefined);
    setResult(undefined);
    try {
      const selection = await installApi.selectImportSource(kind);
      showPlannedImport(await installApi.planSkillImport(selection.token));
    } catch (failure) {
      setError(safeOperationError(failure));
    } finally {
      setBusy(undefined);
    }
  };

  const planGithubImport = async () => {
    setBusy("github");
    setError(undefined);
    setResult(undefined);
    try {
      showPlannedImport(
        await installApi.planGithubImport(
          repositoryUrl.trim(),
          repoRef.trim(),
          subdirectory.trim(),
        ),
      );
    } catch (failure) {
      setError(safeOperationError(failure));
    } finally {
      setBusy(undefined);
    }
  };

  const cancelPlannedImport = async () => {
    const confirmationToken = pendingConfirmationRef.current;
    pendingConfirmationRef.current = undefined;
    setPlannedImport(undefined);
    if (!confirmationToken) {
      return;
    }
    try {
      await installApi.cancelSkillImport(confirmationToken);
    } catch (failure) {
      setError(safeOperationError(failure));
    }
  };

  const selectMode = (nextMode: InstallMode) => {
    setMode(nextMode);
    setError(undefined);
    setResult(undefined);
    void cancelPlannedImport();
  };

  const executeImport = async () => {
    const confirmation = plannedImport?.confirmation_token;
    if (!confirmation) {
      return;
    }
    pendingConfirmationRef.current = undefined;
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
          <h1 id="install-title">{mode === "local" ? "本地导入" : "GitHub 安装"}</h1>
          <p>检查来源和安装计划后写入受管 User Provider</p>
        </div>
        <span className="install-safety-note">
          <ShieldCheck size={16} aria-hidden="true" />
          不覆盖现有 Skill
        </span>
      </header>

      <div className="install-mode-switch" aria-label="安装来源">
        <button
          type="button"
          aria-pressed={mode === "local"}
          onClick={() => selectMode("local")}
        >
          <HardDrive size={16} aria-hidden="true" />
          本地
        </button>
        <button
          type="button"
          aria-pressed={mode === "github"}
          onClick={() => selectMode("github")}
        >
          <GitBranch size={16} aria-hidden="true" />
          GitHub
        </button>
      </div>

      <section className="install-source-section" aria-labelledby="install-source-title">
        <div className="install-section-heading">
          <h2 id="install-source-title">选择来源</h2>
          <p>
            {mode === "local"
              ? "文件选择仅接受 SKILL.md；目录选择必须指向单个 Skill 根目录。"
              : "仅支持公开 GitHub 仓库，分支或标签会解析为固定 commit。"}
          </p>
        </div>
        {mode === "local" ? (
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
        ) : (
          <form
            className="github-install-form"
            onSubmit={(event) => {
              event.preventDefault();
              void planGithubImport();
            }}
          >
            <label className="github-field is-wide">
              <span>仓库 URL</span>
              <input
                type="url"
                required
                placeholder="https://github.com/owner/repository"
                value={repositoryUrl}
                disabled={busy !== undefined}
                onChange={(event) => setRepositoryUrl(event.target.value)}
              />
            </label>
            <label className="github-field">
              <span>Ref</span>
              <input
                type="text"
                required
                value={repoRef}
                disabled={busy !== undefined}
                onChange={(event) => setRepoRef(event.target.value)}
              />
            </label>
            <label className="github-field">
              <span>子目录</span>
              <input
                type="text"
                placeholder="skills/example"
                value={subdirectory}
                disabled={busy !== undefined}
                onChange={(event) => setSubdirectory(event.target.value)}
              />
            </label>
            <button
              className="primary-button github-plan-button"
              type="submit"
              disabled={busy !== undefined || !repositoryUrl.trim() || !repoRef.trim()}
            >
              {busy === "github" ? (
                <LoaderCircle className="is-spinning" size={15} aria-hidden="true" />
              ) : (
                <GitBranch size={15} aria-hidden="true" />
              )}
              {busy === "github" ? "正在检查" : "检查安装计划"}
            </button>
          </form>
        )}
        {busy === "select" || busy === "github" ? (
          <div className="install-feedback" aria-live="polite">
            <LoaderCircle className="is-spinning" size={16} aria-hidden="true" />
            {busy === "github" ? "正在固定版本并检查仓库内容" : "正在检查本地 Skill 并生成计划"}
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
            <h2 id="install-success-title">Skill 已安装</h2>
            <p>Catalog 已刷新。AI 分析仍需在详情页明确发起。</p>
          </div>
          <button className="secondary-button" type="button" onClick={() => setResult(undefined)}>
            继续安装
          </button>
        </section>
      ) : null}

      {plannedImport ? (
        <OperationPlanModal
          plannedImport={plannedImport}
          busy={busy === "execute"}
          onClose={() => void cancelPlannedImport()}
          onConfirm={() => void executeImport()}
        />
      ) : null}
    </section>
  );
}
