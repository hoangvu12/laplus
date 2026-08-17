# 05 — Make subagent tabs durable workspace citizens

**What to build:** Make child surfaces behave exactly like established right-panel workspace tabs while retaining child-specific replay and scroll state: several children can coexist with every existing surface, hide safely, restore after reload, and remain usable during live output.

**Blocked by:** 01 — Open an OpenCode child work stream

**Status:** ready-for-agent

- [x] Several child tabs can remain open together and coexist with files, diffs, terminals, previews, and plans.
      — `rightPanelStore.test.ts::keeps several child tabs among every other surface kind` opens three children among a terminal, a diff, a browser tab, a plan and the explorer and asserts the whole ordered list, then proves a file tab takes the explorer's place without disturbing them. `RightPanelTabs.test.tsx::sits among the other surface kinds in the order the workspace gives them` renders the same mixture and reads back each tab's accessible name in document order.
- [x] Opening an existing child activates its tab; opening another child adds one tab using the workspace's existing ordering and activation rules.
      — `rightPanelStore.test.ts::adds each new child at the end and never reorders one that is already open` is the ordering half; ticket 01's `opens a child work stream as a tab and activates it rather than duplicating` and `SubagentLauncher.test.tsx::activates the tab it already has rather than duplicating it` are the open-versus-activate half, through a real click.
- [ ] Child tabs use existing label, icon, close, context-menu, resizing, and narrow-layout conventions without bespoke status decoration.
      — **Label, icon, close, context menu and the absence of decoration are proven; resizing and narrow layout are not.** `RightPanelTabs.test.tsx` covers the label and its neutral fallback, click-to-activate, the shared `Close <label>` control, middle-click close, and a context menu of exactly `close / close-others / close-to-right / close-all` with no `copy-path` and nothing subagent-specific. `is drawn exactly like any other resource-addressed tab of the same name` renders a child tab and a terminal tab with one name and asserts the markup is identical once icons and generated ids are normalised — which is the no-decoration claim, and also means whatever the workspace does to a terminal tab it does to a child tab. **Debt for ticket 07:** resizing and the narrow layout are the panel shell's, and neither happens without layout; drive a real window at a narrow width and while dragging the splitter.
- [x] Closing a child tab removes only the surface and emits no interrupt, cancellation, detachment, or provider command.
      — `resolveRightPanelSurfaceCleanup` is the complete set of things closing a surface entails, split into the three fields that leave this window and one that does not. `rightPanelCleanup.test.ts::releases only the reader's place when a subagent tab is closed` asserts a child contributes to none of the three and that `cleanupReachesTheServer` is false; `closes a subagent beside a terminal without asking anything of the child` proves the terminal beside it is what reaches the server. `rightPanelStore.test.ts::closing a child tab hides only that surface` proves close-as-hide leaves neighbours alone and the row reopens the same address. Mutation-checked: making a child push its id into `terminalIds`, or counting the forgotten ids as server-facing, each turn these red.
- [x] Starting, updating, blocking, completing, or failing a child never opens or focuses the right panel automatically.
      — `SubagentAutoOpen.test.tsx` mounts a compact child row through the same composition `ChatView` performs and drives it through pending, working, blocked-on-approval and completed, then through failed, declined and stopped, then with two children at once, asserting the real store stays empty throughout — and finally that one click opens exactly one tab, so the row is not merely wired to nothing. Corroborated by `openSubagentSurface` having exactly one production caller, a click handler.
- [x] Open child tabs, their order, and the active selection restore with the parent thread after reload or restart, while full streams remain lazily loaded.
      — `rightPanelStore.test.ts::restores child tabs, their order and the active tab, carrying no stream with them` drives the store's real persistence: write, discard the page, rehydrate at the written version. Laziness is asserted as what was persisted — a restored child surface is exactly `{id, kind, resourceId}`, a reference with no stream attached, and nothing fetches until a surface mounts. `restores each thread's child tabs to that thread` and `does not bring back a child tab that was closed before the reload` are the scoping and the close-stays-closed halves. Mutation-checked: disabling `partialize` turns three of these red.
- [x] A restored child reference that cannot be resolved keeps an explicit unavailable surface instead of disappearing silently.
      — `rightPanelStore.test.ts::never prunes a child tab whose stream it cannot vouch for` runs both reconciliations that prune surfaces whose resource has gone and proves child tabs survive both; nothing in the store removes a child tab for being unresolvable. `SubagentStreamPanel.test.tsx::distinguishes loading, a child that has done nothing, and one that is gone` proves what that surviving tab then says (`data-subagent-state="unavailable"`).
- [ ] Every child tab preserves independent scroll position while the user switches among workspace tabs.
      — **The mechanism is proven; the restored position in a real viewport is not.** Only the active right-panel surface is mounted, so a tab switch unmounts a child's view and its place is kept in `subagentScroll`'s memory, keyed by `scopedThreadKey` plus the child. `subagentScroll.test.ts` proves the memory is independent per surface, scoped to the thread, bounded, and released when a tab closes; `SubagentStreamScroller.test.tsx::gives each child tab back its own place after the reader switches away` and `keeps a child tab's place across a visit to another surface kind` drive real mount/unmount cycles and assert the offset written back. **Debt for ticket 07:** happy-dom lays nothing out, so the restore writes `scrollTop` against fabricated metrics. In a browser the layout effect runs before markdown has reached its final height, so the restored offset can be silently clamped — drive a long child stream, scroll to the middle, switch tabs and back, and assert the same entry is under the cursor, not merely the same number.
- [ ] A live child follows new entries only while pinned to the bottom; manual scroll suspends following and exposes the existing jump-to-latest behavior.
      — **The decision and the wiring are proven; that a real gesture produces them is not.** `subagentScroll.test.ts` proves `isPinnedToBottom` exhaustively, including the slack, non-overflowing content and over-scroll. `SubagentStreamScroller.test.tsx` drives real scroll events and a real click through nine cases: follows while pinned, follows an entry that grows in place without the stream getting longer, does not move a suspended reader in either case, suspends on scroll-up and exposes the affordance, resumes when the reader returns to the bottom, and returns to the live edge on jump-to-latest. The affordance is literally the transcript's — `ScrollToEndButton` is one component used by both. **Debt for ticket 07:** the three viewport metrics are stated by the test, not measured, so what remains unproven is that a wheel or trackpad gesture reaches this viewport element and produces those numbers. Drive a running child, scroll up, watch new entries arrive without being pulled down, and click the pill.
- [ ] Focused right-panel state and rendering tests prove these behaviors without depending on private component structure.
      — **The quality half holds; the coverage half waits on the three above.** Every assertion is semantic output or store state: accessible names, rendered text, persisted values, and whole-markup comparison. The only DOM handles used are `data-slot` (this repo's public handle on a ui primitive's parts) and `data-active-tab` (the tab strip's own attribute). Tick this when 3, 8 and 9 are ticked, since it is a claim about the proof of all of them.

## Notes

`contentKey` was removed rather than corrected. The shell used to be told what
had changed — entry count, last entry id, the stream's `updatedAt` — and every
version of that key is wrong here. Child entries are **upserted by id**
(`applySubagentStreamItem`), so a command going from running to finished, or a
blocker resolved, grows an entry that already exists: the count does not move,
the last id does not move, and `updatedAt` arrives in a _separate_
`stream-updated` message from the `entry-upserted` that carried the growth —
one commit later, and at millisecond resolution (`clock.rs::now_iso`), so two
writes inside one millisecond do not move it at all. `SubagentStreamScroller`
now re-pins on every commit while following, which cannot be wrong about a
payload it does not own.

The scroll memory is session state and deliberately not persisted: a restored
child tab replays lazily and opens at its live edge.

`ScrollToEndButton` was extracted from `ChatView` rather than imitated, because
the spec asks a child surface to expose "the existing jump-to-latest
behavior" — and two copies of a pill are the same affordance only until someone
edits one of them.

Ticket 07 owns the browser-driver acceptance run. The three unticked criteria
above each carry the specific scenario that would close them.
