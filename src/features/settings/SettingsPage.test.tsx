// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { settingsApi, type AiConfigView, type EnvironmentHealth } from "./api";
import { SettingsPage } from "./SettingsPage";

vi.mock("./api", () => ({
  settingsApi: {
    getScanPreferences: vi.fn(),
    updateScanPreferences: vi.fn(),
    getAiConfig: vi.fn(),
    saveAiConfig: vi.fn(),
    testAiConnection: vi.fn(),
    getEnvironmentHealth: vi.fn(),
    listAdditionalRoots: vi.fn(),
    selectAdditionalRoot: vi.fn(),
    removeAdditionalRoot: vi.fn(),
  },
}));

const aiConfig: AiConfigView = {
  configured: false,
  kind: "open_ai_compatible",
  base_url: "https://api.openai.com/v1/",
  model: "model",
  language: "zh-CN",
  timeout_seconds: 45,
  privacy_mode: false,
  has_api_key: false,
};

const health: EnvironmentHealth = {
  items: [
    {
      id: "app_database",
      status: "ready",
      code: "app_database_ready",
      recommendation: "Ready.",
    },
    {
      id: "codex_data_source",
      status: "warning",
      code: "codex_data_source_incompatible",
      recommendation: "Use a compatible source.",
    },
  ],
};

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(settingsApi.getScanPreferences).mockResolvedValue({
    include_plugin_cache: false,
    include_bundled_cache: false,
    initial_scan_notice_seen: false,
  });
  vi.mocked(settingsApi.updateScanPreferences).mockResolvedValue({
    include_plugin_cache: true,
    include_bundled_cache: false,
    initial_scan_notice_seen: false,
  });
  vi.mocked(settingsApi.getAiConfig).mockResolvedValue(aiConfig);
  vi.mocked(settingsApi.saveAiConfig).mockResolvedValue({
    ...aiConfig,
    configured: true,
    has_api_key: true,
  });
  vi.mocked(settingsApi.testAiConnection).mockResolvedValue({
    status: "ready",
    code: "ai_connection_ready",
    latency_ms: 18,
    recommendation: "Ready.",
  });
  vi.mocked(settingsApi.getEnvironmentHealth).mockResolvedValue(health);
  vi.mocked(settingsApi.listAdditionalRoots).mockResolvedValue([]);
  vi.mocked(settingsApi.selectAdditionalRoot).mockResolvedValue([
    { id: "abc123", display_name: "team-skills", read_only: true },
  ]);
  vi.mocked(settingsApi.removeAdditionalRoot).mockResolvedValue([]);
});

afterEach(() => {
  cleanup();
});

describe("SettingsPage", () => {
  it("shows Plugin and Bundled scanning as disabled by default", async () => {
    render(<SettingsPage />);

    const plugin = await screen.findByRole("switch", { name: "扫描 Plugin Skills" });
    const bundled = screen.getByRole("switch", { name: "扫描 Bundled Skills" });

    expect((plugin as HTMLInputElement).checked).toBe(false);
    expect((bundled as HTMLInputElement).checked).toBe(false);
  });

  it("persists independent Plugin and Bundled preferences", async () => {
    render(<SettingsPage />);
    const plugin = await screen.findByRole("switch", { name: "扫描 Plugin Skills" });

    fireEvent.click(plugin);

    await waitFor(() => {
      expect(settingsApi.updateScanPreferences).toHaveBeenCalledWith(true, false);
    });
    expect(screen.getByText("扫描来源已保存，将在下次扫描时生效。")).not.toBeNull();
  });

  it("sends a replacement key only on explicit save and clears the input", async () => {
    render(<SettingsPage />);
    const keyInput = await screen.findByLabelText("新的 API Key");
    fireEvent.change(keyInput, { target: { value: "fixture-secret" } });

    fireEvent.click(screen.getByRole("button", { name: "保存配置" }));

    await waitFor(() => {
      expect(settingsApi.saveAiConfig).toHaveBeenCalledWith(
        expect.objectContaining({
          secretAction: "replace",
          apiKey: "fixture-secret",
        }),
      );
    });
    expect((keyInput as HTMLInputElement).value).toBe("");
  });

  it("saves privacy mode as part of the active AI configuration", async () => {
    render(<SettingsPage />);
    const privacy = await screen.findByRole("switch", { name: "隐私模式" });

    fireEvent.click(privacy);
    fireEvent.click(screen.getByRole("button", { name: "保存配置" }));

    await waitFor(() => {
      expect(settingsApi.saveAiConfig).toHaveBeenCalledWith(
        expect.objectContaining({ privacyMode: true }),
      );
    });
  });

  it("tests only an already saved AI configuration", async () => {
    vi.mocked(settingsApi.getAiConfig).mockResolvedValue({
      ...aiConfig,
      configured: true,
      has_api_key: true,
    });
    render(<SettingsPage />);
    const testButton = await screen.findByRole("button", { name: "测试连接" });

    fireEvent.click(testButton);

    expect(await screen.findByText("连接可用 · 18 ms")).not.toBeNull();
    expect(settingsApi.testAiConnection).toHaveBeenCalledTimes(1);
  });

  it("adds and removes Additional Roots using opaque ids", async () => {
    render(<SettingsPage />);
    const addButton = await screen.findByRole("button", { name: "添加" });

    fireEvent.click(addButton);
    expect(await screen.findByText("team-skills")).not.toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "移除 team-skills" }));

    await waitFor(() => {
      expect(settingsApi.removeAdditionalRoot).toHaveBeenCalledWith("abc123");
    });
    expect(screen.queryByText("team-skills")).toBeNull();
  });

  it("renders stable health codes without paths or database content", async () => {
    render(<SettingsPage />);

    expect(await screen.findByText("app_database_ready")).not.toBeNull();
    expect(screen.getByText("codex_data_source_incompatible")).not.toBeNull();
    expect(screen.queryByText("/Users/example")).toBeNull();
    expect(screen.queryByText("threads")).toBeNull();
  });
});
