// @vitest-environment happy-dom
/**
 * A child tab is an ordinary right-panel tab.
 *
 * The claim this file exists to hold is a negative one — that subagents did not
 * introduce a second tab language — so it is asserted the only way a negative
 * can be: by rendering a child tab and a terminal tab that carry the same name
 * and showing that the workspace draws them the same. Not "the component has no
 * subagent branch", which would be a claim about the source; the markup.
 *
 * Everything else here is the shared conventions themselves, exercised on a
 * child: the label, the close control's name, middle-click, and the context
 * menu's items. Resizing and the narrow layout are the panel shell's and are
 * not reachable without layout — ticket 07's browser scenario covers them.
 */
import type { PreviewSessionSnapshot } from "@t3tools/contracts";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vite-plus/test";

import type { RightPanelSurface } from "~/rightPanelStore";

const contextMenu = vi.hoisted(() => ({
  shown: null as ReadonlyArray<{ id: string; label: string; disabled?: boolean }> | null,
}));

vi.mock("~/localApi", () => ({
  readLocalApi: () => ({
    contextMenu: {
      show: (items: ReadonlyArray<{ id: string; label: string; disabled?: boolean }>) => {
        contextMenu.shown = items;
        return Promise.resolve(null);
      },
    },
  }),
}));

import { RightPanelTabs } from "./RightPanelTabs";

const NO_SESSIONS: Readonly<Record<string, PreviewSessionSnapshot>> = {};

const subagent = (childId: string): RightPanelSurface => ({
  id: `subagent:${childId}`,
  kind: "subagent",
  resourceId: childId,
});

const terminal = (terminalId: string): RightPanelSurface => ({
  id: `terminal:${terminalId}`,
  kind: "terminal",
  resourceId: terminalId,
  terminalIds: [terminalId],
  activeTerminalId: terminalId,
});

function renderTabs(options: {
  surfaces: readonly RightPanelSurface[];
  activeSurfaceId: string | null;
  subagentLabelsById?: ReadonlyMap<string, string>;
  terminalLabelsById?: ReadonlyMap<string, string>;
  onActivate?: (surface: RightPanelSurface) => void;
  onCloseSurface?: (surface: RightPanelSurface) => void;
}) {
  return render(
    <RightPanelTabs
      mode="embedded"
      surfaces={options.surfaces}
      activeSurfaceId={options.activeSurfaceId}
      pendingSurfaceIds={new Set()}
      previewSessions={NO_SESSIONS}
      terminalLabelsById={options.terminalLabelsById ?? new Map()}
      subagentLabelsById={options.subagentLabelsById ?? new Map()}
      onActivate={options.onActivate ?? (() => {})}
      onCloseSurface={options.onCloseSurface ?? (() => {})}
      onCloseOtherSurfaces={() => {}}
      onCloseSurfacesToRight={() => {}}
      onCloseAllSurfaces={() => {}}
      onCopyFilePath={() => {}}
      onAddBrowser={() => {}}
      onAddTerminal={() => {}}
      onAddDiff={() => {}}
      onAddFiles={() => {}}
      browserAvailable
      diffAvailable
      filesAvailable
    >
      <div>surface body</div>
    </RightPanelTabs>,
  );
}

/** The tab element a named close control belongs to. */
function tabFor(label: string): HTMLElement {
  const close = screen.getByRole("button", { name: `Close ${label}` });
  const tab = close.closest("[data-active-tab]");
  if (!(tab instanceof HTMLElement)) throw new Error(`no tab found for ${label}`);
  return tab;
}

/** Icons and generated ids are the two things two tabs may legitimately differ by. */
const comparableTab = (markup: string) =>
  markup.replace(/<svg[\s\S]*?<\/svg>/g, "<icon />").replace(/id="base-ui-[^"]*"/g, 'id="base-ui"');

afterEach(() => {
  contextMenu.shown = null;
  cleanup();
});

describe("a subagent's right-panel tab", () => {
  it("wears the name its inline row already shows", () => {
    renderTabs({
      surfaces: [subagent("call_task_1")],
      activeSurfaceId: "subagent:call_task_1",
      subagentLabelsById: new Map([["call_task_1", "Subagent explore"]]),
    });

    expect(screen.getAllByText("Subagent explore").length).toBeGreaterThan(0);
  });

  it("falls back to a neutral name rather than an identifier", () => {
    renderTabs({ surfaces: [subagent("call_task_1")], activeSurfaceId: "subagent:call_task_1" });

    expect(screen.getAllByText("Subagent").length).toBeGreaterThan(0);
    expect(screen.queryByText("call_task_1")).toBeNull();
  });

  it("activates on click, like every other tab", () => {
    const activated: RightPanelSurface[] = [];
    renderTabs({
      surfaces: [subagent("call_task_1"), subagent("call_task_2")],
      activeSurfaceId: "subagent:call_task_1",
      subagentLabelsById: new Map([
        ["call_task_1", "explore"],
        ["call_task_2", "general"],
      ]),
      onActivate: (surface) => activated.push(surface),
    });

    fireEvent.click(screen.getByRole("button", { name: "general" }));

    expect(activated).toEqual([subagent("call_task_2")]);
  });

  it("closes from the shared close control and from a middle click", () => {
    const closed: RightPanelSurface[] = [];
    renderTabs({
      surfaces: [subagent("call_task_1")],
      activeSurfaceId: "subagent:call_task_1",
      subagentLabelsById: new Map([["call_task_1", "explore"]]),
      onCloseSurface: (surface) => closed.push(surface),
    });

    fireEvent.click(screen.getByRole("button", { name: "Close explore" }));
    fireEvent(tabFor("explore"), new MouseEvent("auxclick", { button: 1, bubbles: true }));

    expect(closed).toEqual([subagent("call_task_1"), subagent("call_task_1")]);
  });

  it("offers the workspace's own tab context menu and nothing of its own", async () => {
    renderTabs({
      surfaces: [subagent("call_task_1"), subagent("call_task_2")],
      activeSurfaceId: "subagent:call_task_1",
      subagentLabelsById: new Map([
        ["call_task_1", "explore"],
        ["call_task_2", "general"],
      ]),
    });

    fireEvent.contextMenu(tabFor("explore"));
    await vi.waitFor(() => expect(contextMenu.shown).not.toBeNull());

    expect(contextMenu.shown?.map((item) => item.id)).toEqual([
      "close",
      "close-others",
      "close-to-right",
      "close-all",
    ]);
  });

  /**
   * The criterion's own words: "without bespoke status decoration". A child
   * that is working, and one that has failed, are the same tab — the inline row
   * in the conversation is where a child's state is reported — so a child tab
   * and a terminal tab of the same name differ by their icon and nothing else.
   */
  it("is drawn exactly like any other resource-addressed tab of the same name", () => {
    const child = renderTabs({
      surfaces: [subagent("call_task_1")],
      activeSurfaceId: "subagent:call_task_1",
      subagentLabelsById: new Map([["call_task_1", "worker"]]),
    });
    const childTab = comparableTab(tabFor("worker").outerHTML);
    child.unmount();

    renderTabs({
      surfaces: [terminal("term-1")],
      activeSurfaceId: "terminal:term-1",
      terminalLabelsById: new Map([["term-1", "worker"]]),
    });

    expect(childTab).toBe(comparableTab(tabFor("worker").outerHTML));
  });

  it("sits among the other surface kinds in the order the workspace gives them", () => {
    renderTabs({
      surfaces: [
        terminal("term-1"),
        subagent("call_task_1"),
        { id: "diff", kind: "diff" },
        subagent("call_task_2"),
        { id: "plan", kind: "plan" },
      ],
      activeSurfaceId: "subagent:call_task_2",
      subagentLabelsById: new Map([
        ["call_task_1", "explore"],
        ["call_task_2", "general"],
      ]),
      terminalLabelsById: new Map([["term-1", "zsh"]]),
    });

    // Read off each tab's accessible name for its close control, which is the
    // workspace's own naming convention and is in document order.
    const named = () =>
      Array.from(document.querySelectorAll<HTMLElement>("[data-active-tab]")).map((tab) => ({
        name: tab.querySelector("[aria-label^='Close ']")?.getAttribute("aria-label"),
        active: tab.dataset.activeTab,
      }));

    expect(named()).toEqual([
      { name: "Close zsh", active: "false" },
      { name: "Close explore", active: "false" },
      { name: "Close Diff", active: "false" },
      { name: "Close general", active: "true" },
      { name: "Close Plan", active: "false" },
    ]);
  });
});
