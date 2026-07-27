import { Navigate, Route, Routes } from "react-router-dom";
import { routeManifest, type AppRoute } from "./app/RouteManifest";
import { AppShell } from "./components/AppShell";
import { PageState } from "./components/PageState";
import "./App.css";

function RoutePage({ route }: { route: AppRoute }) {
  return (
    <AppShell route={route}>
      <PageState route={route} />
    </AppShell>
  );
}

function App() {
  return (
    <Routes>
      <Route path="/" element={<Navigate to="/skills" replace />} />
      {routeManifest.map((route) => (
        <Route key={route.id} path={route.path} element={<RoutePage route={route} />} />
      ))}
      <Route path="*" element={<Navigate to="/skills" replace />} />
    </Routes>
  );
}

export default App;
