import { Navigate, Route, Routes } from "react-router-dom";
import type { ReactNode } from "react";
import { routeManifest, type AppRoute } from "./app/RouteManifest";
import { AppShell } from "./components/AppShell";
import { PageState } from "./components/PageState";
import { SettingsPage } from "./features/settings/SettingsPage";
import { InstallPage } from "./features/install/InstallPage";
import { MarketPage } from "./features/market/MarketPage";
import { SkillDetailPage } from "./features/skills/SkillDetailPage";
import { SkillsPage } from "./features/skills/SkillsPage";
import "./App.css";

function RoutePage({ route, children }: { route: AppRoute; children?: ReactNode }) {
  return (
    <AppShell route={route}>
      {children ?? <PageState route={route} />}
    </AppShell>
  );
}

function App() {
  const skillsRoute = routeManifest.find((route) => route.id === "Skills");
  const skillDetailRoute = routeManifest.find((route) => route.id === "SkillDetail");
  const installRoute = routeManifest.find((route) => route.id === "Install");
  const marketRoute = routeManifest.find((route) => route.id === "Market");
  const settingsRoute = routeManifest.find((route) => route.id === "Settings");

  if (!skillsRoute || !skillDetailRoute || !installRoute || !marketRoute || !settingsRoute) {
    return null;
  }

  return (
    <Routes>
      <Route path="/" element={<Navigate to="/skills" replace />} />
      <Route path="/skills" element={<RoutePage route={skillsRoute}><SkillsPage /></RoutePage>} />
      <Route
        path="/skills/:skillId"
        element={<RoutePage route={skillDetailRoute}><SkillDetailPage /></RoutePage>}
      />
      <Route
        path="/settings"
        element={<RoutePage route={settingsRoute}><SettingsPage /></RoutePage>}
      />
      <Route
        path="/install"
        element={<RoutePage route={installRoute}><InstallPage /></RoutePage>}
      />
      <Route
        path="/market"
        element={<RoutePage route={marketRoute}><MarketPage /></RoutePage>}
      />
      {routeManifest
        .filter(
          (route) =>
            route.id !== "Skills" &&
            route.id !== "SkillDetail" &&
            route.id !== "Install" &&
            route.id !== "Market" &&
            route.id !== "Settings",
        )
        .map((route) => (
        <Route key={route.id} path={route.path} element={<RoutePage route={route} />} />
        ))}
      <Route path="*" element={<Navigate to="/skills" replace />} />
    </Routes>
  );
}

export default App;
