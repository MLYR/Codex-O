import { invoke } from "@tauri-apps/api/core";
import type { PlannedImport } from "../install/api";

export type MarketStatus = "ready" | "stale" | "unavailable";

export interface MarketIssue {
  code: string;
  message: string;
  recovery: string;
}

export interface MarketItem {
  id: string;
  plugin_name: string;
  skill_name: string;
  category?: string;
  description?: string;
  installed: boolean;
}

export interface MarketCatalog {
  status: MarketStatus;
  provider_name?: string;
  commit_sha?: string;
  synced_at_ms?: number;
  items: MarketItem[];
  issue?: MarketIssue;
}

export const marketApi = {
  getCatalog: () => invoke<MarketCatalog>("get_market_catalog"),
  refreshCatalog: () => invoke<MarketCatalog>("refresh_market_catalog"),
  planImport: (marketItemId: string) =>
    invoke<PlannedImport>("plan_market_import", { marketItemId }),
};
