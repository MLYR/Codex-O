// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  settingsApi,
  type AiConfigView,
  type DiagnosticPage,
  type EnvironmentHealth,
} from "./api";
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
    getDeveloperSettings: vi.fn(),
    setDeveloperMode: vi.fn(),
    listDiagnostics: vi.fn(),
    exportDiagnostics: vi.fn(),
    clearDiagnostics: vi.fn(),
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

const diagnosticPage: DiagnosticPage = {
  total: 2,
  store_status: "available",
  dropped_count: 0,
  records: [
    {
      id: "evt-0000000000000001-0000000000000001",
      occurred_at: 1_780_000_000_000,
      level: "error",
      domain: "skill_scan",
      event_code: "skill_scan_failed",
      result: "failed",
      duration_ms: 14,
      error_code: "scan_failed",
      retryable: true,
      recovery_code: "rescan",
      entity_ref: "0123456789abcdef0123",
    },
    {
      id: "evt-0000000000000002-0000000000000002",
      occurred_at: 1_780_000_001_000,
      level: "info",
      domain: "app",
      event_code: "app_started",
      result: "succeeded",
      retryable: false,
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
  vi.mocked(settingsApi.getDeveloperSettings).mockResolvedValue({
    developer_mode_enabled: false,
    store_status: "available",
    memory_capacity: 500,
    file_limit: 5,
    total_bytes_limit: 10 * 1024 * 1024,
  });
  vi.mocked(settingsApi.setDeveloperMode).mockResolvedValue({
    developer_mode_enabled: true,
    store_status: "available",
    memory_capacity: 500,
    file_limit: 5,
    total_bytes_limit: 10 * 1024 * 1024,
  });
  vi.mocked(settingsApi.listDiagnostics).mockResolvedValue(diagnosticPage);
  vi.mocked(settingsApi.exportDiagnostics).mockResolvedValue({
    record_count: 2,
    file_name: "codex-o-diagnostics.jsonl",
  });
  vi.mocked(settingsApi.clearDiagnostics).mockResolvedValue({
    memory_records_cleared: 2,
    files_cleared: 1,
  });
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

  it("sends an explicit clear action for an existing remote key", async () => {
    vi.mocked(settingsApi.getAiConfig).mockResolvedValue({
      ...aiConfig,
      configured: true,
      has_api_key: true,
    });
    render(<SettingsPage />);
    const clearButton = await screen.findByRole("button", { name: "清除" });

    fireEvent.click(clearButton);
    fireEvent.click(screen.getByRole("button", { name: "保存配置" }));

    await waitFor(() => {
      expect(settingsApi.saveAiConfig).toHaveBeenCalledWith(
        expect.objectContaining({
          secretAction: "clear",
          apiKey: undefined,
        }),
      );
    });
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

  it("keeps the diagnostics view hidden while developer mode is disabled", async () => {
    render(<SettingsPage />);

    const developerMode = await screen.findByRole("switch", { name: "开发者模式" });
    expect((developerMode as HTMLInputElement).checked).toBe(false);
    expect(screen.queryByRole("tab", { name: "诊断与日志" })).toBeNull();
    expect(settingsApi.listDiagnostics).not.toHaveBeenCalled();
  });

  it("persists developer mode through Rust before revealing diagnostics", async () => {
    render(<SettingsPage />);
    const developerMode = await screen.findByRole("switch", { name: "开发者模式" });

    fireEvent.click(developerMode);

    await waitFor(() => {
      expect(settingsApi.setDeveloperMode).toHaveBeenCalledWith(true);
    });
    expect(await screen.findByRole("tab", { name: "诊断与日志" })).not.toBeNull();
  });

  it("loads structured diagnostics and shows the selected event detail", async () => {
    vi.mocked(settingsApi.getDeveloperSettings).mockResolvedValue({
      developer_mode_enabled: true,
      store_status: "available",
      memory_capacity: 500,
      file_limit: 5,
      total_bytes_limit: 10 * 1024 * 1024,
    });
    render(<SettingsPage />);
    fireEvent.click(await screen.findByRole("tab", { name: "诊断与日志" }));

    expect((await screen.findAllByText("Skill 扫描失败")).length).toBe(2);
    expect(screen.getAllByText("scan_failed").length).toBe(2);
    expect(screen.getByText("0123456789abcdef0123")).not.toBeNull();
    expect(screen.queryByText("/Users/example")).toBeNull();
  });

  it("applies enumerated diagnostic filters through the backend query", async () => {
    vi.mocked(settingsApi.getDeveloperSettings).mockResolvedValue({
      developer_mode_enabled: true,
      store_status: "available",
      memory_capacity: 500,
      file_limit: 5,
      total_bytes_limit: 10 * 1024 * 1024,
    });
    render(<SettingsPage />);
    fireEvent.click(await screen.findByRole("tab", { name: "诊断与日志" }));
    await screen.findAllByText("Skill 扫描失败");

    fireEvent.change(screen.getByLabelText("诊断级别"), { target: { value: "error" } });

    await waitFor(() => {
      expect(settingsApi.listDiagnostics).toHaveBeenLastCalledWith(
        expect.objectContaining({ level: "error", limit: 200 }),
      );
    });
  });

  it("shows the memory-only degradation and an empty state", async () => {
    vi.mocked(settingsApi.getDeveloperSettings).mockResolvedValue({
      developer_mode_enabled: true,
      store_status: "memory_only",
      memory_capacity: 500,
      file_limit: 5,
      total_bytes_limit: 10 * 1024 * 1024,
    });
    vi.mocked(settingsApi.listDiagnostics).mockResolvedValue({
      records: [],
      total: 0,
      store_status: "memory_only",
      dropped_count: 0,
    });
    render(<SettingsPage />);
    fireEvent.click(await screen.findByRole("tab", { name: "诊断与日志" }));

    expect(await screen.findByText("没有匹配的诊断记录")).not.toBeNull();
    expect(screen.getByText(/日志目录不可用/)).not.toBeNull();
  });

  it("exports and requires a second click before clearing diagnostics", async () => {
    vi.mocked(settingsApi.getDeveloperSettings).mockResolvedValue({
      developer_mode_enabled: true,
      store_status: "available",
      memory_capacity: 500,
      file_limit: 5,
      total_bytes_limit: 10 * 1024 * 1024,
    });
    render(<SettingsPage />);
    fireEvent.click(await screen.findByRole("tab", { name: "诊断与日志" }));
    await screen.findAllByText("Skill 扫描失败");

    fireEvent.click(screen.getByRole("button", { name: "导出" }));
    await waitFor(() => expect(settingsApi.exportDiagnostics).toHaveBeenCalledTimes(1));

    fireEvent.click(screen.getByRole("button", { name: "清空" }));
    expect(settingsApi.clearDiagnostics).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "确认清空" }));

    await waitFor(() => expect(settingsApi.clearDiagnostics).toHaveBeenCalledTimes(1));
    expect(await screen.findByText("没有匹配的诊断记录")).not.toBeNull();
  });
});
