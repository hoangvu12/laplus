# 07 — Drive the complete cross-provider experience

**What to build:** Prove and finish the complete user-visible feature across Claude, Codex, and OpenCode by driving a running Laplus through live child inspection, neighboring workspace surfaces, safe tab lifecycle, reload, and replay; remove the throwaway mockup once the real interaction is established.

**Blocked by:** 02 — Show OpenCode's complete child work and blockers; 03 — Show Claude subagent work streams; 04 — Show Codex subagent work streams and hierarchy; 05 — Make subagent tabs durable workspace citizens; 06 — Treat the delegation tree as active work

**Status:** ready-for-agent

- [ ] One focused browser-driver scenario clicks a compact child row and observes a normal right-panel child tab displaying live prose and work through the shared main-agent UI.
      — **Not met. No browser run happened.** See **The browser gap**, scenario **B1**.
- [ ] The scenario opens a child file or diff beside the child, switches among surfaces, and returns to the preserved child stream.
      — **Not met.** Scenario **B2**.
- [ ] The scenario closes and reopens a running child tab and proves the child continued working while hidden.
      — **Not met.** Scenario **B3**. The server half of this claim _is_ proven —
      `socket_opencode_turn::closing_a_child_surface_does_not_stop_the_server_recording_it`
      unsubscribes from a working child, lets it finish unwatched, and proves the
      reopened stream gained entries and reached its conclusion. What is unproven
      is that the window's close button reaches that path and no other.
- [ ] The scenario scrolls away from live output, observes jump-to-latest behavior, and proves independent position survives tab switching.
      — **Not met.** Scenarios **B4a** and **B4b**. This is also ticket 05's
      criteria 8 and 9, and the reason both are unticked there.
- [ ] The scenario reloads the application and proves tab order, active selection, lazy replay, live continuation, and terminal result restore correctly.
      — **Not met.** Scenario **B5**.
- [ ] The scenario confirms child details do not appear as duplicated parent transcript messages and that no child tab opens automatically.
      — **Not met in a window.** Scenario **B6**. Both halves are proven below the
      window: no duplication at the socket
      (`socket_opencode_turn::an_opencode_child_work_stream_replays_and_then_continues_live`,
      `socket_turn::a_claude_child_work_stream_replays_and_then_continues_live`,
      `socket_codex_turn::codex_children_of_one_collaboration_keep_separate_identities_and_endings`),
      and no auto-open in a DOM against the real store
      (`SubagentAutoOpen.test.tsx`).
- [x] Provider integration evidence confirms Claude, Codex, and OpenCode each satisfy the shared release contract with honest provider-specific omissions.
      — **The release contract across all three providers** below. Every test
      named there was checked to exist and was run green in this ticket's
      verification; the counts are in **Focused verification**.
- [ ] Focused contract, client, provider, and UI checks pass, and the user-visible flow is driven rather than inferred from a green suite.
      — **Half met, and the half that is met is the weaker one, so this stays
      unticked.** Every focused check passes at its recorded baseline
      (**Focused verification**). The driving clause is **not** met, and
      AGENTS.md's own warning applies to this ticket more than to any other in
      the feature: "A green suite is not evidence the application works." Do not
      read the numbers below as the feature being driven.
- [x] Development servers, provider doubles, and browser processes used for verification are stopped after the focused run.
      — Satisfied by construction and then checked rather than assumed: this
      ticket started no dev server, no provider double and no browser. `pgrep`
      afterwards found no browser process of any kind, no vite, and no
      `pnpm dev`/`dev:server`. The one `laplus-server` on the machine is the
      developer's own installed service (`~/.laplus/service/laplus-server`, on
      127.0.0.1:4773), which predates this work by hours and was left alone.
- [x] The throwaway subagent layout prototype and its generated route entry are removed from production code after the real UI is validated; the research/design record remains the decision source.
      — `apps/web/src/routes/subagent-prototype.tsx` (354 lines, headed
      "PROTOTYPE — throw away after choosing a subagent inspection layout") is
      deleted, and `apps/web/src/routeTree.gen.ts` was **regenerated** rather
      than hand-edited: the file's own header says not to edit it, so the
      TanStack generator was run exactly as `tanstackRouter()` runs it in vite's
      `configResolved` hook, rooted at `apps/web`. It produced a pure deletion of
      21 lines and reformatted nothing, which incidentally confirms the committed
      file was in sync with the generator beforehand. `.scratch/subagent-ux/research.md`
      and the spec are untouched and remain the decision source; nothing anywhere
      in the tree still refers to `subagent-prototype`.
      **Read the ordering caveat in the notes**: the spec conditions this removal
      on "after the real UI is validated", and the real UI has not been validated
      in a window.

## The release contract across all three providers

The spec's requirement is that "Claude, Codex, and OpenCode are all release
requirements" and that the product "is not complete until all three satisfy the
shared behavior with honest omissions". Tickets 02, 03 and 04 each proved their
own provider; this is the same evidence in one place, so the contract can be read
without reading six tickets.

Each named test goes through the WebSocket orchestration boundary
(`orchestration.subscribeThread`, `orchestration.subscribeSubagent`,
`orchestration.dispatchCommand`) unless marked _unit_.

### Records child identity

|              | test                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               |
| ------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Claude**   | `socket_turn::a_claude_child_work_stream_replays_and_then_continues_live` — `childId: "ab80091070230889d"` is the CLI's own `task_id`, with `name: "general-purpose"` and `assignment: "Count to three slowly"`; the compact row carries the same id as `data.childId`. `protocol::a_childs_assignment_is_what_it_was_asked_for_rather_than_what_it_is_doing` (_unit_) pins assignment to `task_started`'s description rather than `task_progress`'s activity.                                     |
| **Codex**    | `socket_codex_turn::the_recorded_codex_collaboration_opens_the_childs_own_work_stream` — `childId: "codex-thread-2"` is the child's own Codex thread id, `name: "compute_sum"` from the canonical `agentPath`. `codex_children_of_one_collaboration_keep_separate_identities_and_endings` proves one `wait` naming two receivers yields five independent identities, and `codex::parentage_comes_from_the_path_and_only_where_it_is_proven` (_unit_) proves the three refusals to invent a parent. |
| **OpenCode** | `socket_opencode_turn::an_opencode_child_work_stream_replays_and_then_continues_live` — `childId`, `name`, `assignment`, `state: "working"` on the opening snapshot; the compact row carries the stream reference as `data.childId`.                                                                                                                                                                                                                                                               |

### Records ordered work

|              | test                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| ------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| **Claude**   | `socket_turn::a_claude_child_work_stream_replays_and_then_continues_live` — the ordered `[(1,message),(2,command),(3,command),(4,command),(5,message),(6,outcome)]`, each command with its command line, one `failed` with the CLI's own refusal as detail. `protocol::a_childs_work_reaches_its_stream_as_the_child_reported_it` and `turn::a_childs_tool_is_the_kind_of_work_it_actually_is` (_unit_) pin the translation and the kind each tool lands on. |
| **Codex**    | `socket_codex_turn::codex_children_of_one_collaboration_keep_separate_identities_and_endings` — each child's whole ordered history: spawn, `subAgentActivity`, `wait`, streamed prose coalesced into one entry, a command with its line/status/output, an interaction, then the outcome. `codex_protocol::a_childs_items_fold_into_that_childs_work_and_nothing_else` (_unit_) proves the deltas coalesce and none of it reaches the root.                   |
| **OpenCode** | `socket_opencode_turn::a_child_stream_preserves_the_whole_of_what_the_child_did` — the exact ordered `[message, message, command, read, read, edit, tool, notice, message, outcome]` folded across the replay/live boundary, plus the per-entry facts and two deliberate _silences_ (a `pending` tool part and an unknown child event kind each draw nothing).                                                                                               |

### Records a terminal outcome, and lets it replace stale activity

The rule itself is cross-provider and lives in one place:
`worklog::a_terminal_row_replaces_stale_activity_with_what_came_back` (_unit_),
plus `subagents::a_report_replaces_an_empty_conclusion_in_place` and
`subagents::a_concluded_child_revises_what_it_holds_and_takes_nothing_new`
(_unit_, ticket 06's arbitration A).

|              | test                                                                                                                                                                                                                                                                                                                 |
| ------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Claude**   | `socket_turn::a_claude_child_work_stream_replays_and_then_continues_live` reads every compact row the conversation published: while working the row said `"Running Pause 3 seconds"` and `"1"`; the last row carries the report `"I counted to 3…"` rather than either.                                              |
| **Codex**    | `codex_children_of_one_collaboration_keep_separate_identities_and_endings` asserts all five endings in both the stream outcome and the row message; `codex::every_codex_ending_keeps_what_made_it_different` and `codex::a_terminal_row_replaces_activity_with_what_came_back` (_unit_) pin the mapping and the row. |
| **OpenCode** | `socket_opencode_turn::the_compact_row_follows_the_child_and_then_reports_it` — the row shows the child's command line and file while it works, never `bash` or `task`, then ends `status: "completed"` with `detail: "eleven files"` while the stream keeps the complete terminal entry.                            |

### Replays

|              | test                                                                                                                                                                                                                                                                                                                                                                                              |
| ------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Claude**   | `socket_turn::a_claude_child_work_stream_replays_and_then_continues_live` opens the subscription while the child is genuinely working and asserts **every entry id the wire carried**, not merely that the client's fold is idempotent; a fresh connection replays the identical ids. `a_claude_child_work_stream_replays_after_the_server_restarts` drives two processes over one database file. |
| **Codex**    | `socket_codex_turn::a_codex_child_tab_opens_mid_flight_and_follows_it_without_gaps` opens mid-sentence and asserts the sequences are exactly `1..n` and equal to what a connection that watched none of it replays; `codex_children_of_one_collaboration_keep_separate_identities_and_endings` adds a second server on the same registry file.                                                    |
| **OpenCode** | `socket_opencode_turn::an_opencode_child_work_stream_replays_and_then_continues_live` (replay/live handoff, asserted on wire ids), `a_child_work_stream_replays_after_the_server_restarts` (real process restart over one database file), `store::a_child_work_stream_survives_the_disk_and_stays_out_of_the_transcript` (_unit_).                                                                |

### Records interruption

The shared rule is `subagents::stopping_the_tree_interrupts_exactly_what_was_still_working`,
`subagents::an_interrupted_child_stops_receiving_live_work` and
`subagents::stopping_the_tree_does_not_rewrite_a_previous_runs_children` (_unit_).

|              | test                                                                                                                                                                                                                                                                                                                                                                                       |
| ------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| **Claude**   | `socket_turn::stopping_a_claude_parent_stops_the_child_that_outlived_its_turn` — the case only Claude produces: a conversation whose sole remaining work is a background child, so the stop names no turn at all. The script then runs three more lines including the child's own completion and report, and the test waits five seconds for the stream to move and fails if it ever does. |
| **Codex**    | `socket_codex_turn::stopping_a_codex_parent_stops_the_child_it_was_running`, and `stopping_a_codex_parent_stops_the_generation_below_it_too` — which reaches a **descendant** that has no launcher in the transcript at all. The scripted peer is released afterwards so the provider goes on narrating a child laplus has already ended; the entry identities are asserted unchanged.     |
| **OpenCode** | `socket_opencode_turn::stopping_the_parent_stops_its_delegation_tree`, plus the other Stop door `ending_the_session_ends_the_delegation_tree_with_it`.                                                                                                                                                                                                                                     |

### And the shared lifecycle claims, per provider

- **A child turn never settles the root.** Claude: `a_claude_child_work_stream_replays_and_then_continues_live` counts exactly one `turn.completed` across a subagent's whole life; `a_background_child_keeps_working_after_its_turn_has_settled` covers the other order. Codex: `codex_children_of_one_collaboration_keep_separate_identities_and_endings` publishes exactly one settling `thread.session-set` across two child turn starts, two child turn endings and a child interruption; `protocol_golden::every_codex_fixture_folds_through_a_fresh_state` proves the same at the fold. OpenCode: covered by the routing itself and by `an_opencode_child_work_stream_replays_and_then_continues_live`.
- **The thread stays Working while a descendant is.** Claude: `socket_turn::the_conversation_stays_working_while_a_background_child_does`. OpenCode: `socket_opencode_turn::the_conversation_stays_working_while_its_child_does`. **Codex cannot produce this shape** — its children conclude inside the turn that waits on them — so Codex's half is the _leaving_: `stopping_a_codex_parent_stops_the_generation_below_it_too` asserts the conversation is not working once its tree is.
- **Deleting the parent deletes the child streams**, and ordinary snapshots do not carry them: `socket_opencode_turn::deleting_the_parent_thread_deletes_its_child_work_stream`, `store::a_child_work_stream_survives_the_disk_and_stays_out_of_the_transcript`. Provider-neutral, proven once.

### What each provider honestly does not expose

This is the "honest omissions" half of the criterion, and it is the part a
release reader should not skip. None of these is a defect; each is a place the
protocol says nothing and laplus therefore draws nothing.

**Claude** (`turn::child_stream` carries the same list at the call site)

- **Hierarchy.** A `task_id` names a child of _this conversation_ and says nothing about a child of a child, so `Head::parent_child_id` stays `None` and no nested launcher is drawn. Asserted, not merely intended: `a_claude_child_work_stream_replays_and_then_continues_live` asserts `parentChildId` is null. A subagent that delegates further appears as an `Agent` tool call in its own stream, with no launcher.
- **Child-owned blockers.** A subagent's permission request arrives naming the tool and the session, never the subagent, so it stays the root agent's request and `ApprovalRequest::subagent` stays `None` — spec story 52. Evidence rather than assumption: the `22-background-subagent` child is refused three times and every refusal reaches the server as a `tool_result` on the child's own wire, not as a request anyone could answer.
- **Reasoning.** A forwarded `thinking` block carries `"thinking": ""` and a signature, so there is nothing to record. `protocol::a_childs_work_reaches_its_stream_as_the_child_reported_it` asserts an empty thinking block draws nothing.
- **A separate warning channel.** Claude has no child-level notice event; a child's error is the failed tool result that reports it.
- **A bare ending has no terminal entry.** A child that ends with `task_updated` and **no** `task_notification` reaches a terminal state with no `outcome`, so its tab shows the work and stops. No capture contains the case and the compact row still says "Completed with no result", so nothing is claimed falsely — but see **Residual risks** below, because ticket 06 changed the rule this was reasoned from and the adapter did not follow.

**Codex**

- **No `notice`, `read` or `edit` entries**, so a Codex child tab offers no file or diff navigation at all. `fileChange`, `mcpToolCall`, `webSearch` and `reasoning` are known item kinds whose _bodies_ this build has never decoded — the root path folds them to nothing too — and no capture of one exists. Guessing which field of a `fileChange` holds the path is the fabrication the honesty rule forbids.
- **Five endings, four `OutcomeKind`s.** `errored`/`notFound` both become `failed`, `interrupted`/`shutdown` both become `interrupted`. The difference is not dropped: Codex's own word is on the row verbatim as `data.agentStatus`.
- **No assignment where no `spawnAgent` carried one.** `the_recorded_codex_collaboration_opens_the_childs_own_work_stream` asserts `assignment` is `null` for exactly this reason.
- **Only the last path segment survives** as `name`. The full canonical `/root/reviewer/helper` reaches the client only on the parent row's `data.agentPath`, so a child's _tab_ cannot show it.
- **Child approvals fold as the root's.** `params.threadId` is on every app-server approval request, so a subagent's permission _is_ distinguishable — laplus does not read it yet. Truthful about the request, silent about who is waiting, never attributed on a guess.
- **A concluded child takes nothing new**, so a genuinely resumed Codex child is invisible from its second turn onward. Reopening is arbitration (B), deferred to its own ticket.
- **Half of Codex's evidence is composed, not recorded.** `09-subagent-spawn.jsonl` is a real Codex 0.146.0 turn and proves identity, path/name, separate lifecycles, child prose, terminal outcome and replay. `10-subagent-work.jsonl` is composed from shapes lifted from three real captures, and is what proves rich work, interruption, nesting and the multi-receiver wait. A release reader should not mistake one for the other.

**OpenCode**

- **No proven child-of-child hierarchy.** `parentID` names a child of the conversation and says nothing about a child of a child, so `parentChildId` stays null and no nested launcher is drawn. Codex is the only provider whose protocol proves the relationship.
- **An edit carries the file, not a revision.** A subagent work stream has no turn identity of its own, so the child's "Open diff" brings the existing diff surface into view with the thread's own selection intact (`subagentFileActions::openSubagentDiff` is literally `open(threadRef, "diff")`), where the main agent's `onOpenTurnDiff` would first `selectTurn(ref, turnId, filePath)`. Per-file diff navigation from a child needs a decision about what a child's edit is diffed _against_. **This is the single most likely thing to read as broken in a window** — see scenario **B2**.
- **A file outside the workspace draws no link at all**, rather than one that opens an unresolvable tab (`subagentFileTarget` answers `null`).
- **Which session a permission _reply_ is published on is not established by this repository.** The asking side is settled by OpenCode's own source at `2cba7e2`, but no capture in `fixtures/opencode-http-sse/` contains a `permission.replied` envelope at all, so the adapter handles either session deliberately.

**Shared by all three**

- **`read` and `tool` are coarser than the parent transcript's six `itemType`s**, so a child's web fetch, MCP call or image view draws the generic tool row where the root agent's draws a globe, a wrench or an eye. Same component, same language, less specific icon. Widening the four-member entry vocabulary is a decision about the shared model, deliberately not taken by any of 02/03/04.
- **No historical migration.** The guarantee starts when the child-stream model begins recording, per the spec's own Out of Scope.

## Focused verification

Run in this ticket's worktree with a private `CARGO_TARGET_DIR`. Every number
matches the recorded `d0a3188e` baseline exactly; nothing regressed and nothing
new was added by this ticket, which touches no runtime code.

**`cargo test -p laplus-server --no-fail-fast` over the affected binaries** — 1108 passed, 0 failed, 1 ignored.

| binary                 | this run             | baseline        |
| ---------------------- | -------------------- | --------------- |
| `--lib`                | 894 passed           | 894             |
| `protocol_golden`      | 7                    | 7               |
| `socket_codex_turn`    | 38                   | 38              |
| `socket_continuity`    | 9                    | 9               |
| `socket_deleting`      | 7                    | 7               |
| `socket_interrupt`     | 9                    | 9               |
| `socket_opencode_turn` | 52 passed, 1 ignored | 52 (+1 ignored) |
| `socket_provider`      | 32                   | 32              |
| `socket_session_stop`  | 5                    | 5               |
| `socket_settling`      | 8                    | 8               |
| `socket_streaming`     | 18                   | 18              |
| `socket_turn`          | 29                   | 29              |

**`vp test run` over the feature's focused contract, client-runtime and UI files** — **18 files, 367 tests, all passed**: `packages/contracts/src/orchestration.test.ts`; `packages/client-runtime/src/state/subagentStream.test.ts`; and in `apps/web/src`: `rightPanelStore.test.ts`, `rightPanelCleanup.test.ts`, `session-logic.test.ts`, `subagentFileActions.test.ts`, `components/RightPanelTabs.test.tsx`, `components/Sidebar.logic.test.ts`, `components/chat/{SubagentStreamPanel,SubagentStreamScroller,SubagentLauncher,SubagentNesting,SubagentArtifacts,SubagentAutoOpen}.test.tsx`, `components/chat/subagentScroll.test.ts`, `components/chat/{MessagesTimeline.test.tsx,MessagesTimeline.logic.test.ts}`, `components/chat/ComposerPendingApprovalPanel.test.tsx`.

**`vp run -r typecheck`** — clean across all six projects.

**`vp lint`** — 11 warnings, the baseline count, all pre-existing and none in a subagent path.

**`cargo clippy -p laplus-server --all-targets`** — 76, the baseline count (66 diagnostics plus 10 per-target summary lines).

**`cargo fmt` was deliberately not run.** `server/CLAUDE.md:169` records that this
tree has never been rustfmt-formatted and that the check fails on all 29 files.
The full workspace suite was also not run: AGENTS.md makes it CI's job.

## The browser gap

**Everything below needs a window, and none of it has been done.** This machine
is headless aarch64 with no usable browser — Google Chrome publishes no Linux
ARM64 build, and the distro's chromium is version 85 and cannot parse a modern
Vite bundle — so the developer will drive these themselves.

Work through it with the app open. Each item says what to do and what the code
predicts, so a disagreement between the two is the finding.

### H1 — The held-Working state, and the four things it drives. Do this one first.

This is the sharpest lead in the feature, it is new in ticket 06, and three of
its four consequences have been read in the source but never seen.

**Produce the shape:** a conversation whose root has settled while a background
child is still working. The server then publishes `session.status: "running"`
with `activeTurnId: null` — asserted by
`socket_turn::the_conversation_stays_working_while_a_background_child_does` and
`socket_opencode_turn::the_conversation_stays_working_while_its_child_does`. In
Claude, ask the agent to launch a background subagent for a long task and reply
to you immediately without waiting (fixture `22-background-subagent` is exactly
this shape). In OpenCode, any child still running when the root settles.

`derivePhase` (`session-logic.ts:1469`) returns `"running"` from
`session.status` **alone**, so `ChatView`'s `isWorking` (`ChatView.tsx:1937`) is
true with no turn in flight. Check all four consequences:

- [ ] **H1a — the sidebar pill.** It should read _Working_ (`resolveThreadStatusPill`, `Sidebar.logic.ts:541`) and should leave Working only once the child is terminal, not when the root settles.
- [ ] **H1b — the composer.** The working indicator shows and the stop button is offered. Press it: it must stop the delegation tree, not error on a turn that is not there. (`Shell::stop_the_delegation_tree` has exactly two callers, `thread.turn.interrupt` and `thread.session.stop`.) Confirm the composer still accepts a new prompt when it should.
- [ ] **H1c — the Working duration, and whether it resets.** `WorkingDuration` (`SidebarV2.tsx:220`, fed at `:901`) counts from `resolveWorkingStartedAt` (`Sidebar.logic.ts:523`), which — with the latest turn **completed**, which is exactly this shape — falls through to `thread.session.updatedAt`. `session.updatedAt` moves every time the session republishes. So the code predicts the elapsed number **restarts near zero each time the child says anything**, rather than counting up from when the child began. Watch it for a minute. If it does reset, that is a real bug and it is `resolveWorkingStartedAt`'s, not a subagent one.
- [ ] **H1d — Revert to checkpoint, which may read wrongly.** `ChatView.tsx:4441` refuses with **"Interrupt the current turn before reverting checkpoints."** whenever `phase === "running"`, and phase is running here with no turn to interrupt. Try it and judge the sentence. Refusing may well be right — a child is editing files — but the wording names an action the developer cannot take. If so it is a wording change, or a guard that should distinguish "a turn is in flight" from "a descendant is working", and either way it is a small fix in `ChatView`, not in the subagent model.

### B1 — A compact row opens a live child tab (criterion 1)

- [ ] Run a subagent. In the parent transcript, click the compact `Subagent <name>` row.
- [ ] A normal right-panel tab opens, labelled by the workspace's ordinary conventions with no running/completed/failed decoration of its own.
- [ ] It opens **directly into the work** — no identity header, no assignment header, no composer, nothing typeable.
- [ ] Prose and work entries arrive live and look like the main agent's: capitalised work headings, the same status indicators, markdown rendered rather than escaped.
- [ ] Click the same row again: it **activates** the existing tab and does not open a second one.
- [ ] Do this on all three providers. Claude and Codex have no DOM test of their own — deliberately, because the row is the only provider-specific part and it is asserted at the socket — so this click is the first time either has been exercised through a real renderer.

### B2 — File and diff beside the child (criterion 2)

- [ ] With a child tab open, click a file the child read. It should open a **neighbouring** file tab; the child tab stays open and reachable.
- [ ] Switch back to the child tab: its stream is preserved, not refetched from scratch.
- [ ] Click a child **edit**'s "Open diff". **Expect it to feel incomplete, and judge whether it is acceptable to ship.** It brings the diff surface into view with the thread's own selection intact; it does **not** land on the file the child changed, because a child's edit has no revision to be addressed by. Ticket 02 explicitly asked ticket 07 to judge this against a real window — this is that judgement. Record the verdict, because it decides whether a follow-up ticket is needed before release.
- [ ] Codex children produce no `read`/`edit` entries at all, so a Codex child tab should offer **no** file or diff affordance. Confirm none is drawn.

### B3 — Close and reopen a running child (criterion 3)

- [ ] While a child is actively working, close its tab.
- [ ] Confirm the child keeps working — the compact row in the transcript keeps updating.
- [ ] Reopen from the row. The stream should have **more** entries than when you closed it, and continue live.
- [ ] Confirm closing sent nothing: the child is not interrupted, not detached, and the parent is undisturbed.

### B4a — Scroll restore against real metrics (criterion 4; ticket 05 criterion 8)

Ticket 05's own words: happy-dom lays nothing out, so its restore writes
`scrollTop` against fabricated metrics. In a browser the layout effect runs
**before markdown has reached its final height**, so a restored offset can be
silently clamped.

- [ ] Open a child with a **long** stream — long enough to scroll several screens.
- [ ] Scroll to the middle and note **which entry is under the cursor** (not the pixel number).
- [ ] Switch to another tab and back.
- [ ] Assert the **same entry** is under the cursor. A number that matches while the content has moved is the failure this is looking for.
- [ ] With two child tabs open, confirm each keeps its own place independently.

### B4b — A real jump-to-latest gesture (criterion 4; ticket 05 criterion 9)

Ticket 05 proved the decision and the wiring across nine cases; what is unproven
is that a real wheel or trackpad gesture reaches that viewport element and
produces those three metrics.

- [ ] With a child **running**, stay at the bottom and watch it follow new entries.
- [ ] Scroll up with a wheel, then again with a trackpad, then by dragging the scrollbar. Following should suspend on each.
- [ ] New entries arrive **without pulling you down**.
- [ ] The jump-to-latest pill appears. It is literally the transcript's own `ScrollToEndButton`, so it should look and behave identically to the main agent's.
- [ ] Click it: you return to the live edge and following resumes.
- [ ] Scroll manually back to the bottom instead: following should resume there too.
- [ ] Check the case that motivated removing `contentKey`: an entry that **grows in place** — a command going from running to finished — should keep a pinned reader pinned, even though the entry count never changed.

### B5 — Reload (criterion 5)

- [ ] Open three or four child tabs plus a file, a terminal and a diff. Note the order and which is active.
- [ ] Reload. Tab order, the active selection and the mixture of surface kinds all restore, and child tabs sit among the others rather than in a group of their own.
- [ ] A restored child tab loads its stream **lazily** — nothing fetches until the surface is shown.
- [ ] A child that was still running continues live after the reload; a child that had finished shows its terminal result.
- [ ] Close a child tab, then reload: it should **stay** closed.
- [ ] Switch to a different thread and back: each thread's child tabs are its own.
- [ ] Force the unavailable case if you can (delete or expire a child's stream, then restore its tab): the tab must **stay** and say it is unavailable rather than vanishing.

### B6 — No duplication, no auto-open (criterion 6)

- [ ] Read the parent transcript while several children run. Each child is **one** compact row; none of its prose, commands or edits appear as ordinary parent messages, and the row collapses rather than growing one row per update.
- [ ] Run several children at once with the right panel **closed**. It must stay closed through every state — pending, working, blocked on approval, completed, failed, stopped. Nothing steals focus.
- [ ] With a different tab active, let a child finish. Focus must not move.
- [ ] Confirm **no toast** appears for any child finishing, failing or being stopped.

### N1 — A nested launcher, clicked (ticket 06's lead)

Codex is the only provider that can produce this.

- [ ] Start a Codex conversation that delegates two levels deep, so an agent path has three segments (`/root/reviewer/helper`).
- [ ] Confirm the descendant does **not** appear in the root transcript.
- [ ] Open the spawning child's tab. The descendant's launcher should be **inside** it, drawn by the shared `SimpleWorkEntryRow` — its icon, status indicator and truncated preview have only ever been asserted as text.
- [ ] Click it. A third child tab opens beside its parent, and both stay open.
- [ ] Click it again: it activates rather than duplicating.
- [ ] Where a relationship is **not** proven, no nested launcher should be drawn anywhere.

### S1 — Stop, with the tree live (ticket 06's lead)

- [ ] With several children genuinely working and at least one child tab **open**, press the composer's stop button.
- [ ] Each child reaches _Interrupted_ and stops moving. **The open tab's terminal entry should arrive live, not only on reopen** — that is the specific thing to watch.
- [ ] The compact rows say interrupted, and the sidebar leaves Working.
- [ ] A child that had already reported keeps what it reported rather than being overwritten with an interruption.
- [ ] Do the same through the other Stop door, ending the session.

### R1 — Resizing and the narrow layout (ticket 05 criterion 3)

Neither happens without layout, so neither can be tested where the feature's
tests live. Concretely: the right panel's inline mode has a drag handle on its
left edge, min width 360px, max 70% of the viewport, default 540px, persisted
per browser under `t3code:preview-panel-width`; below `max-width: 980px` the
panel becomes a sheet overlay, and there is a second step at 760px.

- [ ] With a child tab active, drag the splitter to both extremes. The child's content reflows, its scroll position survives, and nothing about it resists the drag differently from a terminal or file tab.
- [ ] Narrow the window past 980px so the panel becomes a sheet, with a child tab active. The tab strip and the child stream both remain usable.
- [ ] Narrow past 760px as well.
- [ ] Widen back: the layout returns and the child tab is still the active one.
- [ ] Check the tab strip overflows the way it does for any other surface once several children are open.

### C1 — Blockers, in the window (OpenCode only)

- [ ] Provoke an OpenCode child permission request. The child's tab should record a blocker naming what it waited for, and the child should read _blocked_.
- [ ] The actionable request appears in the **main conversation's** composer panel and names the waiting child ("Subagent explore: bash needs permission").
- [ ] Answer it with the child's tab **closed** and a different surface active — it must remain answerable.
- [ ] The child's stream records the resolution **on the same entry** rather than adding a second row.

## Notes

**Why the prototype was removed before the UI was validated.** The spec makes
removal conditional on "after the real UI is validated", and it has not been —
so this is a judgement, recorded rather than hidden. The prototype is dead code
either way: it is explicitly labelled not-production in its own first line, has
no importer, and no test covers it. Leaving a stale visual mockup on a routable
path in the shipped bundle is worse than removing it, and the spec's own reason
for the criterion is that the prototype "is evidence for the decision, not
production code to promote". `.scratch/subagent-ux/research.md` remains the
decision source, exactly as the spec intends, and the file is recoverable from
history if the design question is ever reopened. If the developer's own browser
pass finds the real UI wanting, nothing about that decision is made harder by the
mockup being gone.

**This ticket changed no runtime code.** The only production change is the
deletion of the prototype route and its regenerated route-tree entry. Every
number in **Focused verification** is therefore a re-confirmation of tickets
01–06 on one tree, which is what makes it useful as the feature's final green
check — and equally what makes it unable to say anything about the window.

**Criteria 1–6 are deliberately left unticked, and this ticket stays
`ready-for-agent`.** The evidence for what is proven is in the sibling tickets
and consolidated above; what is missing is not more tests but a window.

## Residual risks

- **The feature has never been drawn on a screen.** Every criterion in **The browser gap** is open, and AGENTS.md's warning is the honest summary: a whole afternoon's findings once came from driving the window for a minute, none of which a passing suite had caught. This feature has had none of that minute. The tests are strong — they assert wire ids, whole ordered lists, real clicks against the real store, and several are mutation-checked — but every one of them runs where nothing is laid out.
- **`turn.rs::child_stream`'s stale reasoning — arbitrated.** The comment claiming `Streams::record` "keeps the first outcome it is given" was false and has been **rewritten**. The behavioural gap behind it — a Claude child ending with a bare `task_updated` and no `task_notification` reaches a terminal state with no terminal entry — was **deliberately not closed**, and the reason is now written at the call site. Ticket 03 and this ticket both assumed arbitration (A) would close it; it only half does. (A) is narrow in both directions: **only `Empty` may be replaced**. `protocol::terminal_task_status` folds `failed`, `error`, `cancelled`, `killed` and `timeout` onto `"failed"`, so concluding the bare ending would record `Outcome::failed(None)` for every one of those — not `Empty` — and the `task_notification`'s actual reason a millisecond later would then be refused for ever. That trades a case no capture contains for a case the captures do contain. Closing it properly means deciding what a bare _failure_ concludes as, which is a change to the shared rule rather than to this adapter, and needs a fixture that does not exist. Its own ticket.
- **Half of Codex's rich-work evidence is composed rather than recorded**, as ticket 04 says plainly. Interruption, nesting, multi-receiver waits and the five terminal states are proven against `10-subagent-work.jsonl`, which invents no field but was assembled from shapes lifted from three real captures. A real Codex conversation delegating two levels deep — scenario **N1** — is the first thing that would test that assembly against reality.
- **Arbitration (B), a concluded child reopening on proven provider evidence, is not implemented** and remains deferred to its own ticket. The visible cost is Codex's: a genuinely resumed child is invisible from its second turn onward.
- **`cargo fmt --check` remains unrun** for this branch, as for every branch in this feature and for CI. The tree has never been rustfmt-formatted.

## What the feature-wide review found (base `8ca1365d` → `0f6e46d9`)

The final review ran `/code-review`'s two axes against the whole feature rather
than against any one ticket's base. Its subject was the seam per-ticket review
cannot see: individually-correct changes composing into something wrong.

### A release blocker

**A stopped child's compact row contradicts its own stream, and then reports the
answer the developer declined to wait for.** `Shell::stop_the_delegation_tree`
(`orchestration.rs`) reaches only `Streams::interrupt` and `follow_delegation`.
Every compact-row emitter lives in a provider fold path (`turn.rs::fold`,
`opencode.rs`, `codex.rs`), so a Stop draws no row and nothing refuses one
afterwards. Two consequences, both driven and both real:

1. Immediately after the Stop the row still reads `running` with its pre-stop
   detail, while the child's tab reads _Interrupted_.
2. The provider goes on narrating a child the developer ended. `Streams::record`
   refuses all of it — that is ticket 06 criterion 7, and it holds — but the row
   does not, so the row settles on `tool.completed` / `status: "completed"` /
   `detail: "eleven variants"`.

`socket_turn::a_stopped_claude_child_row_agrees_with_the_stream_it_belongs_to` is
the executable form, marked `#[ignore]` so the suite stays at its baseline:

```
cargo test -p laplus-server --test socket_turn a_stopped_claude_child_row_agrees -- --ignored
```

This is the predicted shape exactly. Ticket 02 made the compact row the
terminal-preview surface (spec: _"When a child becomes terminal, replace latest
activity atomically with a bounded result, failure, interruption, or
empty-result preview"_, story 39). Ticket 06 added a new terminal path that
bypasses it, and asserted only the stream. It is not caught by S1 in **The
browser gap** either: S1's _"The compact rows say interrupted"_ would find it,
but a statically provable contradiction should not be waiting on a window.

**Not fixed here, deliberately.** The fix needs a terminal row drawn on the
developer's own command _and_ a rule that a concluded child's row is not
redrawn by later provider narration. The row's collapse key is provider-specific
(`subagent:{taskId}` for Claude and OpenCode, `agent:{threadId}` for Codex), so
the emitter cannot be provider-neutral without either a provider switch in the
orchestration layer or new state on the shared child-stream model; and the
suppression rule sits at `session::spend`, the choke point that applies _every_
transcript activity for _every_ provider. That is not a change to make in the
last minutes before a publish. Its own ticket, and it should block the release
until it lands.

### Fixed in the review

- **Codex's terminal child row left stale activity standing** (`codex.rs::collaboration_agent_row`).
  A silent completion emitted no `detail` at all, on the reasoning that "there is
  no stale line it would be honest to show instead". On this wire that is not
  silence: `session-logic.ts::mergeDerivedWorkLogEntries` collapses a child's rows
  onto one entry with `detail = next.detail ?? previous.detail`, so an absent
  `detail` leaves the _running_ row's line standing as the child's ending —
  exactly the stale activity ticket 02's cross-provider rule exists to prevent,
  and one of the two providers it was never checked against. The empty conclusion
  is now said, in the words the other two drivers already use: the sentence moved
  to `subagents::OutcomeKind::without_a_report` and `worklog::concluded_without_a_report`
  delegates to it, so the vocabulary cannot drift a second time.
  `codex::a_terminal_row_replaces_activity_with_what_came_back` carries the
  assertion that previously pinned the wrong behaviour.
- **`turn.rs::child_stream`'s stale justification** — see **Residual risks**.
- **The Claude driver's third copy of the output bound.** `turn.rs` still declared
  its own `CHILD_OUTPUT` and `bounded`; ticket 03 recorded that they belonged in
  `subagents.rs` and that unifying them while ticket 04 was in flight would
  collide, ticket 04 built the home, and nobody went back for Claude's copy. The
  two constants were still in step by luck. `turn.rs::bounded` now delegates and
  keeps only the part that is this wire's (whitespace-only output is absence).
- **Documentation made false by a later wave**, all verified against the code
  they describe: `subagents::EntryKind`'s "Eight members" is nine and omitted the
  nested launcher; three intra-doc links and the `server/CONTEXT.md` glossary
  still named `crate::subagents::Delegated`, renamed to `Launcher` in `ca3d0456`;
  `Streams::active`'s doc claimed Working state is derived from it when
  `threads.rs` derives it from `Streams::working`; `session-logic.ts`'s
  `subagentChildId` still said an absent stream means "a driver that has not
  learned to record child work yet", which commit `86ec2ba7` corrected in its two
  siblings and missed here; `worklog.rs`'s doc for `waiting_child` was attached
  to the `SUBAGENT` constant above it; and a `worklog` test doc still described
  `description` in the vocabulary its production comment had abandoned.

### Recorded, not acted on

- **Two `child_entry_kind` classifiers** (`turn.rs`, `opencode.rs`) remain
  duplicated and have diverged (`terminal`, `notebook` ordering, `TodoWrite`).
  Ticket 03 already recorded this as deliberate, with repo precedent —
  `worklog::opencode_item_type` sits beside `worklog::Kind::of` saying its
  first-party rules intentionally differ. The classifier is per-provider on
  purpose. Standing.
- **A nested descendant's tab is labelled generic "Subagent".**
  `ChatView.tsx::subagentLabelsById` is built from the root transcript's work-log
  entries only, and ticket 06 deliberately keeps a descendant _out_ of the root
  transcript, so a tab opened from a launcher inside another child's stream can
  never find its name — even though `OrchestrationSubagentLauncher.name` carries
  it. Codex-only, since Codex is the only provider that proves nesting. A label
  degradation rather than a wrong claim; worth a follow-up ticket, and worth
  looking at during **N1**.
- **Codex child approvals are unattributed**, and this ticket's **What each
  provider honestly does not expose** files it under the wrong heading. Codex
  _does_ expose `threadId` on `item/commandExecution/requestApproval`
  (`fixtures/codex-app-server/03-write-approval.jsonl`), so this is an
  unimplemented requirement (story 50) rather than an honest omission. It was not
  implemented, and should not be: no capture contains a _child_ raising an
  approval — the fixture's `threadId` is the root's — so the attribution could be
  written but not proven.
- **Codex children still produce no `read` or `edit` entries**, so file and diff
  navigation from a Codex child tab is unreachable where OpenCode and Claude have
  it. Disclosed by ticket 04; it is a partial requirement (stories 10–12), not a
  nuance.
- **Cosmetic Standards findings, not taken**, because a pure-quality change with
  no test behind it is a bad trade before a publish: three near-identical
  right-panel wrappers (`rightPanelStore::openSubagentSurface`,
  `subagentFileActions::openSubagentFile`/`openSubagentDiff`); the module-global
  scroll-position map in `subagentScroll.ts` with its test-only reset; and
  `client-runtime/state/subagents.ts`'s `export *` re-export, which follows the
  established shape of `state/terminal.ts`, `state/projects.ts`, `state/shell.ts`
  and `state/threads.ts` and is reached through an explicit subpath, so AGENTS.md's
  "no root export" rule is not breached.

### Two of this ticket's own leads, checked statically

- **H1c is a false lead.** `Threads::follow_delegation` returns early unless the
  session status actually transitions, so `session.updatedAt` does not move per
  child event and the Working duration will not reset. Do not spend browser time
  on it.
- **The four cross-ticket claims each worker asserted and nobody composed** were
  checked and hold, with one exception. `worklog::subagent`'s terminal rule is
  correct for Claude (`protocol::subagent_moved`'s `reported`/`finished` gate lets
  the report through and refuses the second bare ending, so the row cannot regress
  from a report back to "Completed with no result") and was wrong for Codex, which
  is the fix above. `settle_watch` weakened nothing: `settle_watch()` starts its
  `on_a_turn` cell `true`, so for every pre-existing caller the predicate matches
  the same first settle the base predicate did, and `values_until` loops until a
  match rather than returning on exhaustion — a stricter predicate would hang, not
  pass vacuously. The contract's discriminated `payload` union decodes everything
  the server can emit by construction: `EntryKind` is a closed Rust enum whose
  nine `as_str` values are exactly the union's nine literals, and every payload
  builder matches its member's schema, so forward-compatibility is a question
  about unknown _provider_ variants — which the adapters still route to
  `EntryKind::Tool` and to the drift counters — rather than about unknown entry
  kinds.
