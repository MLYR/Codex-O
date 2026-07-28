import type { LucideIcon } from "lucide-react";
import {
  Blocks,
  Download,
  FileSearch,
  Gauge,
  Library,
  ListTree,
  Puzzle,
  RefreshCw,
  Settings,
} from "lucide-react";

export type RouteId =
  | "Skills"
  | "SkillDetail"
  | "Market"
  | "Install"
  | "Updates"
  | "Sessions"
  | "TokenStats"
  | "Settings"
  | "MCP";

export type DeliveryPhase = "M1" | "M2" | "M3" | "v1.x";

export interface AppRoute {
  id: RouteId;
  path: string;
  title: string;
  phase: DeliveryPhase;
  group: string;
  icon: LucideIcon;
}

export const routeManifest: readonly AppRoute[] = [
  {
    id: "Skills",
    path: "/skills",
    title: "我的 Skills",
    phase: "M1",
    group: "Skills",
    icon: Library,
  },
  {
    id: "SkillDetail",
    path: "/skills/:skillId",
    title: "Skill 详情",
    phase: "M1",
    group: "Skills",
    icon: FileSearch,
  },
  {
    id: "Market",
    path: "/market",
    title: "Skill 市场",
    phase: "M2",
    group: "管理",
    icon: Blocks,
  },
  {
    id: "Install",
    path: "/install",
    title: "安装 Skill",
    phase: "M2",
    group: "管理",
    icon: Download,
  },
  {
    id: "Updates",
    path: "/updates",
    title: "更新中心",
    phase: "M2",
    group: "管理",
    icon: RefreshCw,
  },
  {
    id: "Sessions",
    path: "/sessions",
    title: "会话管理",
    phase: "M3",
    group: "会话与使用",
    icon: ListTree,
  },
  {
    id: "TokenStats",
    path: "/token-stats",
    title: "Token 统计",
    phase: "M3",
    group: "会话与使用",
    icon: Gauge,
  },
  {
    id: "Settings",
    path: "/settings",
    title: "设置",
    phase: "M1",
    group: "系统",
    icon: Settings,
  },
  {
    id: "MCP",
    path: "/mcp",
    title: "MCP 管理",
    phase: "v1.x",
    group: "系统",
    icon: Puzzle,
  },
];
