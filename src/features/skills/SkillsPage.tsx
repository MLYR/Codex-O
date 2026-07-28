import { AlertCircle, FileSearch, RefreshCw, ScanSearch, Search } from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";
import { Link } from "react-router-dom";
import { settingsApi } from "../settings/api";
import { skillCatalogApi } from "./api";
import { formatBytes, formatUpdatedAt, scopeLabel } from "./format";
import type {
  CatalogScan,
  SkillListQuery,
  SkillScope,
  SkillSort,
  SkillSummary,
  SkillValidity,
} from "./types";

const initialQuery: SkillListQuery = { sort: "name" };

function hasFilters(query: SkillListQuery): boolean {
  return Boolean(query.query || query.providerId || query.scope || query.validity);
}

export function SkillsPage() {
  const [query, setQuery] = useState<SkillListQuery>(initialQuery);
  const [catalog, setCatalog] = useState<CatalogScan | null>();
  const [error, setError] = useState<string>();
  const [isLoading, setIsLoading] = useState(true);
  const [isScanning, setIsScanning] = useState(false);
  const [showInitialNotice, setShowInitialNotice] = useState(false);

  useEffect(() => {
    let active = true;
    void Promise.allSettled([
      skillCatalogApi.loadCatalog(),
      settingsApi.getScanPreferences(),
    ]).then(([catalogResult, preferencesResult]) => {
      if (!active) {
        return;
      }
      const cachedCatalog =
        catalogResult.status === "fulfilled" ? catalogResult.value : null;
      setCatalog(cachedCatalog);
      if (catalogResult.status === "rejected") {
        setError("无法读取上次的 Skill 索引。");
      }
      if (
        !cachedCatalog
        && preferencesResult.status === "fulfilled"
        && !preferencesResult.value.initial_scan_notice_seen
      ) {
        setShowInitialNotice(true);
      }
      setIsLoading(false);
    });
    return () => {
      active = false;
    };
  }, []);

  const skills = useMemo(
    () => filterSkills(catalog?.skills ?? [], query),
    [catalog, query],
  );

  const updateQuery = <Key extends keyof SkillListQuery>(
    key: Key,
    value: SkillListQuery[Key],
  ) => {
    setQuery((current) => ({ ...current, [key]: value || undefined }));
  };

  const scan = useCallback(async () => {
    setIsScanning(true);
    setError(undefined);
    try {
      setCatalog(await skillCatalogApi.scanSkills());
    } catch {
      setError("扫描未完成，现有列表没有被修改。");
    } finally {
      setIsScanning(false);
    }
  }, []);

  const dismissInitialNotice = async () => {
    setShowInitialNotice(false);
    try {
      await settingsApi.acknowledgeInitialScanNotice();
    } catch {
      setError("首次扫描提示状态未保存。");
    }
  };

  const startInitialScan = async () => {
    await dismissInitialNotice();
    await scan();
  };

  return (
    <section className="skills-page" aria-labelledby="skills-title">
      <header className="skills-page-header">
        <div>
          <h1 id="skills-title">我的 Skills</h1>
          <p>来自本地受控来源的只读清单</p>
        </div>
        <button
          className="icon-text-button"
          type="button"
          onClick={() => void scan()}
          disabled={isScanning}
        >
          <RefreshCw size={16} aria-hidden="true" className={isScanning ? "is-spinning" : ""} />
          {catalog ? "重新扫描" : "扫描 Skills"}
        </button>
      </header>

      {catalog ? (
        <div className="skill-filters" aria-label="Skill 筛选">
          <label className="search-field">
            <Search size={16} aria-hidden="true" />
            <input
              aria-label="搜索 Skill"
              value={query.query ?? ""}
              onChange={(event) => updateQuery("query", event.target.value)}
              placeholder="搜索名称或描述"
            />
          </label>
          <label>
            <span>来源</span>
            <select
              aria-label="来源筛选"
              value={query.providerId ?? ""}
              onChange={(event) => updateQuery("providerId", event.target.value)}
            >
              <option value="">全部来源</option>
              {catalog.providers.map((provider) => (
                <option key={provider.id} value={provider.id}>
                  {provider.display_name}
                </option>
              ))}
            </select>
          </label>
          <label>
            <span>作用域</span>
            <select
              aria-label="作用域筛选"
              value={query.scope ?? ""}
              onChange={(event) => updateQuery("scope", event.target.value as SkillScope)}
            >
              <option value="">全部作用域</option>
              <option value="user">用户</option>
              <option value="repository">仓库</option>
              <option value="legacy_user">旧用户目录</option>
              <option value="plugin">插件</option>
              <option value="bundled">内置</option>
              <option value="additional">附加目录</option>
            </select>
          </label>
          <label>
            <span>状态</span>
            <select
              aria-label="状态筛选"
              value={query.validity ?? ""}
              onChange={(event) => updateQuery("validity", event.target.value as SkillValidity)}
            >
              <option value="">全部状态</option>
              <option value="valid">有效</option>
              <option value="needs_attention">需关注</option>
            </select>
          </label>
          <label>
            <span>排序</span>
            <select
              aria-label="排序"
              value={query.sort}
              onChange={(event) => updateQuery("sort", event.target.value as SkillSort)}
            >
              <option value="name">名称</option>
              <option value="updated">更新时间</option>
              <option value="size">大小</option>
            </select>
          </label>
        </div>
      ) : null}

      {error && catalog ? (
        <div className="scan-message scan-message-error" role="alert">
          <AlertCircle size={15} aria-hidden="true" />
          {error}
        </div>
      ) : null}

      {isLoading ? (
        <StatePanel title="正在读取 Skill 索引" detail="不会自动扫描本地目录。" />
      ) : isScanning && !catalog ? (
        <StatePanel title="正在后台扫描 Skills" detail="扫描完成后列表会自动更新。" />
      ) : error && !catalog ? (
        <StatePanel
          title="无法读取 Skills"
          detail={error}
          kind="error"
          actionLabel="重新扫描"
          onAction={() => void scan()}
        />
      ) : !catalog ? (
        <StatePanel
          title="尚未扫描 Skills"
          detail="扫描仅在你主动开始后执行。"
          actionLabel="开始扫描"
          onAction={() => void scan()}
        />
      ) : catalog && skills.length === 0 ? (
        <StatePanel
          title={hasFilters(query) ? "没有匹配的 Skill" : "尚未发现 Skill"}
          detail={hasFilters(query) ? "请调整搜索或筛选条件。" : "当前受控来源中没有可浏览的 Skill。"}
        />
      ) : (
        <>
          <p className="result-count" aria-live="polite">
            {skills.length} 个 Skill
          </p>
          <div className="skill-grid">
            {skills.map((skill) => <SkillCard key={skill.id} skill={skill} />)}
          </div>
        </>
      )}

      {showInitialNotice ? (
        <div className="dialog-backdrop">
          <div
            className="scan-dialog"
            role="dialog"
            aria-modal="true"
            aria-labelledby="initial-scan-title"
          >
            <span className="scan-dialog-icon" aria-hidden="true">
              <ScanSearch size={22} />
            </span>
            <h2 id="initial-scan-title">扫描本地 Skills</h2>
            <p>尚未建立 Skill 索引。扫描将在后台执行，期间可以继续使用其他页面。</p>
            <div className="scan-dialog-actions">
              <button type="button" className="secondary-button" onClick={() => void dismissInitialNotice()}>
                暂不扫描
              </button>
              <button type="button" className="primary-button" onClick={() => void startInitialScan()}>
                开始扫描
              </button>
            </div>
          </div>
        </div>
      ) : null}
    </section>
  );
}

function filterSkills(skills: SkillSummary[], query: SkillListQuery): SkillSummary[] {
  const search = query.query?.trim().toLocaleLowerCase() ?? "";

  return skills
    .filter((skill) => {
      const matchesSearch = !search
        || skill.display_name.toLocaleLowerCase().includes(search)
        || skill.description?.toLocaleLowerCase().includes(search);

      return matchesSearch
        && (!query.providerId || skill.provider.id === query.providerId)
        && (!query.scope || skill.scope === query.scope)
        && (!query.validity || skill.validity === query.validity);
    })
    .sort((left, right) => {
      if (query.sort === "updated") {
        return (right.updated_at_ms ?? 0) - (left.updated_at_ms ?? 0)
          || left.display_name.localeCompare(right.display_name)
          || left.id.localeCompare(right.id);
      }
      if (query.sort === "size") {
        return right.size_bytes - left.size_bytes
          || left.display_name.localeCompare(right.display_name)
          || left.id.localeCompare(right.id);
      }
      return left.display_name.localeCompare(right.display_name) || left.id.localeCompare(right.id);
    });
}

function SkillCard({ skill }: { skill: SkillSummary }) {
  return (
    <Link className="skill-card" to={`/skills/${encodeURIComponent(skill.id)}`}>
      <div className="skill-card-heading">
        <span className="skill-card-icon" aria-hidden="true">
          <FileSearch size={18} />
        </span>
        <div>
          <h2>{skill.display_name}</h2>
          <p>{skill.description || "未提供静态描述"}</p>
        </div>
      </div>
      <div className="skill-card-meta">
        <span>{skill.provider.display_name}</span>
        <span>{scopeLabel(skill.scope)}</span>
        <span>{skill.validity === "valid" ? "有效" : "需关注"}</span>
      </div>
      <footer>
        <span>{formatBytes(skill.size_bytes)}</span>
        <span>{formatUpdatedAt(skill.updated_at_ms)}</span>
      </footer>
    </Link>
  );
}

function StatePanel({
  title,
  detail,
  kind,
  actionLabel,
  onAction,
}: {
  title: string;
  detail: string;
  kind?: "error";
  actionLabel?: string;
  onAction?: () => void;
}) {
  return (
    <div className={`skills-state${kind ? " skills-state-error" : ""}`} role={kind ? "alert" : undefined}>
      {kind ? <AlertCircle size={22} aria-hidden="true" /> : <FileSearch size={22} aria-hidden="true" />}
      <h2>{title}</h2>
      <p>{detail}</p>
      {actionLabel && onAction ? (
        <button type="button" className="primary-button" onClick={onAction}>
          {actionLabel}
        </button>
      ) : null}
    </div>
  );
}
