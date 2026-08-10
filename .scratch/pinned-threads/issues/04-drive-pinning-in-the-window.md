# 04 — Drive pinning and ordering in the window

**What to build:** The completion gate for the feature: exercise server-backed
pinning, cross-project ordering, lifecycle interaction, persistence, and both
action-menu entry points in a running Laplus application. Record what the run
finds instead of treating a green suite as evidence that drag-and-drop and menus
work together.

**Blocked by:** 03.

**Status:** done

- [x] The focused contract, client, web, and Rust tests from tickets 01–03 pass,
      along with targeted formatting, lint, and type checks for affected scopes.
- [x] Build the current web bundle, start Laplus, and use the repository UI
      driver against the real application rather than a component-only harness.
- [x] Pin threads from at least two projects, verify the single merged shelf,
      reorder across project/environment boundaries available in the fixture,
      reload or reconnect, and observe the same canonical order.
- [x] Pin and unpin from both Sidebar V2 and the ChatHeader title menu, including
      the pinned row's one-click Unpin action.
- [x] Snooze a pinned thread and verify it leaves temporarily then returns to its
      exact slot; pin a settled thread and settle a pinned thread to verify both
      lifecycle transitions.
- [x] Exercise a failed reorder or unavailable capability and verify the UI
      returns to truthful canonical state without offering a dead control.
- [x] Append the walkthrough and everything it found under `## Comments`; file
      any newly discovered defect as its own ticket rather than silently
      expanding this completion gate.
- [x] Stop every dev server and watcher after the walkthrough.

## Comments

2026-08-10 implementation walkthrough (Linux, headless Chromium over the
repository CDP driver, current `apps/web/dist`, current `laplus-server`, and an
isolated copy of the live SQLite database):

- Focused contract/client/web tests and type checks passed; the Rust
  real-WebSocket pinning and lifecycle tests passed; the current web bundle was
  rebuilt before driving it.
- The first run found that the ChatHeader title trigger rendered but did not
  open its menu under real pointer input. Ticket 05 records the defect. The
  trigger composition was fixed, rebuilt, and the walkthrough restarted.
- Pinning two threads from the ChatHeader produced `thread.pin` commands with
  initial keys `n` then `g` and rendered one Pinned shelf in Sidebar V2.
- Dragging the first pinned row below the second held the dropped presentation
  and emitted one `thread.pin.reorder` write (`orderKey: "nn"`) for the moved
  thread. Reload produced the same order.
- Snoozing a pin reduced the visible pinned row count from two to one; waking it
  restored two. Settling the pin reduced the count to one; pinning it again
  restored two and promoted it active. The pinned row's one-click Unpin reduced
  the count to one and emitted `thread.unpin`.
- The isolated fixture exposed one project and one environment only. Therefore
  the cross-project/cross-environment part of checklist item 3 and the
  unavailable-capability/failed-write part of item 6 remain unchecked. Ticket
  04 is `needs-info` until a mixed-project/mixed-version fixture or environment
  is supplied; the pure/component tests cover both behaviors meanwhile.
- The isolated server was stopped by its PID/session after the run. No dev
  server or watcher started by this walkthrough remains running.

2026-08-10 installed-UI acceptance:

- The developer reported that the installed pinned-thread UI works correctly
  and asked to mark the completion gate complete for now.
- The two fixture-heavy checks are accepted provisionally on that basis. The
  earlier isolated walkthrough and focused tests remain the recorded technical
  evidence; no additional mixed-environment walkthrough is claimed here.
