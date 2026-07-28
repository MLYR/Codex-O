import { CircleDot } from "lucide-react";
import { useEffect, useState } from "react";
import type { AppRoute } from "../app/RouteManifest";
import { settingsApi, type AiConfigView } from "../features/settings/api";

interface TopbarProps {
  route: AppRoute;
}

export function Topbar({ route }: TopbarProps) {
  const [aiConfig, setAiConfig] = useState<AiConfigView>();

  useEffect(() => {
    let active = true;
    void settingsApi
      .getAiConfig()
      .then((config) => {
        if (active) {
          setAiConfig(config);
        }
      })
      .catch(() => {
        if (active) {
          setAiConfig(undefined);
        }
      });
    return () => {
      active = false;
    };
  }, []);

  const status = aiStatus(aiConfig);
  return (
    <header className="topbar">
      <span className="topbar-title">{route.title}</span>
      <span className={`topbar-status topbar-status-${status.tone}`}>
        <span className="status-dot" aria-hidden="true" />
        <CircleDot size={14} strokeWidth={1.75} aria-hidden="true" />
        {status.label}
      </span>
    </header>
  );
}

function aiStatus(config?: AiConfigView) {
  if (!config?.configured) {
    return { tone: "warning", label: "AI 未配置" };
  }
  if (config.privacy_mode && config.kind !== "ollama") {
    return { tone: "warning", label: "隐私模式 · 远程已阻断" };
  }
  const provider = {
    open_ai_compatible: "OpenAI-compatible",
    anthropic: "Anthropic",
    ollama: "Ollama",
  }[config.kind];
  return {
    tone: "ready",
    label: `${provider} · ${config.model || "已配置"}`,
  };
}
