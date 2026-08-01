import { beforeEach, describe, expect, it, vi } from "vitest";

const pluginLog = vi.hoisted(() => ({
  error: vi.fn(),
  info: vi.fn(),
  warn: vi.fn(),
}));

vi.mock("@tauri-apps/plugin-log", () => pluginLog);

import { logFrontendEvent } from "./logger";

const event = {
  category: "system" as const,
  domain: "app" as const,
  event_code: "frontend_ready",
  result: "succeeded" as const,
  module: "frontend",
  retryable: false,
};

describe("frontend logger", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("drops unsafe identifiers before calling the plugin", async () => {
    await logFrontendEvent("info", { ...event, module: "/Users/example" });

    expect(pluginLog.info).not.toHaveBeenCalled();
  });

  it("rebuilds the payload without runtime-injected fields", async () => {
    await logFrontendEvent("info", { ...event, prompt: "secret" } as typeof event & { prompt: string });

    const payload = JSON.parse(pluginLog.info.mock.calls[0][0] as string) as Record<string, unknown>;
    expect(payload.event_code).toBe("frontend_ready");
    expect(payload.prompt).toBeUndefined();
  });
});
