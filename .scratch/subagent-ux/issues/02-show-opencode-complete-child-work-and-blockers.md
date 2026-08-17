# 02 — Show OpenCode's complete child work and blockers

**What to build:** Expand the OpenCode tracer so its child tab shows the complete work OpenCode exposes—commands, output, reads, searches, edits and diffs, tools, errors, blockers, and result—while actionable descendant requests remain impossible to miss in the main conversation.

**Blocked by:** 01 — Open an OpenCode child work stream

**Status:** ready-for-human

- [x] An OpenCode child stream preserves chronological prose, command invocation/output/status, file reads and searches, edits with diff navigation, other tool calls/results, warnings/errors, and terminal outcomes when those events are present.
      — `socket_opencode_turn::a_child_stream_preserves_the_whole_of_what_the_child_did` asserts the exact ordered list `[message, message, command, read, read, edit, tool, notice, message, outcome]` folded across the replay/live boundary, plus the per-entry facts: the command line and its output, the read's file, the search's pattern, the edit's changed file, the failed call's error, the warning's level. It also asserts two _silences_ — the `pending` `tool` part OpenCode opens every call with draws nothing, and an unknown child event kind draws nothing.
      **Limitation on "with diff navigation":** an edit entry carries the _file_ it changed, and that is what the child tab offers navigation from — see criterion 3. What it does **not** carry is a revision the diff panel is addressed by, because a subagent work stream has no turn identity of its own. So the child's "Open diff" brings the existing diff surface into view with the thread's own selection intact, where the main agent's `onOpenTurnDiff` would first `selectTurn(ref, turnId, filePath)` and land on the file within the turn that changed it. Per-file diff navigation from a child needs a decision about what a child's edit is diffed _against_, which is a change to the shared model rather than to this adapter. `subagentFileActions.ts::openSubagentDiff` records this at the call site. Ticket 07 should judge it against a real window.
- [x] The child tab renders those entries through the same semantic UI used for equivalent main-agent work rather than a raw event log.
      — `SubagentStreamPanel.test.tsx::draws the child's work in the main agent's work-entry language`. Evidence rather than assertion: the headings come out capitalised by the shared row's own `capitalizePhrase`, and the failure indicator is `SimpleWorkEntryRow`'s `aria-label="Tool call failed"` — neither is something this panel could produce. `keeps the child's work in the order it happened` proves chronology survives rendering; `draws a child's warning in chronological context` and `says why a child waited and how it resolved, on one row` cover the two non-tool kinds.
      **Known fidelity gap:** the stream's `read`/`tool` kinds are coarser than the parent transcript's six `itemType`s, so a child's web fetch, MCP call or image view draws the generic tool row where the root agent's draws a globe, a wrench or an eye. Same component and same language, less specific icon. `child_entry_kind` in `opencode.rs` carries why: widening the vocabulary is a decision about the shared entry model that Claude and Codex also fill.
- [x] File and diff actions from the child open neighboring existing right-panel surfaces without closing or replacing the child tab.
      — `SubagentArtifacts.test.tsx` drives real DOM clicks against the real store with the child's tab already open: the surfaces end `["subagent:call_task_1", "file:src/main.rs"]` and `["subagent:call_task_1", "diff"]`. `subagentFileActions.test.ts` proves the store half, including three artifacts in a row and returning to the child by activation rather than reopen. Mutation-checked: pointing `openSubagentFile` at the wrong path and `openSubagentDiff` at the files surface turns five tests red.
      A provider reports the path _it_ used, which for OpenCode is absolute, while the file surface is addressed by a workspace-relative path — `subagentFileTarget` converts, and answers `null` for a file outside the workspace so no affordance is drawn rather than one that opens an unresolvable tab. `opens an absolute path the child reported as the workspace file it names` and `offers no link for a file outside the workspace` prove both.
- [x] The compact parent row shows the latest meaningful child activity while running, ignoring transport noise and unhelpful partial states.
      — `socket_opencode_turn::the_compact_row_follows_the_child_and_then_reports_it`: the row shows `ls -1 src | wc -l` and `src/main.rs` as the child works, and never `bash` (the announced-but-empty part) or `task` (the tool the subagent itself is).
- [x] Terminal state atomically replaces stale activity with a bounded result, failure, interruption, or empty-result preview while the full outcome remains in the child stream.
      — the same test's tail asserts the last row is `status: "completed"` with `detail: "eleven files"`, and the stream still carries the complete terminal entry. `worklog::a_terminal_row_replaces_stale_activity_with_what_came_back` covers the case that was actually broken: a child finishing with _no_ report now reads "Completed with no result" instead of reverting to what it was doing a moment earlier.
- [x] A child-owned permission or question is persisted in the child stream and also appears as an actionable request in the main conversation identifying the waiting child.
      — `socket_opencode_turn::a_descendant_permission_is_recorded_in_the_child_and_answered_from_the_conversation` and `…question…`: the child's state becomes `blocked` with a `blocker` entry naming the provider's request id, and the conversation receives `approval.requested` / `user-input.requested` carrying `subagent.childId` and `subagent.name`, with a summary reading "Subagent explore: bash needs permission". Client side: `session-logic.test.ts::carries the waiting child through to the approval panel` (and `leaves a root agent's request unattributed`), `ComposerPendingApprovalPanel.test.tsx::names the subagent waiting on a decision`.
- [x] Answering or rejecting a descendant request routes through the originating child's provider request identity and records resolution in that child's stream.
      — both tests assert the peer received the reply on `child-per-1` / `child-que-1` and that the child's stream comes back with the resolution written onto the _same_ entry (`blockers.len() == 1`). `a_legacy_descendant_permission_is_answered_on_the_childs_session` covers the one route where identity is load-bearing beyond the request id: the session-scoped legacy route must carry `ses_child_1`, not the conversation's session.
      When the decision **cannot** be delivered, the conversation and the child agree rather than diverging: `a_decision_that_could_not_be_delivered_says_so_in_both_places` proves the conversation carries both the resolution and a `session.failed`, while the child's blocker records `undelivered` and the child stays `blocked` — because it is. Mutation-checked.
- [x] A blocker remains actionable when the child tab is closed or another right-panel tab is active.
      — `a_descendant_permission_is_recorded_in_the_child_and_answered_from_the_conversation` unsubscribes from the child stream _before_ dispatching the decision, so the answer is given with no child surface open anywhere. By construction the request lives in the main conversation's existing `Driving::outstanding` fold and its composer panel, which no right-panel state can reach.
- [x] Providers' unknown future child event variants remain forward-compatible under the existing drift policy instead of breaking the parent turn or child stream.
      — the scripted child session emits `child.future.event`; `a_child_stream_preserves_the_whole_of_what_the_child_did` asserts it produces no entry, the stream reaches `completed`, and the parent turn settles normally. Unknown _stored_ entry kinds are still dropped by `subagents::entry_from_stored` rather than refusing the conversation.
- [x] Scripted OpenCode tests cover rich child activity and a descendant permission/question through the external orchestration boundary.
      — eight new tests in `socket_opencode_turn.rs`, all through the WebSocket boundary against the scripted peer: `a_child_stream_preserves_the_whole_of_what_the_child_did`, `the_compact_row_follows_the_child_and_then_reports_it`, `an_entry_that_changes_in_place_reaches_a_watching_client`, `a_blockers_resolution_reaches_a_watching_client`, `a_decision_that_could_not_be_delivered_says_so_in_both_places`, `a_legacy_descendant_permission_is_answered_on_the_childs_session`, `a_descendant_permission_is_recorded_in_the_child_and_answered_from_the_conversation`, `a_descendant_question_is_recorded_in_the_child_and_answered_from_the_conversation`.

## Notes

**The entry vocabulary tickets 03 and 04 build on.** `OrchestrationSubagentEntry`
is a discriminated union on `kind` — the `payload: Schema.Unknown` ticket 01 left
open is gone. `message` carries `{text}`; `command`/`read`/`edit`/`tool` carry
`OrchestrationSubagentWork` (`title`, `status` in the client's own
`toolLifecycleStatus` vocabulary, and nullable `detail`/`command`/`paths`/`query`);
`notice` carries `{level, text}`; `blocker` carries `{requestId, blocker, title,
detail, resolution}`; `outcome` is unchanged. A `null` is absence, not emptiness.

**A blocker is one entry that moves.** Asking and answering share a key
(`blocker:<requestId>`), so a child's history reads "it waited for this, and this
is what it was told" rather than leaving two rows to be paired by an id. The same
property makes a tool call one entry that moves through its statuses.
`an_entry_that_changes_in_place_reaches_a_watching_client` proves a live reader
learns both changes from `entry-upserted` frames **alone** — it discards every
`stream-updated` before folding — because the head's `updatedAt` is
millisecond-resolution and two writes inside one millisecond stamp identically.

**One cross-provider change was made deliberately.** `worklog::subagent` no
longer falls back to the child's last activity once the row is terminal, which
is the spec's "terminal state replaces stale activity" and affects the Claude
and Codex drivers too. Proven here for OpenCode; tickets 03 and 04 own proving it
for theirs.

**Which session OpenCode publishes a permission _reply_ on is not established by
this repository.** The asking side is settled by OpenCode's own source at
`2cba7e2` (cited in `research.md` and at the call site), but no capture in
`fixtures/opencode-http-sse/` contains a `permission.replied` envelope at all.
The adapter therefore handles the reply arriving on either session deliberately,
and says so where it does. Capture one and that hedge can go.

What this ticket does **not** prove is the application: ticket 07 owns the
browser-driver acceptance run.

`cargo fmt --check` is not run for this branch. `server/CLAUDE.md` records that
this tree has never been rustfmt-formatted and that the check fails on every
file.
