import { describe, expect, it } from "vitest";
import { routeManifest } from "./RouteManifest";

const expectedRouteIds = [
  "Skills",
  "SkillDetail",
  "Market",
  "Install",
  "Updates",
  "Sessions",
  "TokenStats",
  "Settings",
  "MCP",
];

describe("routeManifest", () => {
  it("contains the complete, unique set of formal routes", () => {
    expect(routeManifest).toHaveLength(9);
    expect(routeManifest.map((route) => route.id).sort()).toEqual([...expectedRouteIds].sort());
    expect(new Set(routeManifest.map((route) => route.path)).size).toBe(routeManifest.length);
  });
});
