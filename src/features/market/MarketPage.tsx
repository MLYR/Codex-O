import {
  AlertCircle,
  Check,
  Download,
  LoaderCircle,
  PackageSearch,
  RefreshCw,
  Search,
} from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { Link } from "react-router-dom";
import {
  installApi,
  safeOperationError,
  type OperationError,
  type PlannedImport,
} from "../install/api";
import { OperationPlanModal } from "../install/OperationPlanModal";
import { marketApi, type MarketCatalog } from "./api";
import "../install/InstallPage.css";
import "./MarketPage.css";

export function MarketPage() {
  const [catalog, setCatalog] = useState<MarketCatalog>();
  const [query, setQuery] = useState("");
  const [category, setCategory] = useState("");
  const [isLoading, setIsLoading] = useState(true);
  const [isRefreshing, setIsRefreshing] = useState(false);
  const [planningId, setPlanningId] = useState<string>();
  const [plannedImport, setPlannedImport] = useState<PlannedImport>();
  const [isExecuting, setIsExecuting] = useState(false);
  const [operationError, setOperationError] = useState<OperationError>();
  const activeConfirmation = useRef<string | undefined>(undefined);

  const refresh = useCallback(async () => {
    setIsRefreshing(true);
    try {
      setCatalog(await marketApi.refreshCatalog());
    } catch (failure) {
      setOperationError(safeOperationError(failure));
    } finally {
      setIsRefreshing(false);
      setIsLoading(false);
    }
  }, []);

  useEffect(() => {
    let active = true;
    void marketApi
      .getCatalog()
      .then((snapshot) => {
        if (active) {
          setCatalog(snapshot);
          setIsLoading(false);
        }
      })
      .catch(() => undefined)
      .finally(() => {
        if (active) {
          void refresh();
        }
      });
    return () => {
      active = false;
      const token = activeConfirmation.current;
      activeConfirmation.current = undefined;
      if (token) {
        void installApi.cancelSkillImport(token).catch(() => undefined);
      }
    };
  }, [refresh]);

  const categories = useMemo(
    () =>
      Array.from(
        new Set(
          (catalog?.items ?? [])
            .map((item) => item.category)
            .filter((value): value is string => Boolean(value)),
        ),
      ).sort((left, right) => left.localeCompare(right)),
    [catalog],
  );

  const items = useMemo(() => {
    const normalized = query.trim().toLocaleLowerCase();
    return (catalog?.items ?? []).filter((item) => {
      const matchesQuery =
        !normalized ||
        item.skill_name.toLocaleLowerCase().includes(normalized) ||
        item.plugin_name.toLocaleLowerCase().includes(normalized) ||
        item.description?.toLocaleLowerCase().includes(normalized);
      return matchesQuery && (!category || item.category === category);
    });
  }, [catalog, category, query]);

  const closePlan = useCallback(() => {
    if (isExecuting) {
      return;
    }
    const token = activeConfirmation.current;
    activeConfirmation.current = undefined;
    setPlannedImport(undefined);
    if (token) {
      void installApi.cancelSkillImport(token).catch(() => undefined);
    }
  }, [isExecuting]);

  const planImport = async (marketItemId: string) => {
    setPlanningId(marketItemId);
    setOperationError(undefined);
    try {
      const plan = await marketApi.planImport(marketItemId);
      activeConfirmation.current = plan.confirmation_token?.token;
      setPlannedImport(plan);
    } catch (failure) {
      setOperationError(safeOperationError(failure));
    } finally {
      setPlanningId(undefined);
    }
  };

  const executeImport = async () => {
    const token = activeConfirmation.current;
    if (!token) {
      return;
    }
    setIsExecuting(true);
    setOperationError(undefined);
    activeConfirmation.current = undefined;
    try {
      await installApi.executeSkillImport(token);
      setPlannedImport(undefined);
      setCatalog(await marketApi.getCatalog());
    } catch (failure) {
      setPlannedImport(undefined);
      setOperationError(safeOperationError(failure));
    } finally {
      setIsExecuting(false);
    }
  };

  return (
    <section className="market-page" aria-labelledby="market-title">
      <header className="market-header">
        <div>
          <h1 id="market-title">官方市场</h1>
          <p>{catalog?.provider_name ?? "OpenAI 官方 Skill 来源"}</p>
        </div>
        <button
          className="icon-text-button"
          type="button"
          disabled={isRefreshing}
          onClick={() => void refresh()}
        >
          <RefreshCw
            size={16}
            aria-hidden="true"
            className={isRefreshing ? "is-spinning" : ""}
          />
          {isRefreshing ? "正在同步" : "同步市场"}
        </button>
      </header>

      {catalog?.status === "stale" ? (
        <div className="market-notice is-stale" role="status">
          <AlertCircle size={17} aria-hidden="true" />
          <div>
            <strong>正在使用上次同步的市场快照</strong>
            <span>{catalog.issue?.recovery}</span>
          </div>
        </div>
      ) : null}

      {operationError ? (
        <div className="market-notice is-error" role="alert">
          <AlertCircle size={17} aria-hidden="true" />
          <div>
            <strong>{operationError.code}</strong>
            <span>{operationError.recovery}</span>
          </div>
        </div>
      ) : null}

      {catalog && catalog.status !== "unavailable" ? (
        <>
          <div className="market-toolbar" aria-label="市场筛选">
            <label className="market-search">
              <Search size={16} aria-hidden="true" />
              <input
                aria-label="搜索市场 Skill"
                value={query}
                placeholder="搜索 Skill 或插件"
                onChange={(event) => setQuery(event.target.value)}
              />
            </label>
            <label className="market-category">
              <span>分类</span>
              <select
                aria-label="市场分类"
                value={category}
                onChange={(event) => setCategory(event.target.value)}
              >
                <option value="">全部分类</option>
                {categories.map((value) => (
                  <option key={value} value={value}>
                    {value}
                  </option>
                ))}
              </select>
            </label>
            <div className="market-source-meta">
              <span>{catalog.items.length} 个 Skill</span>
              <code title={catalog.commit_sha}>commit {catalog.commit_sha?.slice(0, 8)}</code>
            </div>
          </div>

          {items.length ? (
            <div className="market-grid" aria-live="polite">
              {items.map((item) => (
                <article className="market-item" key={item.id}>
                  <div className="market-item-main">
                    <div className="market-item-heading">
                      <h2>{item.skill_name}</h2>
                      {item.category ? <span>{item.category}</span> : null}
                    </div>
                    <p>{item.description ?? "该条目未提供描述。"}</p>
                    <small>插件：{item.plugin_name}</small>
                  </div>
                  <button
                    className={item.installed ? "market-install is-installed" : "market-install"}
                    type="button"
                    disabled={item.installed || Boolean(planningId)}
                    onClick={() => void planImport(item.id)}
                  >
                    {item.installed ? (
                      <Check size={15} aria-hidden="true" />
                    ) : planningId === item.id ? (
                      <LoaderCircle className="is-spinning" size={15} aria-hidden="true" />
                    ) : (
                      <Download size={15} aria-hidden="true" />
                    )}
                    {item.installed ? "已安装" : planningId === item.id ? "正在检查" : "安装"}
                  </button>
                </article>
              ))}
            </div>
          ) : (
            <MarketState
              title={catalog.items.length ? "没有匹配的 Skill" : "市场暂时没有可用 Skill"}
              detail={catalog.items.length ? "调整搜索词或分类后重试。" : "同步完成后仍未发现符合策略的 Skill。"}
            />
          )}
        </>
      ) : isLoading ? (
        <MarketState loading title="正在读取市场" detail="正在检查本地快照。" />
      ) : (
        <MarketState
          error
          title="官方市场暂时不可用"
          detail={catalog?.issue?.recovery ?? "请检查网络连接后重新同步。"}
        >
          <Link className="icon-text-button" to="/install">
            <Download size={15} aria-hidden="true" />
            使用本地或 GitHub 安装
          </Link>
        </MarketState>
      )}

      {plannedImport ? (
        <OperationPlanModal
          plannedImport={plannedImport}
          busy={isExecuting}
          onClose={closePlan}
          onConfirm={() => void executeImport()}
        />
      ) : null}
    </section>
  );
}

function MarketState({
  title,
  detail,
  loading = false,
  error = false,
  children,
}: {
  title: string;
  detail: string;
  loading?: boolean;
  error?: boolean;
  children?: ReactNode;
}) {
  return (
    <div className="market-state" role={error ? "alert" : "status"}>
      {loading ? (
        <LoaderCircle className="is-spinning" size={22} aria-hidden="true" />
      ) : error ? (
        <AlertCircle size={22} aria-hidden="true" />
      ) : (
        <PackageSearch size={22} aria-hidden="true" />
      )}
      <strong>{title}</strong>
      <span>{detail}</span>
      {children}
    </div>
  );
}
