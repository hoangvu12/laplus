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

function renderReport(width: number, reportView = view) {
  Object.defineProperty(window, "innerWidth", { configurable: true, value: width });
  const root = createRootRoute();
  const route = createRoute({
    getParentRoute: () => root,
    path: "/usage",
    component: () => <UsageReport view={reportView} />,
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
    fireEvent.click(screen.getByRole("button", { name: "TOKENS" }));
    expect(screen.getByText("50")).toBeTruthy();
    expect(screen.getByText("Processed tokens")).toBeTruthy();
  });

  it("moves from API-equivalent headline through providers, metrics, chart and breakdowns", async () => {
    renderReport(1280, {
      ...view,
      summary: {
        contractVersion: 3,
        readAt: "2026-08-09T12:00:00Z",
        timeZone: "UTC",
        sinceDay: view.input.sinceDay,
        untilDay: view.input.untilDay,
        buckets: [
          {
            day: "2026-08-09",
            provider: "claude",
            model: "claude-fixture",
            totals: {
              uncachedInputTokens: 10,
              cachedInputTokens: 20,
              cacheCreationTokens: 5,
              outputTokens: 15,
              reasoningTokens: 3,
            },
            costUsd: 1.25,
            cacheSavingsUsd: 0.5,
            costSource: "providerReported",
            records: 1,
            unpricedRecords: 0,
            sessions: 1,
          },
        ],
        sources: [],
        pricing: {
          status: "fresh",
          source: "LiteLLM",
          fetchedAt: "2026-08-09T12:00:00Z",
          knownModels: 1,
        },
        scanDurationMs: 1,
        totals: {
          uncachedInputTokens: 10,
          cachedInputTokens: 20,
          cacheCreationTokens: 5,
          outputTokens: 15,
          reasoningTokens: 3,
          processedTokens: 50,
          costUsd: 1.25,
          cacheSavingsUsd: 0.5,
          records: 1,
          unpricedRecords: 0,
        },
        distinctSessions: 1,
        byProvider: [
          {
            key: "claude",
            provider: "claude",
            totals: {
              uncachedInputTokens: 10,
              cachedInputTokens: 20,
              cacheCreationTokens: 5,
              outputTokens: 15,
              reasoningTokens: 3,
              processedTokens: 50,
              costUsd: 1.25,
              cacheSavingsUsd: 0.5,
              records: 1,
              unpricedRecords: 0,
            },
            tokenShare: 1,
            costShare: 1,
          },
        ],
        byModel: [],
        byDay: [
          {
            key: "2026-08-09",
            day: "2026-08-09",
            totals: {
              uncachedInputTokens: 10,
              cachedInputTokens: 20,
              cacheCreationTokens: 5,
              outputTokens: 15,
              reasoningTokens: 3,
              processedTokens: 50,
              costUsd: 1.25,
              cacheSavingsUsd: 0.5,
              records: 1,
              unpricedRecords: 0,
            },
            tokenShare: 1,
            costShare: 1,
          },
        ],
      } as never,
    });
    expect(await screen.findByText("Raw token cost")).toBeTruthy();
    expect(screen.getByText("* if billed at full API rate")).toBeTruthy();
    expect(screen.getAllByText("Claude Code").length).toBeGreaterThan(0);
    expect(screen.getByText("Cached input")).toBeTruthy();
    expect(screen.getByRole("img", { name: "Daily cost by provider" })).toBeTruthy();
    expect(screen.getByText("claude-fixture")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "DAY" }));
    expect(screen.getByRole("columnheader", { name: "Day" })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "TOKENS" }));
    expect(screen.getByRole("img", { name: "Daily processed tokens by provider" })).toBeTruthy();
  });

  it("returns a direct route to the main application", async () => {
    const { container } = renderReport(1280);
    fireEvent.click(await screen.findByRole("button", { name: "Back" }));
    expect(container.ownerDocument.location.pathname).toBe("/");
  });

  it("offers every reporting range, refresh, and provider coverage", async () => {
    let selected = 0;
    let refreshes = 0;
    renderReport(1280, {
      ...view,
      rangeDays: 30,
      onRangeChange: (days) => {
        selected = days;
      },
      onRefresh: () => {
        refreshes += 1;
      },
      sources: [
        {
          fingerprint: {
            hostId: "host",
            provider: "claude",
            resolvedHomePath: "/fixture",
            volumeId: "1:2",
          },
          status: "partial",
          scannedFiles: 1,
          skippedFiles: 1,
          malformedRecords: 0,
          distinctSessions: 1,
          message: "Some rows were skipped",
        },
      ],
    });
    fireEvent.click(await screen.findByRole("button", { name: "7 days" }));
    fireEvent.click(screen.getByRole("button", { name: "Refresh usage" }));
    expect(selected).toBe(7);
    expect(refreshes).toBe(1);
    expect(screen.getByText("Claude: partial")).toBeTruthy();
  });
});
