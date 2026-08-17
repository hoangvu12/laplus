// @vitest-environment happy-dom
/**
 * The child surface's scrolling shell, driven through a real DOM.
 *
 * **What these tests do not prove.** happy-dom performs no layout, so a
 * viewport here reports `scrollHeight` and `clientHeight` of zero and nothing
 * ever really scrolls. Every test below therefore *states* the metrics a
 * browser would have measured and asserts what the shell decides from them —
 * the wiring from metrics to follow state to affordance to scroll offset. That
 * the browser measures those metrics, that a wheel gesture produces them, and
 * that a restored offset lands on the same entry it did before are all
 * unproven here and belong to ticket 07's browser-driver scenario.
 *
 * The tests are still worth their cost: inverting the pinned comparison,
 * following while suspended, dropping the remembered offset, or keying the
 * memory on the child alone each turn one of them red.
 */
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vite-plus/test";

import { SubagentStreamScroller } from "./SubagentStreamScroller";
import { resetSubagentScrollMemory } from "./subagentScroll";

// happy-dom has no Web Animations API and base-ui's scroll area asks for it on
// a timer. Nothing under test depends on animations.
if (typeof Element.prototype.getAnimations !== "function") {
  Element.prototype.getAnimations = () => [];
}

const CONTENT_HEIGHT = 1000;
const VIEWPORT_HEIGHT = 400;
const LIVE_EDGE = CONTENT_HEIGHT - VIEWPORT_HEIGHT;

function viewport(): HTMLElement {
  const element = document.querySelector<HTMLElement>('[data-slot="scroll-area-viewport"]');
  if (!element) throw new Error("the child surface rendered no scroll viewport");
  return element;
}

/** State the layout a browser would have measured, since happy-dom measures none. */
function measured(element: HTMLElement): HTMLElement {
  Object.defineProperty(element, "scrollHeight", {
    value: CONTENT_HEIGHT,
    configurable: true,
  });
  Object.defineProperty(element, "clientHeight", { value: VIEWPORT_HEIGHT, configurable: true });
  return element;
}

function scrollTo(offset: number) {
  const element = measured(viewport());
  element.scrollTop = offset;
  fireEvent.scroll(element);
}

function renderScroller(
  props: { surfaceKey?: string; contentKey?: string } = {},
): ReturnType<typeof render> {
  return render(
    <SubagentStreamScroller
      surfaceKey={props.surfaceKey ?? "env-1:thread-A:child-1"}
      contentKey={props.contentKey ?? "1"}
      streamState="working"
    >
      <p>the child said something</p>
    </SubagentStreamScroller>,
  );
}

const jumpToLatest = () => screen.queryByRole("button", { name: "Scroll to end" });

beforeEach(() => {
  resetSubagentScrollMemory();
});

afterEach(() => {
  cleanup();
});

describe("a child work stream's scrolling shell", () => {
  it("shows the child's work and, while following, offers no jump-to-latest", () => {
    renderScroller();

    expect(screen.getByText("the child said something")).not.toBeNull();
    expect(jumpToLatest()).toBeNull();
  });

  /** Sticky following: new work while pinned moves the reader with it. */
  it("follows new entries while the reader is pinned to the bottom", () => {
    const view = renderScroller({ contentKey: "1" });
    measured(viewport());

    view.rerender(
      <SubagentStreamScroller
        surfaceKey="env-1:thread-A:child-1"
        contentKey="2"
        streamState="working"
      >
        <p>the child said something</p>
      </SubagentStreamScroller>,
    );

    expect(viewport().scrollTop).toBe(LIVE_EDGE);
  });

  it("suspends following when the reader scrolls up, and says how to come back", () => {
    renderScroller();

    scrollTo(120);

    expect(jumpToLatest()).not.toBeNull();
  });

  it("does not move a suspended reader when new work arrives", () => {
    const view = renderScroller({ contentKey: "1" });
    scrollTo(120);

    view.rerender(
      <SubagentStreamScroller
        surfaceKey="env-1:thread-A:child-1"
        contentKey="2"
        streamState="working"
      >
        <p>the child said something</p>
        <p>and then something else</p>
      </SubagentStreamScroller>,
    );

    expect(viewport().scrollTop).toBe(120);
    expect(jumpToLatest()).not.toBeNull();
  });

  it("resumes following when the reader scrolls back to the bottom themselves", () => {
    renderScroller();
    scrollTo(120);
    expect(jumpToLatest()).not.toBeNull();

    scrollTo(LIVE_EDGE);

    expect(jumpToLatest()).toBeNull();
  });

  it("returns to the live edge and resumes following when jump-to-latest is used", () => {
    renderScroller();
    scrollTo(120);

    fireEvent.click(screen.getByRole("button", { name: "Scroll to end" }));

    expect(viewport().scrollTop).toBe(LIVE_EDGE);
    expect(jumpToLatest()).toBeNull();
  });

  /**
   * The criterion about switching tabs. Only the active right-panel surface is
   * mounted, so leaving a child tab unmounts its view: its place has to be kept
   * outside it, per surface, or every switch would drop the reader at the
   * bottom of a stream they were reading the middle of.
   */
  it("gives each child tab back its own place after the reader switches away", () => {
    const first = renderScroller({ surfaceKey: "env-1:thread-A:child-1" });
    scrollTo(120);
    first.unmount();

    // A sibling child, opened for the first time, starts at its live edge and
    // is following — it inherits nothing from the tab beside it.
    const second = renderScroller({ surfaceKey: "env-1:thread-A:child-2" });
    expect(jumpToLatest()).toBeNull();
    scrollTo(300);
    second.unmount();

    renderScroller({ surfaceKey: "env-1:thread-A:child-1" });
    expect(viewport().scrollTop).toBe(120);
    // And it is still suspended, so live work does not yank it to the bottom.
    expect(jumpToLatest()).not.toBeNull();
  });

  it("keeps a child tab's place across a visit to another surface kind", () => {
    const child = renderScroller({ surfaceKey: "env-1:thread-A:child-1" });
    scrollTo(180);
    child.unmount();

    // The reader opens a file tab, which mounts no child surface at all.
    renderScroller({ surfaceKey: "env-1:thread-A:child-1" });

    expect(viewport().scrollTop).toBe(180);
  });

  it("starts a child that was left at its live edge following again", () => {
    const first = renderScroller();
    scrollTo(LIVE_EDGE);
    first.unmount();

    renderScroller();

    expect(jumpToLatest()).toBeNull();
  });
});
