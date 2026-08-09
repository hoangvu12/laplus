// @vitest-environment happy-dom
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import {
  createMemoryHistory,
  createRootRoute,
  createRoute,
  createRouter,
  RouterProvider,
} from "@tanstack/react-router";
import { afterEach, describe, expect, it } from "vite-plus/test";

import { UsageReport, type UsageReportView } from "./UsagePage";

const view: UsageReportView = {
  input: { sinceDay: "2026-08-03" as never, untilDay: "2026-08-09" as never, timeZone: "UTC" },
  state: "success",
  processedTokens: 50,
  error: null,
};

afterEach(cleanup);

function renderReport(width: number) {
  Object.defineProperty(window, "innerWidth", { configurable: true, value: width });
  const root = createRootRoute();
  const route = createRoute({
    getParentRoute: () => root,
    path: "/usage",
    component: () => <UsageReport view={view} />,
  });
  const router = createRouter({
    routeTree: root.addChildren([route]),
    history: createMemoryHistory({ initialEntries: ["/usage"] }),
  });
  return render(<RouterProvider router={router} />);
}

describe("Usage route", () => {
  it.each([1280, 375])("renders the inclusive range and processed total at %ipx", async (width) => {
    renderReport(width);
    expect(await screen.findByRole("heading", { name: "Usage" })).toBeTruthy();
    expect(screen.getByText("Aug 3, 2026 to Aug 9, 2026")).toBeTruthy();
    expect(screen.getByText("50")).toBeTruthy();
    expect(screen.getByText("Processed tokens")).toBeTruthy();
  });

  it("returns a direct route to the main application", async () => {
    const { container } = renderReport(1280);
    fireEvent.click(await screen.findByRole("button", { name: "Back" }));
    expect(container.ownerDocument.location.pathname).toBe("/");
  });
});
