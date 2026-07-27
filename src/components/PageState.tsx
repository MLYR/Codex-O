import type { AppRoute } from "../app/RouteManifest";

interface PageStateProps {
  route: AppRoute;
}

export function PageState({ route }: PageStateProps) {
  const Icon = route.icon;
  const isRoadmap = route.phase !== "M1";

  return (
    <section className="page-state" aria-labelledby="page-state-title">
      <div className="page-state-panel">
        <div className="page-icon" aria-hidden="true">
          <Icon size={28} strokeWidth={1.5} />
        </div>
        <h1 id="page-state-title">{route.title}</h1>
        <p className="page-status">
          <span className="page-status-dot" aria-hidden="true" />
          {route.phase} 阶段空壳
        </p>
        {isRoadmap ? <p className="page-roadmap">Roadmap · 此能力将在 {route.phase} 阶段实现。</p> : null}
      </div>
    </section>
  );
}
