import { CircleDot } from "lucide-react";
import type { AppRoute } from "../app/RouteManifest";

interface TopbarProps {
  route: AppRoute;
}

export function Topbar({ route }: TopbarProps) {
  return (
    <header className="topbar">
      <span className="topbar-title">{route.title}</span>
      <span className="topbar-status">
        <span className="status-dot" aria-hidden="true" />
        <CircleDot size={14} strokeWidth={1.75} aria-hidden="true" />
        AI 未配置
      </span>
    </header>
  );
}
