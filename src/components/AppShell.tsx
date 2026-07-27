import type { ReactNode } from "react";
import type { AppRoute } from "../app/RouteManifest";
import { Sidebar } from "./Sidebar";
import { Topbar } from "./Topbar";

interface AppShellProps {
  route: AppRoute;
  children: ReactNode;
}

export function AppShell({ route, children }: AppShellProps) {
  return (
    <div className="app-shell">
      <Sidebar activeRoute={route} />
      <div className="app-main">
        <Topbar route={route} />
        <main className="content">{children}</main>
      </div>
    </div>
  );
}
