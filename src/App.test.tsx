// @vitest-environment jsdom

import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { MemoryRouter } from "react-router-dom";
import App from "./App";

afterEach(() => cleanup());

describe("App install route", () => {
  it("renders the production local import page instead of the roadmap placeholder", () => {
    // Render the production route tree without triggering any import action.
    render(
      <MemoryRouter initialEntries={["/install"]}>
        <App />
      </MemoryRouter>,
    );

    expect(screen.getByRole("heading", { name: "本地导入" })).not.toBeNull();
    expect(screen.getByRole("button", { name: /选择 SKILL.md/ })).not.toBeNull();
    expect(screen.getByRole("button", { name: /选择 Skill 目录/ })).not.toBeNull();
    expect(screen.queryByText("Roadmap · 此能力将在 M2 阶段实现。")).toBeNull();
  });
});
