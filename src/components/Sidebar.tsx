import { Bot, CircleDot } from "lucide-react";
import { NavLink } from "react-router-dom";
import { routeManifest, type AppRoute } from "../app/RouteManifest";

interface SidebarProps {
  activeRoute: AppRoute;
}

export function Sidebar({ activeRoute }: SidebarProps) {
  const groups = [...new Set(routeManifest.map((route) => route.group))];

  return (
    <aside className="sidebar" aria-label="主导航" data-active-route={activeRoute.id}>
      <div className="brand">
        <span className="brand-mark" aria-hidden="true">
          <Bot size={16} strokeWidth={1.75} />
        </span>
        <span className="brand-name">Codex-O</span>
        <span className="brand-version">T0</span>
      </div>
      <nav className="navigation">
        {groups.map((group) => (
          <section className="nav-group" key={group} aria-label={group}>
            <span className="nav-label">{group}</span>
            {routeManifest
              .filter((route) => route.group === group)
              .map((route) => {
                const Icon = route.icon;
                const isDetail = route.id === "SkillDetail";

                return (
                  <NavLink
                    className="nav-link"
                    key={route.id}
                    to={isDetail ? "/skills/example" : route.path}
                    aria-label={route.title}
                    end={!isDetail}
                  >
                    <Icon className="nav-icon" size={16} strokeWidth={1.75} />
                    <span>{route.title}</span>
                  </NavLink>
                );
              })}
          </section>
        ))}
      </nav>
      <div className="sidebar-footer">
        <div className="sidebar-status">
          <CircleDot size={14} strokeWidth={1.75} />
          <span>工程初始化中</span>
        </div>
      </div>
    </aside>
  );
}
