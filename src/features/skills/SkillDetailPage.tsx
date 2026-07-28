import {
  AlertCircle,
  ArrowLeft,
  ChevronDown,
  ChevronUp,
  FileCode2,
  FileText,
  FolderTree,
  ShieldCheck,
} from "lucide-react";
import { useEffect, useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { skillCatalogApi } from "./api";
import { diagnosticLabel, formatBytes, formatUpdatedAt, scopeLabel } from "./format";
import type { SkillDetail } from "./types";

export function SkillDetailPage() {
  const { skillId } = useParams();
  const navigate = useNavigate();
  const [detail, setDetail] = useState<SkillDetail>();
  const [error, setError] = useState<string>();
  const [isLoading, setIsLoading] = useState(true);
  const [isLoadingSource, setIsLoadingSource] = useState(false);
  const [isSourceExpanded, setIsSourceExpanded] = useState(false);

  useEffect(() => {
    if (!skillId) {
      setError("请求的 Skill 不存在。");
      setIsLoading(false);
      return;
    }
    let active = true;
    setIsLoading(true);
    setError(undefined);
    void skillCatalogApi
      .getSkillDetail(skillId)
      .then((response) => {
        if (active) {
          setDetail(response);
        }
      })
      .catch(() => {
        if (active) {
          setError("请求的 Skill 不可用。");
        }
      })
      .finally(() => {
        if (active) {
          setIsLoading(false);
        }
      });
    return () => {
      active = false;
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
      setError("无法读取此 Skill 的原文。");
    } finally {
      setIsLoadingSource(false);
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
        <div className="detail-readonly">
          <ShieldCheck size={16} aria-hidden="true" />
          只读
        </div>
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
          <strong>未配置</strong>
        </div>
        <div>
          <span>读取能力</span>
          <strong>{summary.provider.capabilities.can_read ? "可读取" : "不可读取"}</strong>
        </div>
      </section>

      <section className="detail-section">
        <div className="detail-section-title">
          <FileText size={17} aria-hidden="true" />
          <h2>标题结构</h2>
        </div>
        {detail.headings.length ? (
          <ol className="heading-tree">
            {detail.headings.map((heading) => (
              <li key={`${heading.line_start}-${heading.text}`} style={{ paddingLeft: `${(heading.level - 1) * 16}px` }}>
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
          {isSourceExpanded ? <ChevronUp size={16} aria-hidden="true" /> : <ChevronDown size={16} aria-hidden="true" />}
          {isLoadingSource ? "正在读取" : isSourceExpanded ? "收起原文" : "查看原文"}
        </button>
        {isSourceExpanded && detail.source !== undefined ? (
          <pre className="skill-source" data-testid="skill-source">{detail.source}</pre>
        ) : null}
      </section>
    </article>
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
    <div className={`skills-state${error ? " skills-state-error" : ""}`} role={error ? "alert" : undefined}>
      {error ? <AlertCircle size={22} aria-hidden="true" /> : <FileText size={22} aria-hidden="true" />}
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
