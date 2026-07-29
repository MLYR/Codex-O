import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  AlertCircle,
  Archive,
  ArrowLeft,
  BrainCircuit,
  ChevronDown,
  ChevronUp,
  FileCode2,
  FileSearch,
  FileText,
  FolderTree,
  GitCompareArrows,
  LoaderCircle,
  RefreshCw,
  ShieldAlert,
  ShieldCheck,
} from "lucide-react";
import { useEffect, useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { skillCatalogApi } from "./api";
import { OperationDialog } from "./QuarantinePanel";
import { diagnosticLabel, formatBytes, formatUpdatedAt, scopeLabel } from "./format";
import type {
  AnalysisProgress,
  AnalysisRunStatus,
  AnalysisView,
  EvidenceExcerpt,
  SkillComparison,
  SkillDetail,
  SkillPassport,
  SkillSummary,
  PlannedOperation,
} from "./types";

const terminalAnalysisStatuses = new Set([
  "ready",
  "stale",
  "failed",
  "degraded",
  "not_configured",
]);

export function SkillDetailPage() {
  const { skillId } = useParams();
  const navigate = useNavigate();
  const [detail, setDetail] = useState<SkillDetail>();
  const [analysis, setAnalysis] = useState<AnalysisView>();
  const [skills, setSkills] = useState<SkillSummary[]>([]);
  const [compareId, setCompareId] = useState("");
  const [comparison, setComparison] = useState<SkillComparison>();
  const [evidence, setEvidence] = useState<EvidenceExcerpt>();
  const [error, setError] = useState<string>();
  const [analysisError, setAnalysisError] = useState<string>();
  const [isLoading, setIsLoading] = useState(true);
  const [isLoadingSource, setIsLoadingSource] = useState(false);
  const [isSourceExpanded, setIsSourceExpanded] = useState(false);
  const [busyAction, setBusyAction] = useState<string>();
  const [quarantinePlan, setQuarantinePlan] = useState<PlannedOperation>();
  const [quarantineAcknowledgement, setQuarantineAcknowledgement] = useState("");

  const refreshAnalysis = async (id: string) => {
    try {
      setAnalysis(await skillCatalogApi.getSkillAnalysis(id));
      setAnalysisError(undefined);
    } catch {
      setAnalysisError("无法读取缓存护照，静态详情仍可使用。");
    }
  };

  useEffect(() => {
    if (!skillId) {
      setError("请求的 Skill 不存在。");
      setIsLoading(false);
      return;
    }
    let active = true;
    setIsLoading(true);
    setError(undefined);
    void Promise.allSettled([
      skillCatalogApi.getSkillDetail(skillId),
      skillCatalogApi.getSkillAnalysis(skillId),
      skillCatalogApi.listSkills({ sort: "name" }),
    ]).then(([detailResult, analysisResult, listResult]) => {
      if (!active) {
        return;
      }
      if (detailResult.status === "rejected") {
        setError("请求的 Skill 不可用。");
      } else {
        setDetail(detailResult.value);
      }
      if (analysisResult.status === "fulfilled") {
        setAnalysis(analysisResult.value);
      } else {
        setAnalysisError("无法读取缓存护照，静态详情仍可使用。");
      }
      if (listResult.status === "fulfilled") {
        setSkills(listResult.value.skills);
      }
      setIsLoading(false);
    });
    return () => {
      active = false;
    };
  }, [skillId]);

  useEffect(() => {
    if (!skillId) {
      return;
    }
    let unlisten: UnlistenFn | undefined;
    void listen<AnalysisProgress>("analysis_progress", (event) => {
      const job = event.payload.jobs.find((item) => item.skill_id === skillId);
      if (!job) {
        return;
      }
      if (job.status === "queued" || job.status === "running") {
        setBusyAction("analysis");
        return;
      }
      if (terminalAnalysisStatuses.has(job.status)) {
        setBusyAction(undefined);
        void refreshAnalysis(skillId);
      }
    }).then((dispose) => {
      unlisten = dispose;
    });
    return () => {
      unlisten?.();
    };
  }, [skillId]);

  const loadSource = async () => {
    if (!skillId) {
      return;
    }
    if (detail?.source !== undefined) {
      setIsSourceExpanded((expanded) => !expanded);
      return;
    }
    setIsLoadingSource(true);
    try {
      const response = await skillCatalogApi.getSkillDetail(skillId, true);
      setDetail(response);
      setIsSourceExpanded(true);
    } catch {
      setAnalysisError("无法读取此 Skill 的原文。");
    } finally {
      setIsLoadingSource(false);
    }
  };

  const runAnalysis = async (force: boolean) => {
    if (!skillId) {
      return;
    }
    setBusyAction("analysis");
    setAnalysisError(undefined);
    try {
      const result = await skillCatalogApi.analyzeSkill(skillId, force);
      if (result.status === "not_configured") {
        setBusyAction(undefined);
        await refreshAnalysis(skillId);
      }
    } catch {
      setBusyAction(undefined);
      setAnalysisError("分析未启动，静态详情保持可用。");
    }
  };

  const openEvidence = async (evidenceId: string) => {
    setBusyAction(`evidence:${evidenceId}`);
    setAnalysisError(undefined);
    try {
      setEvidence(await skillCatalogApi.readEvidenceExcerpt(evidenceId));
    } catch {
      setAnalysisError("证据已失效或内容发生变化，请重新分析。");
    } finally {
      setBusyAction(undefined);
    }
  };

  const compare = async () => {
    if (!skillId || !compareId) {
      return;
    }
    setBusyAction("compare");
    setAnalysisError(undefined);
    try {
      setComparison(await skillCatalogApi.compareSkills([skillId, compareId]));
    } catch {
      setAnalysisError("无法比较这两个 Skill。");
    } finally {
      setBusyAction(undefined);
    }
  };

  const planQuarantine = async () => {
    if (!skillId) {
      return;
    }
    setBusyAction("quarantine");
    try {
      const planned = await skillCatalogApi.planQuarantine(skillId);
      setQuarantinePlan(planned);
      setQuarantineAcknowledgement("");
      setAnalysisError(undefined);
    } catch {
      setAnalysisError("quarantine_not_allowed");
    } finally {
      setBusyAction(undefined);
    }
  };

  const executeQuarantine = async () => {
    if (!quarantinePlan?.confirmation_token) {
      return;
    }
    setBusyAction("quarantine");
    try {
      await skillCatalogApi.executeQuarantine(
        quarantinePlan.confirmation_token.token,
        quarantineAcknowledgement || undefined,
      );
      navigate("/skills");
    } catch {
      setAnalysisError("operation_failed");
    } finally {
      setBusyAction(undefined);
    }
  };

  if (isLoading) {
    return <DetailState title="正在读取 Skill 详情" detail="仅加载安全的静态信息。" />;
  }

  if (error || !detail) {
    return (
      <DetailState
        title="无法打开 Skill"
        detail={error ?? "请求的 Skill 不存在。"}
        error={error ?? "not_found"}
        onBack={() => navigate("/skills")}
      />
    );
  }

  const { summary } = detail;
  const compareOptions = skills.filter((skill) => skill.id !== summary.id);
  return (
    <article className="skill-detail" aria-labelledby="skill-detail-title">
      <button className="back-button" type="button" onClick={() => navigate("/skills")}>
        <ArrowLeft size={16} aria-hidden="true" />
        返回列表
      </button>

      <header className="skill-detail-header">
        <div>
          <div className="detail-eyebrow">
            <span>{summary.provider.display_name}</span>
            <span>{scopeLabel(summary.scope)}</span>
            <span>{summary.validity === "valid" ? "有效" : "需关注"}</span>
          </div>
          <h1 id="skill-detail-title">{summary.display_name}</h1>
          <p>{summary.description || "未提供静态描述"}</p>
        </div>
        {summary.provider.capabilities.can_quarantine ? (
          <button className="danger-button" type="button" disabled={busyAction === "quarantine"} onClick={() => void planQuarantine()}>
            <Archive size={16} aria-hidden="true" />隔离
          </button>
        ) : <div className="detail-readonly"><ShieldCheck size={16} aria-hidden="true" />此来源只读</div>}
      </header>

      <section className="detail-facts" aria-label="Skill 静态信息">
        <div>
          <span>大小</span>
          <strong>{formatBytes(summary.size_bytes)}</strong>
        </div>
        <div>
          <span>更新时间</span>
          <strong>{formatUpdatedAt(summary.updated_at_ms)}</strong>
        </div>
        <div>
          <span>AI 分析</span>
          <strong>{analysisStatusLabel(analysis?.status)}</strong>
        </div>
        <div>
          <span>读取能力</span>
          <strong>{summary.provider.capabilities.can_read ? "可读取" : "不可读取"}</strong>
        </div>
      </section>

      <section className="detail-section analysis-section" aria-labelledby="analysis-title">
        <div className="detail-section-title detail-section-title-action">
          <BrainCircuit size={17} aria-hidden="true" />
          <h2 id="analysis-title">Skill 护照</h2>
          <button
            className="icon-text-button"
            type="button"
            disabled={busyAction === "analysis"}
            onClick={() => void runAnalysis(Boolean(analysis?.passport))}
          >
            {busyAction === "analysis" ? (
              <LoaderCircle className="is-spinning" size={16} aria-hidden="true" />
            ) : analysis?.passport ? (
              <RefreshCw size={16} aria-hidden="true" />
            ) : (
              <BrainCircuit size={16} aria-hidden="true" />
            )}
            {busyAction === "analysis"
              ? "分析中"
              : analysis?.passport
                ? "重新分析"
                : "分析"}
          </button>
        </div>
        {analysisError ? (
          <div className="analysis-message analysis-message-error" role="status">
            <AlertCircle size={15} aria-hidden="true" />
            {analysisError}
          </div>
        ) : null}
        {analysis?.passport ? (
          <PassportView
            analysis={analysis}
            evidence={evidence}
            busyAction={busyAction}
            onEvidence={openEvidence}
          />
        ) : (
          <AnalysisEmpty analysis={analysis} onSettings={() => navigate("/settings")} />
        )}
      </section>

      <section className="detail-section comparison-section" aria-labelledby="comparison-title">
        <div className="detail-section-title">
          <GitCompareArrows size={17} aria-hidden="true" />
          <h2 id="comparison-title">比较 Skill</h2>
        </div>
        <div className="comparison-controls">
          <select
            aria-label="选择对比 Skill"
            value={compareId}
            onChange={(event) => {
              setCompareId(event.target.value);
              setComparison(undefined);
            }}
          >
            <option value="">选择另一个 Skill</option>
            {compareOptions.map((skill) => (
              <option key={skill.id} value={skill.id}>
                {skill.display_name} · {skill.provider.display_name}
              </option>
            ))}
          </select>
          <button
            className="secondary-button"
            type="button"
            disabled={!compareId || busyAction === "compare"}
            onClick={() => void compare()}
          >
            {busyAction === "compare" ? (
              <LoaderCircle className="is-spinning" size={15} aria-hidden="true" />
            ) : (
              <GitCompareArrows size={15} aria-hidden="true" />
            )}
            比较
          </button>
        </div>
        {comparison ? <ComparisonView comparison={comparison} /> : null}
      </section>

      <section className="detail-section">
        <div className="detail-section-title">
          <FileText size={17} aria-hidden="true" />
          <h2>标题结构</h2>
        </div>
        {detail.headings.length ? (
          <ol className="heading-tree">
            {detail.headings.map((heading) => (
              <li
                key={`${heading.line_start}-${heading.text}`}
                style={{ paddingLeft: `${(heading.level - 1) * 16}px` }}
              >
                <span>H{heading.level}</span>
                <strong>{heading.text}</strong>
                <small>第 {heading.line_start} 行</small>
              </li>
            ))}
          </ol>
        ) : (
          <p className="detail-empty">未发现标题。</p>
        )}
      </section>

      <section className="detail-section">
        <div className="detail-section-title">
          <FolderTree size={17} aria-hidden="true" />
          <h2>资源树</h2>
        </div>
        {detail.resources.length ? (
          <ul className="resource-list">
            {detail.resources.map((resource) => (
              <li key={resource.relative_path}>
                <code>{resource.relative_path}</code>
                <span>{formatBytes(resource.size_bytes)}</span>
              </li>
            ))}
          </ul>
        ) : (
          <p className="detail-empty">没有受控资源文件。</p>
        )}
      </section>

      <section className="detail-section">
        <div className="detail-section-title">
          <AlertCircle size={17} aria-hidden="true" />
          <h2>诊断</h2>
        </div>
        {detail.diagnostics.length ? (
          <ul className="diagnostic-list">
            {detail.diagnostics.map((diagnostic, index) => (
              <li key={`${diagnostic.code}-${diagnostic.relative_path ?? index}`}>
                <strong>{diagnosticLabel(diagnostic.code)}</strong>
                {diagnostic.relative_path ? <code>{diagnostic.relative_path}</code> : null}
              </li>
            ))}
          </ul>
        ) : (
          <p className="detail-empty">没有诊断。</p>
        )}
      </section>

      <section className="detail-section source-section">
        <div className="detail-section-title">
          <FileCode2 size={17} aria-hidden="true" />
          <h2>SKILL.md 原文</h2>
        </div>
        <button
          className="icon-text-button"
          type="button"
          onClick={() => void loadSource()}
          disabled={isLoadingSource}
        >
          {isSourceExpanded ? (
            <ChevronUp size={16} aria-hidden="true" />
          ) : (
            <ChevronDown size={16} aria-hidden="true" />
          )}
          {isLoadingSource ? "正在读取" : isSourceExpanded ? "收起原文" : "查看原文"}
        </button>
        {isSourceExpanded && detail.source !== undefined ? (
          <pre className="skill-source" data-testid="skill-source">
            {detail.source}
          </pre>
        ) : null}
      </section>
      {quarantinePlan ? <OperationDialog planned={quarantinePlan} acknowledgement={quarantineAcknowledgement} onAcknowledgement={setQuarantineAcknowledgement} busy={busyAction === "quarantine"} onClose={() => setQuarantinePlan(undefined)} onConfirm={() => void executeQuarantine()} /> : null}
    </article>
  );
}

function PassportView({
  analysis,
  evidence,
  busyAction,
  onEvidence,
}: {
  analysis: AnalysisView;
  evidence?: EvidenceExcerpt;
  busyAction?: string;
  onEvidence: (evidenceId: string) => Promise<void>;
}) {
  const passport = analysis.passport as SkillPassport;
  const redactionTotal = Object.values(analysis.redactions).reduce((sum, value) => sum + value, 0);
  return (
    <div className="passport">
      <div className="analysis-meta">
        <span>{analysis.provider ?? "未知 Provider"}</span>
        <span>{analysis.model ?? "未知模型"}</span>
        <span>{formatUpdatedAt(analysis.analyzed_at_ms)}</span>
        <span>{analysis.cache_hit ? "缓存命中" : "新分析"}</span>
        {analysis.stale ? <span>已过期</span> : null}
        {analysis.degraded ? <span>降级</span> : null}
      </div>
      <p className="passport-summary">{passport.summary}</p>
      <div className="passport-grid">
        <PassportList title="能力" values={passport.capabilities} />
        <PassportList title="触发示例" values={passport.triggerExamples} />
        <PassportList title="适用场景" values={passport.suitableWhen} />
        <PassportList title="不适用场景" values={passport.avoidWhen} />
        <PassportList title="工作流" values={passport.workflow} ordered />
        <PassportList title="前置条件" values={passport.prerequisites} />
        <PassportList
          title="资源"
          values={passport.resources.map(
            (resource) => `${resource.relativePath} · ${resource.summary}`,
          )}
        />
        <PassportList title="副作用" values={passport.sideEffects} />
        <PassportList title="相关 Skill" values={passport.relatedHints} />
        <PassportList title="不确定性" values={passport.uncertainties} warning />
      </div>

      <div className="risk-panel">
        <div className="passport-subtitle">
          <ShieldAlert size={16} aria-hidden="true" />
          <h3>风险</h3>
          <span>置信度 {confidenceLabel(passport.confidence)}</span>
        </div>
        {passport.risks.length ? (
          <ul>
            {passport.risks.map((risk, index) => (
              <li key={`${risk.category}-${index}`} data-severity={risk.severity}>
                <b>{riskSeverityLabel(risk.severity)}</b>
                <strong>{risk.category}</strong>
                <span>{risk.description}</span>
              </li>
            ))}
          </ul>
        ) : (
          <p className="detail-empty">没有已确认的风险结论。</p>
        )}
      </div>

      <div className="analysis-transparency">
        <div>
          <strong>发送范围</strong>
          <span>{analysis.sent_sections.length} 个受控片段</span>
        </div>
        <div>
          <strong>脱敏</strong>
          <span>{redactionTotal} 处</span>
        </div>
        <ul>
          {analysis.sent_sections.map((section) => (
            <li key={section.id}>
              <code>{section.relative_path}</code>
              <span>
                {section.title} · {section.line_start}-{section.line_end} 行
              </span>
            </li>
          ))}
        </ul>
      </div>

      <div className="evidence-panel">
        <div className="passport-subtitle">
          <FileSearch size={16} aria-hidden="true" />
          <h3>证据</h3>
        </div>
        {analysis.evidence.length ? (
          <div className="evidence-links">
            {analysis.evidence.map((item) => (
              <button
                key={item.id}
                type="button"
                disabled={busyAction === `evidence:${item.id}`}
                onClick={() => void onEvidence(item.id)}
              >
                {busyAction === `evidence:${item.id}` ? (
                  <LoaderCircle className="is-spinning" size={14} aria-hidden="true" />
                ) : (
                  <FileSearch size={14} aria-hidden="true" />
                )}
                {item.relative_path} · {item.line_start}-{item.line_end} 行
              </button>
            ))}
          </div>
        ) : (
          <p className="detail-empty">此护照没有可验证证据。</p>
        )}
        {evidence ? (
          <div className="evidence-excerpt" data-testid="evidence-excerpt">
            <header>
              <code>{evidence.relative_path}</code>
              <span>
                {evidence.line_start}-{evidence.line_end} 行
              </span>
            </header>
            <ol start={evidence.line_start}>
              {evidence.lines.map((line) => (
                <li key={line.number}>
                  <code>{line.text || " "}</code>
                </li>
              ))}
            </ol>
          </div>
        ) : null}
      </div>
    </div>
  );
}

function PassportList({
  title,
  values,
  ordered,
  warning,
}: {
  title: string;
  values: string[];
  ordered?: boolean;
  warning?: boolean;
}) {
  const List = ordered ? "ol" : "ul";
  return (
    <section className={warning ? "passport-field passport-field-warning" : "passport-field"}>
      <h3>{title}</h3>
      {values.length ? (
        <List>
          {values.map((value, index) => (
            <li key={`${value}-${index}`}>{value}</li>
          ))}
        </List>
      ) : (
        <p>信息不足</p>
      )}
    </section>
  );
}

function AnalysisEmpty({
  analysis,
  onSettings,
}: {
  analysis?: AnalysisView;
  onSettings: () => void;
}) {
  const isUnconfigured = !analysis || analysis.status === "not_configured";
  return (
    <div className="analysis-empty">
      <BrainCircuit size={21} aria-hidden="true" />
      <div>
        <strong>{isUnconfigured ? "AI 尚未配置" : "尚未生成护照"}</strong>
        <span>
          {isUnconfigured
            ? "静态详情、资源树和原文仍可使用。"
            : "点击分析后生成可追溯护照。"}
        </span>
      </div>
      {isUnconfigured ? (
        <button className="secondary-button" type="button" onClick={onSettings}>
          打开设置
        </button>
      ) : null}
    </div>
  );
}

function ComparisonView({ comparison }: { comparison: SkillComparison }) {
  return (
    <div className="comparison-table" data-testid="skill-comparison">
      <div className="comparison-header">
        <span />
        <div>
          <strong>{comparison.left.display_name}</strong>
          <span>{comparison.left.provider}</span>
        </div>
        <div>
          <strong>{comparison.right.display_name}</strong>
          <span>{comparison.right.provider}</span>
        </div>
      </div>
      {comparison.rows.map((row) => (
        <div
          className={row.different ? "comparison-row comparison-row-different" : "comparison-row"}
          key={row.key}
        >
          <strong>{row.label}</strong>
          <ComparisonValues values={row.left} />
          <ComparisonValues values={row.right} />
        </div>
      ))}
    </div>
  );
}

function ComparisonValues({ values }: { values: string[] }) {
  return (
    <ul>
      {values.map((value, index) => (
        <li key={`${value}-${index}`}>{value}</li>
      ))}
    </ul>
  );
}

function DetailState({
  title,
  detail,
  error,
  onBack,
}: {
  title: string;
  detail: string;
  error?: string;
  onBack?: () => void;
}) {
  return (
    <div
      className={`skills-state${error ? " skills-state-error" : ""}`}
      role={error ? "alert" : undefined}
    >
      {error ? (
        <AlertCircle size={22} aria-hidden="true" />
      ) : (
        <FileText size={22} aria-hidden="true" />
      )}
      <h1>{title}</h1>
      <p>{detail}</p>
      {onBack ? (
        <button className="icon-text-button" type="button" onClick={onBack}>
          <ArrowLeft size={16} aria-hidden="true" />
          返回列表
        </button>
      ) : null}
    </div>
  );
}

function analysisStatusLabel(status?: AnalysisRunStatus) {
  const labels: Record<AnalysisRunStatus, string> = {
    not_requested: "未分析",
    not_configured: "未配置",
    ready: "已就绪",
    stale: "已过期",
    failed: "失败",
    degraded: "降级",
  };
  return status ? labels[status] : "不可用";
}

function confidenceLabel(confidence: SkillPassport["confidence"]) {
  return { high: "高", medium: "中", low: "低" }[confidence];
}

function riskSeverityLabel(severity: "low" | "medium" | "high") {
  return { low: "低", medium: "中", high: "高" }[severity];
}
