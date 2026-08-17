# 01 — Open an OpenCode child work stream

**What to build:** Deliver the first complete inspectable subagent work stream using OpenCode: capture a child's prose and terminal result independently from the parent transcript, persist it, expose lazy replay and live continuation through orchestration, and open it from the compact inline row as a read-only right-panel tab using the main agent's presentation.

**Blocked by:** None — can start immediately

**Status:** ready-for-agent

- [x] A newly started OpenCode child has stable identity, assignment, lifecycle state, stream reference, and an ordered stream containing its prose and terminal result.
      — `socket_opencode_turn::an_opencode_child_work_stream_replays_and_then_continues_live` asserts `childId`, `name`, `assignment` and `state: "working"` on the opening snapshot, and the ordered entries `[(1, message), (2, message), (3, outcome)]`. The compact row carries the stream reference (`data.childId`).
- [x] The parent transcript retains one compact child row and does not contain the child's detailed prose as ordinary parent messages.
      — same test: the thread snapshot's `messages` contain none of the child's prose, and every child-related activity is the one `subagent:call_task_1` row. `session-logic.test.ts::keeps a subagent row's work-stream reference through collapse` proves the several row updates collapse to one.
- [ ] Clicking the compact row opens a normal resource-addressed child tab in the existing right-panel workspace; clicking it again activates the same tab rather than duplicating it.
      — **Partly proven; deliberately not ticked.** `rightPanelStore.test.ts::opens a child work stream as a tab and activates it rather than duplicating` proves open-versus-activate and the resource address. `MessagesTimeline.logic.test.ts::resolveWorkEntryActivation` proves the row launches the right child rather than expanding, and `MessagesTimeline.test.tsx::makes a subagent row a launcher into its work stream` proves the affordance appears only when there is somewhere to open it. **Missing:** `ChatView`'s `openSubagentSurface` callback → store call has no test, and nothing performs an actual click. Ticket 07's browser scenario is what closes this.
- [x] The child tab is read-only, has no additional identity/task header or composer, and renders child prose and result using the main agent's existing message/work language.
      — `SubagentStreamPanel.test.tsx` (8 tests): prose renders through `ChatMarkdown` (emphasis and lists survive); the conclusion renders through the shared `SimpleWorkEntryRow`, evidenced by that component's own `aria-label="Tool call failed"`; the surface carries no `<textarea>`, `<input>`, `<form>`, `contenteditable`, `<button>` or `role="button"`; and it renders neither the child's name, nor its assignment, nor its id, nor any heading, though the stream it is given carries all of them.
- [ ] Closing the tab only hides it and does not stop or alter the child; clicking the compact row reopens the same stream.
      — **Partly proven; deliberately not ticked.** `socket_opencode_turn::closing_a_child_surface_does_not_stop_the_server_recording_it` unsubscribes from a child that is still working, lets it finish unwatched, and proves the reopened stream has more entries and reached its conclusion. `rightPanelStore.test.ts::closing a child tab hides only that surface` proves close-as-hide and that reopening lands on the same surface id. **Missing:** the same click wiring as the criterion above.
- [x] Opening the tab lazily replays persisted entries and continues with live entries without loss, duplication, or ordering changes at the replay/live boundary.
      — `socket_opencode_turn::an_opencode_child_work_stream_replays_and_then_continues_live` opens the subscription while the child is still working, releases the scripted peer, folds snapshot and live events the way a client does, and asserts the three entries in order — _and_ that the wire carried exactly those three entry ids, so the claim is not merely that the client's fold is idempotent. `subagentStream.test.ts` proves the fold itself: upsert by id, order by sequence.
- [x] Reloading after child completion replays the same complete stream and terminal result.
      — same test, via a fresh connection that watched none of it. `socket_opencode_turn::a_child_work_stream_replays_after_the_server_restarts` proves it across a real process restart over one database file (mutation-checked: disabling `Shell::new`'s restore makes it fail). `store.rs::a_child_work_stream_survives_the_disk_and_stays_out_of_the_transcript` proves the round trip.
- [x] Ordinary parent-thread snapshots carry only the compact child index rather than the complete child stream.
      — `an_opencode_child_work_stream_replays_and_then_continues_live` asserts the snapshot's child activities are all the one compact row, carry no `entries`, and that the thread grew no child-stream field. The same store test asserts `Database::conversations()` is unchanged by any of it.
- [x] Deleting the parent thread deletes the recorded child stream.
      — `socket_opencode_turn::deleting_the_parent_thread_deletes_its_child_work_stream`: readable before the delete, refused with `OrchestrationGetSnapshotError` after. The store test proves the rows go from disk too.
- [x] Existing OpenCode socket-provider tests prove the behavior through the orchestration boundary, with focused contract, client-state, and rendering tests supporting that external seam.
      — socket: the five tests above in `socket_opencode_turn`. Contract: `packages/contracts/src/orchestration.test.ts`. Client-state: `packages/client-runtime/src/state/subagentStream.test.ts` and `apps/web/src/rightPanelStore.test.ts`. Rendering: `apps/web/src/components/chat/SubagentStreamPanel.test.tsx` and `MessagesTimeline.test.tsx`.

## What is left

Two criteria are unticked for one reason: **nothing in this branch performs a
click, and `ChatView`'s `openSubagentSurface` callback has no test of its own.**
Everything on either side of that link is proven — the row decides to launch,
the store opens or activates — but the link itself is not. Ticket 07 owns the
browser-driver scenario that closes it; a cheaper alternative would be a focused
test of the `ChatView` callback.

`cargo fmt --check` is not run for this branch. `server/CLAUDE.md` records that
this tree has never been rustfmt-formatted and that the check fails on every
file; reformatting would touch 29 files unrelated to this work.
