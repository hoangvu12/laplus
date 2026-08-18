# Inspectable subagent work streams

Status: ready-for-human

## Problem Statement

Laplus currently represents a subagent as a compact work-log row such as
`Subagent explore`. The row can expose a changing description or final summary,
but it does not let the developer enter the child agent's work and see what it
is actually doing. The developer cannot inspect the child's ordered prose,
commands, reads, edits, tool calls, errors, blockers, and result as one durable
session. With several children running, the current presentation answers only
that a subagent exists; it does not answer what each child did or how it reached
its conclusion.

The missing view is especially awkward because Laplus already has the right
interaction language: the main agent has a rich transcript and the right-panel
workspace has browser-style tabs for files, diffs, terminals, previews, and
plans. Subagent work is flattened instead of participating in those existing
surfaces.

## Solution

Give every newly recorded subagent an ordered, persisted **subagent work
stream**. Keep the existing compact inline row in the parent transcript as the
summary and launcher. Clicking it opens or activates that child as a normal tab
in the existing right-panel workspace.

The child tab is read-only and opens directly into the work stream. It reuses
the main agent's message and work-entry presentation for prose, commands, file
activity, edits and diffs, tool calls, errors, blockers, and the terminal
result. It adds no bespoke header, composer, selector, status decoration, or
tab behavior. Multiple child tabs can coexist with each other and with the
workspace's existing surfaces.

Child events are stored independently from the parent transcript and loaded
lazily when a child tab opens. The parent retains only the compact child index
needed to render the inline row. The server continues recording a child while
its tab is closed, and the complete stream remains replayable for the lifetime
of the parent thread.

Claude, Codex, and OpenCode are all included in the first releasable version.
They share one product model while preserving provider-specific richness and
degrading honestly when a protocol does not expose a field or relationship.

## User Stories

1. As a developer, I want to click a subagent row and inspect that child's work, so that I understand what it is actually doing.
2. As a developer, I want subagent work to open in the existing right-panel workspace, so that the interaction feels native to Laplus.
3. As a developer, I want a subagent to open as a normal browser-style tab, so that I can use the same workspace behavior I already understand.
4. As a developer, I want clicking an already-open subagent to activate its existing tab, so that duplicate tabs are not created for one child.
5. As a developer, I want several subagent tabs to remain open together, so that I can move between parallel workers.
6. As a developer, I want subagent tabs to coexist with files, diffs, terminals, previews, and plans, so that inspecting a child does not take away the rest of my workspace.
7. As a developer, I want a child tab to use the same message presentation as the main agent, so that child prose is immediately readable.
8. As a developer, I want a child tab to use the same work-entry presentation as the main agent, so that commands and tools do not become an unfamiliar raw log.
9. As a developer, I want to see the child's commands and their status and output, so that I can understand what it executed.
10. As a developer, I want to see the child's file reads and searches, so that I can understand the evidence it examined.
11. As a developer, I want to see the child's edits and open their diffs, so that I can review the work it changed.
12. As a developer, I want child file and diff actions to open neighboring right-panel tabs, so that the child tab remains available while I inspect an artifact.
13. As a developer, I want to see the child's other tool calls and results, so that its work is auditable beyond shell commands.
14. As a developer, I want to see child warnings and errors in chronological context, so that I can diagnose a failed approach.
15. As a developer, I want the child's final result to be the terminal entry in the same stream, so that the conclusion remains connected to the work that produced it.
16. As a developer, I want a clear terminal reason when a child fails or is interrupted, so that stale activity is not mistaken for an outcome.
17. As a developer, I want a clear empty outcome when a child completes without a textual result, so that I know the stream is complete rather than broken.
18. As a developer, I want the child tab to be read-only, so that inspecting work cannot accidentally steer a provider that does not support child interaction.
19. As a developer, I want the child view to open directly into its work, so that a duplicate identity or task header does not consume panel space.
20. As a developer, I want the existing tab label conventions to apply to subagents, so that this feature does not introduce a second tab language.
21. As a developer, I want closing a child tab to hide only the view, so that closing presentation never cancels work.
22. As a developer, I want a closed child tab to remain reopenable from its inline row, so that I can return to it at any time.
23. As a developer, I want subagent tabs to open only after an explicit click, so that background delegation never steals focus.
24. As a developer, I want open child tabs and their order to survive reload or restart, so that my right-panel workspace remains stable.
25. As a developer, I want the previously active child tab to be restored, so that I return to the work I was inspecting.
26. As a developer, I want an unavailable restored child to show an explicit unavailable state, so that a tab never disappears silently.
27. As a developer, I want a running child tab to follow new entries while I am at the bottom, so that I can watch live progress.
28. As a developer, I want live following to stop when I scroll upward, so that new events do not pull me away from older work.
29. As a developer, I want a jump-to-latest affordance when new child work arrives above my current scroll position, so that I can return to live output deliberately.
30. As a developer, I want each child tab to preserve its own scroll position, so that switching tabs does not lose my place.
31. As a developer, I want the complete child stream to replay after completion, so that subagent work remains reviewable rather than ephemeral.
32. As a developer, I want the complete child stream to replay after an application restart, so that inspection is not limited to the live run.
33. As a developer, I want subagent work retained for as long as its parent thread exists, so that historical child rows do not point to expired work.
34. As a developer, I want deleting the parent thread to delete its child streams, so that delegated work follows the conversation's lifecycle.
35. As a developer, I want old threads to load without eagerly fetching every child stream, so that retained subagent history does not slow ordinary conversation navigation.
36. As a developer, I want the server to record children while their tabs are closed, so that choosing not to watch live does not create gaps.
37. As a developer, I want the inline parent row to show the child's identity or type, so that I can distinguish parallel workers.
38. As a developer, I want the inline parent row to show the assignment, so that I know why the child exists before opening it.
39. As a developer, I want the inline parent row to show the child's current state, so that I know whether it is pending, working, blocked, completed, interrupted, or failed.
40. As a developer, I want the inline parent row to show the latest meaningful activity while the child runs, so that I can scan progress without opening every tab.
41. As a developer, I want noisy partial tokens and heartbeat events omitted from the compact row, so that latest activity remains meaningful.
42. As a developer, I want terminal state to replace stale activity with a bounded result or failure preview, so that the row describes what came back.
43. As a developer, I want detailed child entries to remain out of the parent transcript, so that parallel workers do not interleave into an unreadable conversation.
44. As a developer, I want a nested subagent to appear inside the work stream of the child that launched it, so that delegation hierarchy remains truthful.
45. As a developer, I want clicking a nested child's row to open another normal child tab, so that nested work is as inspectable as direct work.
46. As a developer, I do not want nested children duplicated in the root transcript, so that one worker has one visible parent.
47. As a developer, I want Laplus to preserve only hierarchy the provider can prove, so that generated relationships are never presented as fact.
48. As a developer, I want a child-owned approval or question recorded in that child's stream, so that its history explains why it waited.
49. As a developer, I want actionable child approvals and questions surfaced through the main conversation's existing controls, so that blockers cannot hide in an inactive tab.
50. As a developer, I want the actionable request to identify the waiting child, so that I understand who will receive my response.
51. As a developer, I want my response routed back to the originating child request, so that the correct worker continues.
52. As a developer, I want providers without child-attribution metadata to retain their truthful root behavior, so that Laplus does not invent ownership.
53. As a developer, I want stopping the parent to stop its delegation tree, so that children do not silently continue editing after I stop the work.
54. As a developer, I want each affected child stream to record interruption, so that the final state remains auditable.
55. As a developer, I want closing a tab to remain distinct from stopping work, so that common workspace cleanup is safe.
56. As a developer, I want the thread to remain Working while any descendant is active, so that the sidebar does not claim an active delegation tree is idle.
57. As a developer, I want a background child to keep updating after the parent becomes temporarily quiet, so that its independent lifecycle stays visible.
58. As a developer, I want the parent agent to continue normally when children finish, so that completion does not require manual coordination.
59. As a developer, I do not want a toast for every child completion, so that parallel work does not create notification bursts.
60. As a Claude user, I want inspectable child work, so that this feature is not limited to one provider.
61. As a Codex user, I want inspectable child work and preserved agent hierarchy where available, so that Codex's richer identities are not flattened.
62. As an OpenCode user, I want the complete child session and descendant blockers preserved, so that Laplus reflects the events OpenCode already emits.
63. As a developer using several providers, I want the same basic child-tab interaction everywhere, so that switching providers does not require learning another UI.
64. As a developer using a richer provider, I want Laplus to preserve the detail it exposes, so that cross-provider consistency does not mean lowest-common-denominator data loss.
65. As a developer using a less expressive provider, I want absent details omitted honestly, so that the interface never fabricates work.
66. As a developer, I want newly captured child work to use the new model without requiring historical migration, so that the feature can ship without unreliable transcript reconstruction.

## Implementation Decisions

- Introduce a provider-neutral child-stream model with stable child identity,
  optional parent identity or canonical path, semantic name or type, assignment,
  lifecycle state, ordered stream entries, terminal outcome, and provider-owned
  request identity where applicable.
- A stream entry has stable event identity and ordering, a timestamp, a content
  kind, and content appropriate to the kind. The shared kinds cover child prose,
  commands and output, reads and searches, edits and diffs, other tool calls and
  results, warnings and errors, approvals or questions, and terminal outcomes.
- Provider adapters normalize only what their protocols expose. Unknown future
  event variants remain forward-compatible and observable through the existing
  drift policy rather than breaking the child or parent turn.
- Route provider events by stable root and child identities before ordinary
  parent-conversation folding. Root events continue to update the parent;
  child events update the child's stream and, where appropriate, the compact
  parent index row.
- Keep child turn and thread boundary events out of the parent turn lifecycle.
  A child completion must not incorrectly settle or clear the root turn.
- Store child streams independently from the parent transcript. The parent
  persists only the lightweight child index required for the inline launcher:
  identity, assignment, state, latest meaningful activity or terminal preview,
  hierarchy reference, and stream reference.
- Persist complete ordered child streams for the lifetime of the parent thread.
  Removing the parent removes its child index and streams.
- Expose a lazy child-stream read/replay operation and a live update mechanism
  through the existing orchestration boundary. Replay and live continuation
  must meet without event loss or duplication.
- Load a full child stream only when its right-panel surface is open. Closing
  the surface releases the client view but does not stop server capture.
- Add subagent as a resource-addressed right-panel surface. Its identity is
  stable per child, so opening the same child activates the existing surface;
  opening another child creates another normal tab.
- Reuse the right-panel workspace's existing ordering, activation, closing,
  context-menu, resizing, narrow-layout, and persistence semantics. Do not add
  a separate inspector container or singleton subagent selector.
- Reuse the main agent's transcript and work-entry components in a read-only
  configuration. The child surface has no pinned header and no composer.
- Reuse existing tab styling and labeling conventions. Do not add bespoke
  running, completed, or failed decoration to the tab itself in this version.
- Keep the inline parent row as the identity, assignment, status, latest
  activity, and terminal-preview surface. Clicking the row opens or activates
  the child's right-panel surface.
- Derive latest activity from the latest meaningful displayable child entry.
  Coalesce streaming prose and ignore transport noise, heartbeat events, and
  partial states that do not communicate progress.
- When a child becomes terminal, replace latest activity atomically with a
  bounded result, failure, interruption, or empty-result preview. Preserve the
  complete terminal entry in the child stream.
- Preserve one independent scroll/follow state per open child surface. Follow
  live entries only while pinned to the bottom; scrolling up suspends following
  and enables the existing jump-to-latest interaction.
- Starting or updating a child never opens the right panel. Opening is caused
  only by an explicit click on an inline launcher.
- Closing a child surface is presentation-only. It cannot send interrupt,
  cancellation, detachment, or provider lifecycle commands.
- Restore open child surfaces, their order, and active selection using the
  existing thread-scoped right-panel persistence. Replay content lazily after
  restoration. Preserve an explicit unavailable state when a persisted child
  reference cannot be resolved.
- Place a nested child's inline launcher inside the spawning child's stream
  when provider identity proves the relationship. Do not duplicate descendants
  into the root transcript and do not infer missing parentage.
- Child-owned approval and question entries are part of the child stream. The
  actionable response remains in the main conversation's existing request UI,
  identifies the waiting child, and routes the decision through the child's
  provider request identity.
- The initial Stop behavior applies to the whole delegation tree. Every affected
  child records a terminal interruption. Closing a tab never participates in
  this behavior.
- Per-child Stop, steering, and composing are capability-gated future features,
  not part of the read-only surface.
- Derive thread Working state from active work anywhere in its known delegation
  tree. A quiet or settled root does not make the thread idle while a descendant
  remains active.
- Child completion updates the inline row and parent orchestration normally but
  creates no additional toast or desktop notification.
- Claude, Codex, and OpenCode are all release requirements. Provider-specific
  implementation slices may land incrementally, but the product is not complete
  until all three satisfy the shared behavior with honest omissions.
- Do not migrate or synthesize full streams for historical subagent rows. The
  new guarantee starts when the child-stream model begins recording them.

## Testing Decisions

- Prefer externally observable behavior over adapter internals. A good test
  drives recorded or scripted provider events through the server's normal
  orchestration boundary and asserts what a client can fetch, subscribe to, and
  render. It does not assert private helper calls or storage layout.
- The primary seam is the existing WebSocket orchestration boundary. Exercise
  the same child-stream contract with the established scripted Claude, Codex,
  and OpenCode provider harnesses.
- Extend the existing Claude background-subagent scenario to prove ordered
  prose and work capture, post-root child activity, result persistence, replay,
  interruption, and Working state.
- Extend the existing Codex collaboration scenario to prove stable child
  identity, canonical hierarchy, separate parent-operation and child lifecycles,
  ordered replay, nesting where captured, interruption, and truthful terminal
  outcomes.
- Extend the existing OpenCode child-session scenario to prove child prose,
  commands, reads, edits, tool calls, result, lazy replay/live continuation, and
  parent/child session routing.
- Add an OpenCode child permission/question scenario. Prove that the child entry
  is persisted, the main conversation receives an actionable request identifying
  the child, the response uses the provider's child request identity, and the
  child stream records resolution.
- At the socket boundary, prove replay followed by subscription cannot lose or
  duplicate an event at the handoff, including reconnect and reload cases.
- At the socket boundary, prove child turn boundaries never settle the parent,
  and prove the thread remains Working until the final active descendant is
  terminal.
- At the socket boundary, prove stopping the parent records interruption for
  every known active descendant and stops further live child activity.
- At the socket boundary, prove deleting a parent removes its child streams and
  that ordinary parent snapshots do not eagerly carry complete child histories.
- Extend the existing right-panel state tests to cover resource-addressed child
  surfaces: open versus activate, multiple children, ordering, close-as-hide,
  persistence, restoration, unavailable resources, and coexistence with all
  existing surface kinds.
- Extend the existing timeline derivation tests to cover compact child rows,
  meaningful latest-activity selection, terminal replacement, nested launchers,
  and no duplication of detailed child entries into the root transcript.
- Test the shared read-only transcript/work rendering with representative child
  prose, commands, file operations, diffs, tools, errors, blockers, and terminal
  outcomes. Assert semantic output and interactions rather than component
  implementation structure.
- Test independent scroll state, sticky live following, suspended follow after
  manual scroll, jump to latest, and restoration when switching child tabs.
- Add one focused browser-driver acceptance scenario against a running Laplus.
  It must click an inline child, observe a normal right-panel tab and live rich
  work, open a file or diff beside it, switch tabs, close and reopen the child
  without stopping it, and reload to verify restored tabs and replay.
- The browser scenario is required because a green socket suite cannot prove
  that the window exposes child work correctly. Stop its server and browser
  processes after the focused run.
- Keep verification focused: run the affected provider integration binaries,
  targeted contract/client/UI tests, type checks and formatting, then drive the
  browser scenario. The full workspace suite remains CI's responsibility.

## Out of Scope

- Sending messages directly to a subagent.
- A composer inside the child surface.
- Per-child Stop, resume, steer, close, or other lifecycle controls.
- Allowing children to continue after the developer stops the parent delegation
  tree.
- Automatically opening a child surface when the child starts, updates, blocks,
  completes, or fails.
- New tab styling, badges, or status decoration specific to subagents.
- A pinned child identity or assignment header inside the child surface.
- A singleton Subagents panel with an internal selector.
- A dedicated full-screen child route as the default desktop interaction.
- Tiling or comparing several child transcripts outside the existing right-panel
  tabs.
- A separate fleet-management dashboard.
- Toasts or desktop notifications for individual child completion.
- Duplicating full child work into the parent transcript.
- Inventing hierarchy, activity, results, blockers, or provider capabilities
  that the source protocol does not expose.
- Migrating or reconstructing child streams for historical subagent rows.
- Retaining child streams after their parent thread is deleted.
- Changing the existing parent transcript, file, diff, terminal, preview, plan,
  or right-panel tab interactions beyond adding child surfaces and their links.

## Further Notes

- The agreed product vocabulary defines a **subagent** as a delegated child and
  a **subagent work stream** as its ordered, replayable conversation and work.
  Avoid describing the child as merely a tool call or its stream as only a
  progress log or result view.
- The comparative research establishes OpenCode's live inspector, Devin's
  enterable child sessions, and graphical peer-session layouts as primary-source
  references. The selected design intentionally follows Laplus's existing
  browser-style right-panel tabs rather than copying another product's container.
- OpenCode proves that descendant permission and question requests can occur and
  that actionable blockers should surface at the active parent. Laplus currently
  receives those child envelopes but retains only child prose, so this feature
  necessarily includes adapter and data-model work rather than UI work alone.
- The throwaway visual prototype settled the container question: an inline row
  opens an ordinary right-panel child tab beside file and diff tabs. The
  prototype is evidence for the decision, not production code to promote.
- This is a multi-session, cross-contract feature. The next step is to split the
  spec into tracer-bullet tickets with explicit blocking edges before beginning
  implementation.

## Feature-wide review and verification

Ran after the seventh ticket merged, against base `8ca1365d` and head
`0f6e46d9` — over the composed feature rather than over any one ticket's base,
because each ticket was reviewed only against its own and what nobody had
checked was the interaction.

`/code-review`'s two axis sub-agents were launched and never returned a report,
so the findings recorded here and in ticket 07 were located and verified
directly rather than taken from them. **Re-running the two axes is outstanding**
and should happen before the release; this review's coverage is what one reader
reached, not what two independent axes would.

**Status is deliberately not `ready-for-human`.** The review did not come back
clean. Its one blocker has since been fixed — see below — but the review's own
two axes were never re-run.

### The blocker — fixed

**A stopped child's compact row contradicted its own stream, and then reported
the answer the developer declined to wait for.** `Shell::stop_the_delegation_tree`
reached only `Streams::interrupt` and `Threads::follow_delegation`; every
compact-row emitter lives in a provider fold path, so a Stop drew no row and
nothing refused one afterwards. The row stayed `running` with its pre-stop
detail, and then the provider's continued narration — which `Streams::record`
correctly refuses for the stream — carried the row to `completed` with the
child's report on it, beside a stream that said `interrupted`.

Ticket 02 made the compact row the terminal-preview surface; ticket 06 added a
new terminal path that bypasses it and asserted only the stream. Neither ticket
was wrong about itself.

**Fixed in two halves, both provider-neutral.** `Streams::interrupt` now answers
with what it actually ended, and `stop_the_delegation_tree` draws each stopped
child's terminal row from that answer — `worklog::child_row_key` is the one
place the row's provider-specific collapse key is spelled, and a descendant
(which owns no root row) is deliberately given none. And `session::spend`, the
choke point where every provider's transcript activities are applied, refuses an
activity that would move the row of a child `Streams` holds as interrupted;
it recognises one by `data.childId`, the stream reference every driver's compact
child row already carries, so it needs no provider knowledge at all.

Neither half weakens `Streams::record`: ticket 06 criterion 7 is proven by the
same tests it always was.

Proven for all three providers, each mutation-checked:

```
cargo test -p laplus-server --test socket_turn a_stopped_claude_child_row_agrees_with_the_stream_it_belongs_to
cargo test -p laplus-server --test socket_opencode_turn stopping_the_parent_stops_its_delegation_tree
cargo test -p laplus-server --test socket_codex_turn stopping_a_codex_parent
```

The Claude test is no longer `#[ignore]`d. It also no longer asserts
`payload.status == "interrupted"`, which was wrong: `status` is the client's
`WorkLogToolLifecycleStatus`, whose literals do not include that word, and a
`tool.completed` carrying an unreadable status is drawn as _completed_. The row
says `stopped`, which is the mapping the Codex driver already made.

The full account, the two fixes the review itself took, and everything it
recorded without acting on, are in ticket 07 under **What the feature-wide
review found**.

### What was run, and what it said

All numbers below are real output from the integration worktree, with
`CARGO_TARGET_DIR=/tmp/laplus-target-integration` — per-worktree, because a
shared target directory silently served one worktree's test binaries to another
earlier in this run.

- `cargo test -p laplus-server --no-fail-fast` — at review time **1520 passed, 0
  failed, 3 ignored**, against a 1520/0 baseline. The third ignored test was
  `a_stopped_claude_child_row_agrees_with_the_stream_it_belongs_to`, added by
  this review to hold the blocker above; the fix un-ignores it, so the suite is
  now **1521 passed, 0 failed, 2 ignored** — `local_generation_peer_child` and
  `opencode_peer_child`, both pre-existing.
- `vp run -r test` — all six projects green: `packages/shared` 4 files / 35
  tests, `packages/contracts` 23 / 203, `packages/client-runtime` 36 / 424,
  `apps/web` 186 / 1715.
- The feature's focused files, run by name: `apps/web` 8 files / 156 tests
  (`SubagentStreamPanel`, `SubagentNesting`, `SubagentStreamScroller`,
  `subagentScroll`, `rightPanelStore`, `rightPanelCleanup`, `session-logic`,
  `subagentFileActions`); `contracts/src/orchestration.test.ts` 49 tests;
  `client-runtime/src/state/subagentStream.test.ts` 5 tests.
- `vp run -r typecheck` — clean across all 6 projects (`apps/cli`, `apps/web`,
  `packages/contracts`, `packages/shared`, `packages/client-runtime`,
  `oxlint-plugin-t3code`).
- `vp lint` — **11 warnings**, the baseline, none of them in this feature's
  files. (`vp lint --report-unused-disable-directives`, which the `lint` package
  script adds, reports a twelfth in `ThreadTerminalDrawer.tsx`, untouched here.)
- `cargo clippy -p laplus-server --all-targets` — **76 warnings**, the baseline.
- `cargo fmt` was **not** run and `cargo fmt --check` was **not** run.
  `server/CLAUDE.md:169` records that this tree has never been rustfmt-formatted
  and that the check fails on all 29 files.

### What none of that proves

**No browser run happened.** Not a single line of this feature has been drawn in
a window. That was the user's decision — they will drive the UI themselves — and
it is the largest open risk, not an oversight. AGENTS.md's warning applies
directly: a whole afternoon's findings once came from driving the window for a
minute, none of which a passing suite had caught.

Concretely:

- **Tickets 05 and 07 retain browser-only acceptance criteria.** Ticket 05's
  criteria for resizing, the narrow layout, real scroll restoration against real
  metrics and a real jump-to-latest gesture cannot be met by happy-dom, which
  lays nothing out. Ticket 07's criteria 1–6 are deliberately left unticked and
  the ticket stays `ready-for-agent` for exactly this reason.
- **Ticket 07 carries the human checklist for them**, under **The browser gap**:
  H1 (the held-Working state and the four things it drives — do this first), B1–B6,
  N1 (a nested launcher clicked inside an open child tab), S1 (Stop with the tree
  live), R1 (resizing and the narrow layout) and C1 (blockers, OpenCode only).
  Ticket 07 also records which of its own leads the feature-wide review has since
  settled statically, so the window time is not spent on them.

The honest summary: the feature is implemented, reviewed and green at every seam
a test can reach — including the one seam a test _did_ reach and find broken,
now fixed and proven for all three providers — and nobody has yet opened the
window.

### The two axes, re-run

The first feature-wide pass reported Standards and Spec findings it had not
received — its two axis sub-agents were launched and never returned, and it
retracted that framing itself. Both axes were then re-run properly and their
reports read directly. What follows is that second pass.

**Fixed from the Standards axis.** Two doc comments asserted things that were
not true: `subagents::bounded` claimed the Claude driver's copy "was retired by
the feature-wide review" while `turn::bounded` was still there, and
`worklog::subagent_row_key` counted "a **third** caller" where there are two.
Both now say what is so. The axis also read three `bounded`s as duplication;
they are not — each driver's wrapper holds only its own wire's rule about
_absence_ (whitespace-only output for Claude, a null-or-non-string `Value` for
OpenCode) and both delegate the bound itself. That is now written down where it
was previously left to be inferred.

**Fixed from the Spec axis — a nested launcher could show a descendant's task as
its answer.** `launcherWorkEntry` read `outcome?.text ?? assignment`, and
`Outcome::interrupted(None)`, `failed(None)` and `completed(None)` all carry
`text: null` — so a descendant that ended silently rendered the assignment it
had been given before it began. That is precisely the stale activity story 42's
terminal rule displaces, on ticket 06's new surface, bypassing the shared
sentence (`OutcomeKind::without_a_report`) that exists to prevent it. Now routed
through `OUTCOME_LABELS`, so a descendant reads the same in its parent's stream
as in its own tab. Proven by three cases in `SubagentNesting.test.tsx`, each
asserting the assignment's _absence_ as well as the sentence's presence, and
mutation-checked against the original expression.

### Known drift, recorded rather than changed

These are real and none is a defect in behaviour. Each was left alone because
the feature was one step from a publish and the change is wider than the finding.

- **The contract inverts the glossary's two stream words.**
  `OrchestrationSubagentStream` is the _head_ and `OrchestrationSubagentSnapshot`
  is the stream, while `CONTEXT.md` defines a **subagent work stream** as the
  conversation _and_ its work and **stream head** as that without it.
  `subagents.rs`'s `Head::to_value` makes the inversion explicit. The rename
  wants contracts, the Rust server and the client moving together.
- **Story 38 — the row stops showing the assignment once the child speaks.**
  Both row builders rank latest activity above the description, so "why does
  this child exist" is legible only before its first activity. It falls between
  ticket 01 (asserts assignment on the head) and ticket 02 (owns the row's
  detail and deliberately ranks activity first); no ticket carries a criterion
  for it.
- **Story 39 — `pending` and `blocked` are unreachable on a compact row.**
  `WorkLogToolLifecycleStatus` has no such members, and a child that blocks
  emits its request row without re-emitting its own compact row, so the row
  keeps its pre-block detail. The blocker is still actionable in the
  conversation, which is what ticket 02's criterion 8 proves.
- **Story 40 — a nested launcher has no latest-activity field.** `Launcher`
  carries identity, assignment, state and outcome, so a running descendant shows
  its assignment until it ends. Structural rather than a render bug.
- **`EntryKind::Subagent` names what the glossary calls a nested launcher**, and
  `child_entry_kind` is written once per driver. Both were deliberate — the
  first so that all nine variants match their wire literal exactly, the second
  with the repo's own precedent (`worklog::opencode_item_type` beside
  `worklog::Kind::of`).
