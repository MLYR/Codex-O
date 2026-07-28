// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { settingsApi } from "./api";
import { SettingsPage } from "./SettingsPage";

vi.mock("./api", () => ({
  settingsApi: {
    getScanPreferences: vi.fn(),
    updateScanPreferences: vi.fn(),
  },
}));

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(settingsApi.getScanPreferences).mockResolvedValue({
    include_plugin_cache: false,
    initial_scan_notice_seen: false,
  });
  vi.mocked(settingsApi.updateScanPreferences).mockResolvedValue({
    include_plugin_cache: true,
    initial_scan_notice_seen: false,
  });
});

afterEach(() => {
  cleanup();
});

describe("SettingsPage", () => {
  it("shows plugin scanning as disabled by default", async () => {
    render(<SettingsPage />);

    const toggle = await screen.findByRole("switch", {
      name: "扫描插件与内置 Skill",
    });

    expect((toggle as HTMLInputElement).checked).toBe(false);
    expect(screen.getByText("已关闭")).not.toBeNull();
  });

  it("persists the enabled preference through the Rust API", async () => {
    render(<SettingsPage />);
    const toggle = await screen.findByRole("switch", {
      name: "扫描插件与内置 Skill",
    });

    fireEvent.click(toggle);

    await waitFor(() => {
      expect(settingsApi.updateScanPreferences).toHaveBeenCalledWith(true);
      expect((toggle as HTMLInputElement).checked).toBe(true);
    });
    expect(screen.getByText("已开启，将在下次扫描时生效")).not.toBeNull();
  });
});
