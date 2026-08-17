# 04 — Show Codex subagent work streams and hierarchy

**What to build:** Give Codex users the same inspectable child-tab experience while preserving Codex's richer stable identities, canonical agent paths, nested relationships, collaboration activity, work, errors, and outcomes without confusing child lifecycle events with the parent turn.

**Blocked by:** 01 — Open an OpenCode child work stream

**Status:** ready-for-human

- [x] A newly captured Codex subagent has stable child identity, semantic path/name when provided, assignment, state, stream reference, and ordered persisted entries.
      — `socket_codex_turn::the_recorded_codex_collaboration_opens_the_childs_own_work_stream` reads all of it off the **recorded** capture: `childId: "codex-thread-2"` (the child's own Codex thread id), `name: "compute_sum"` from the canonical `agentPath`, `state`, `outcome`, and the ordered entries `[tool, message, outcome]`. Its `assignment` is `null` and asserted to be — that capture contains no `spawnAgent` call and therefore no prompt, and an assignment the protocol did not supply is not invented. `codex_children_of_one_collaboration_keep_separate_identities_and_endings` proves the assignment where a spawn does carry one (`"Review the decoder."`), and that the entries persist: it restarts the server on the same registry file and asserts the reloaded stream and head are equal to the ones the live run produced.
- [x] Collaboration operation rows and long-lived child rows remain separate, so completing a spawn or interaction operation cannot falsely complete the child.
      — `the_recorded_codex_collaboration_opens_the_childs_own_work_stream` asserts the `wait` row carries `data.operation`, its own `toolCallId`, and **no** `childId`, beside a child row that carries one. `a_codex_child_tab_opens_mid_flight_and_follows_it_without_gaps` opens a tab on a child whose `spawnAgent` has already completed and asserts the stream's first entry is a _completed_ spawn while the stream itself is still `pending`/`working` with `outcome: null`. Unit: `codex::tests::a_completed_spawn_leaves_its_child_open` (the operation sets neither state nor outcome).
- [x] Clicking a Codex child row opens or activates a normal read-only right-panel tab using the shared main-agent message/work presentation.
      — The Codex half is `the_recorded_codex_collaboration_opens_the_childs_own_work_stream`: the child row carries `data.childId`, and `orchestration.subscribeSubagent` on that exact id returns the stream. Everything downstream of the row is ticket 01's provider-neutral path and is unchanged by this ticket — `SubagentLauncher.test.tsx` (a real click on the real row through `openSubagentSurface`, open-versus-activate), `rightPanelStore.test.ts` (resource-addressed surfaces), `SubagentStreamPanel.test.tsx` (read-only, no composer, shared message/work rendering), `MessagesTimeline.logic.test.ts::resolveWorkEntryActivation` (launch beats expand). No duplicate DOM test was written for Codex: the row is the only provider-specific part, and it is asserted at the socket.
- [x] Codex child prose, collaboration activity, commands/tools, errors, interactions, and terminal outcomes are preserved in chronological order when supplied by the protocol.
      — `codex_children_of_one_collaboration_keep_separate_identities_and_endings` asserts each child's whole history as an ordered list: the spawn, the `subAgentActivity` that started it, the `wait`, its streamed prose (deltas coalesced into one entry under the item's key), its command with command line, status and output, an `interacted` interaction, and the terminal outcome last. `codex_protocol::tests::a_childs_items_fold_into_that_childs_work_and_nothing_else` proves the deltas coalesce and that none of it reaches the root's messages or commands.
      **What Codex children do not produce, and why.** No `notice`, `read` or `edit` entries, so a Codex child tab offers no file or diff navigation. `fileChange`, `mcpToolCall`, `webSearch` and `reasoning` are known item kinds whose _bodies_ this build has never decoded — the root path folds them to nothing too — and no capture of one exists. Guessing which field of a `fileChange` holds the path is the fabrication the honesty rule forbids, so the child's stream stays silent about them; `child_notification`'s final arm is where a recorded one would land. A child's error surface is its turn error, which becomes the terminal outcome rather than a `notice`.
- [x] Canonical paths and parent identities preserve proven hierarchy without inventing missing relationships.
      — `codex_children_of_one_collaboration_keep_separate_identities_and_endings`: `/root/reviewer/helper` resolves `parentChildId` to the reviewer's own thread id, and the assertion is repeated after a restart. `codex::tests::parentage_comes_from_the_path_and_only_where_it_is_proven` covers the three refusals: a path directly under the root has no parent _child_, a path whose ancestor no known agent holds resolves to nothing, and an ancestor **two** agents both claim resolves to nothing rather than to a tie-break. The path itself stays verbatim on the row as `data.agentPath`.
      Nested _placement_ — moving that launcher out of the root transcript and into the spawning child's stream — is ticket 06's and is deliberately not done here.
- [x] Multiple receivers from one collaboration operation retain independent identities, streams, and terminal states.
      — `codex_children_of_one_collaboration_keep_separate_identities_and_endings`: one `wait` naming two receivers and reporting five agents produces five launchers, five streams and five independent endings. The one operation leaves one entry in each receiver's own history, under one key, which the `wait`'s completion later revises in place rather than duplicating.
- [x] Child turn/thread boundary events never settle, clear, or otherwise impersonate the root turn lifecycle.
      — Same test: across a turn containing two child `turn/started`s, two child turn endings and a child interruption, exactly **one** settling `thread.session-set` is published, and it is the root's own completion. `protocol_golden::every_codex_fixture_folds_through_a_fresh_state` proves the same at the fold: the composed capture's five child messages, its command and its child turns leave the root's `assistantMessages`, `commandExecutions`, `turnStatus` and drift counters untouched. The pre-existing `a_child_status_arriving_before_its_agent_is_named_leaves_the_parent_alone` and `a_child_turn_completion_updates_the_agent_without_completing_the_parent` still hold.
- [x] Interrupted, shutdown, errored, missing, and completed children retain distinct truthful terminal outcomes and messages.
      — `codex_children_of_one_collaboration_keep_separate_identities_and_endings` asserts all five, both in the stream's outcome and in the compact row's message. `codex::tests::every_codex_ending_keeps_what_made_it_different` pins the mapping, including that a silent completion is `empty` rather than a sentence laplus wrote. `a_terminal_row_replaces_activity_with_what_came_back` pins the row's half: a reported child's row shows what came back, never the command it was running when it stopped.
      Codex distinguishes five endings where `OutcomeKind` has four: `errored`/`notFound` both become `failed` and `interrupted`/`shutdown` both become `interrupted`. The difference is not dropped — where Codex gave no message of its own the outcome names its state, and Codex's own word is on the row verbatim as `data.agentStatus`. See the notes.
- [x] Replay, reload, and live continuation preserve ordering without gaps or duplicates.
      — `a_codex_child_tab_opens_mid_flight_and_follows_it_without_gaps` opens a tab while the child is mid-sentence, folds the opening snapshot together with every live frame the way a client does, and asserts the sequences are exactly `1..n` and that the result equals what a connection that watched none of it replays. `codex_children_of_one_collaboration_keep_separate_identities_and_endings` adds the reload: a second server on the same registry file replays the same entries and head.
- [x] Recorded Codex collaboration fixtures prove identity, hierarchy, separate operation/child lifecycles, rich work, interruption, and replay through the external orchestration boundary.
      — **What the genuine recording proves.** `09-subagent-spawn.jsonl` is a real Codex 0.146.0 turn, and `the_recorded_codex_collaboration_opens_the_childs_own_work_stream` drives it through the socket for: stable identity, the canonical path and the name taken from it, separate operation and child lifecycles, the child's own prose, its terminal outcome, and replay through `orchestration.subscribeSubagent`.
      — **What the composed fixture proves.** `10-subagent-work.jsonl` is **composed, not recorded**, out of message shapes lifted from `09-subagent-spawn`, `02-command-execution` and `01-plain-turn`; it invents no field, and its README paragraph says exactly which shapes and why. It carries what one small real turn could not: rich child work (streamed prose and a command), interruption, nesting three segments deep, a multi-receiver `wait`, and the five terminal `agentsStates` that 09's always-empty map could not. `codex_children_of_one_collaboration_keep_separate_identities_and_endings` and `a_codex_child_tab_opens_mid_flight_and_follows_it_without_gaps` drive it through the same socket.
      A future reader should not mistake one for the other: **identity, path/name, separate lifecycles, child prose, terminal outcome and replay are recorded evidence; rich work, interruption, nesting and the multi-receiver wait are composed.**

## Notes

**A subagent is a thread.** Codex delegates by starting another thread and
narrating it on the same wire, so a child's prose, commands and turn boundaries
arrive as ordinary `item/*` and `turn/*` notifications distinguished only by
`threadId`. Everything here follows from routing on that one field: the child's
events become `subagents::Update`s and never touch the parent conversation, the
child's turn cannot settle the root's, and the child's thread id is its child id.

**Four proposals, deliberately not implemented.** Each is a place where Codex
exposes more than the shared vocabulary can hold. All four were ruled out of this
ticket; they are recorded here with what each would need.

1. **`Head.path` — the full canonical agent path.** Only its last segment
   survives, as `name`, and the whole path reaches the client only on the parent
   row's `data.agentPath`. A child's _tab_ therefore cannot show
   `/root/reviewer/helper`. Would need an additive `path: Option<String>` on
   `subagents::Head` and `OrchestrationSubagentStream`, plus a `subagent_streams`
   column. Ticket 06 may want it for the delegation tree.
2. **Parent-to-child input has no honest entry kind.** `message` is the child's
   own prose by definition, so a `sendInput` prompt recorded there would
   misattribute it. It is a `tool` entry titled "Sent input to subagent" instead.
   An honest home needs either an author on the message payload or a new kind.
3. **Five Codex endings, four `OutcomeKind`s.** `errored`/`notFound` and
   `interrupted`/`shutdown` each collapse to one kind. A fifth kind, or an
   `outcome.providerStatus` carrying the provider's own word, would let a client
   distinguish "it broke" from "its thread was gone" without reading prose.
4. **Child reasoning is dropped.** There is no thinking entry kind, and recording
   a child's reasoning as prose would present its thinking as its speech.

**Codex can prove which agent a permission belongs to, and laplus does not read
it yet.** `params.threadId` is on every app-server approval request — see
`fixtures/codex-app-server/03-write-approval.jsonl` — so a subagent's permission
is distinguishable from the root's, and `approval::ApprovalRequest::subagent`
(ticket 02) is the field that would carry it. Wiring it needs `fold_request` to
route on that id, a `blocker` entry in the child's stream, and the resolution
written back under the same key — ticket 02's machinery, applied to Codex. Until
then a child's approval folds as the root's: truthful about the request, silent
about who is waiting, and never attributed on a guess. `fold_request`'s doc says
so at the point where it would change.

**A reported child is closed to anything new, and that is honest but lossy.**
A child concludes on its own `turn/completed`, which is the only completion
signal the recording offers. A later `sendInput` or `resumeAgent` to that child
therefore records nothing, because a terminal state is final in
`subagents::Streams` and this driver refuses new entry keys after a conclusion.
The one exception is a key the stream already holds: the `wait` that was waiting
on a subagent completes _after_ it does, so its entry is revised where it stands
rather than appended after the outcome. Two consequences worth knowing:
`Children` is in-memory, so after a restart that refusal is gone and a late event
could append behind a terminal entry; and a genuinely resumed Codex child is
invisible from its second turn onward. Reopening a concluded child is a change to
the shared model rather than to this adapter, and is not made here.

**What the compact row says.** While a child runs the row carries the latest
meaningful thing _it_ did — its own prose or its own command line, not the
parent's spawn, wait or input, and not its path. Once it has reported, the row
carries what came back and nothing else, which is the spec's "terminal state
replaces stale activity" for this driver. The row is redrawn only on the events
Codex already publishes one for; a child's prose does not publish an extra row,
because rows for interleaved children do not collapse into one and the parent
transcript would grow a row per sentence.

**Two duplications left in place on purpose.** `codex::bounded` is
`opencode::bounded`, and `socket_codex_turn::folded_entries` is
`socket_opencode_turn::folded_entries`. The only right home for the first is
`crate::subagents` and for the second `tests/harness/`, and consolidating either
means editing a file another branch of this feature owns this wave. Both sites
name their twin so the duplication is visible rather than accidental; fold them
together at the merge.

`ChildNotification` and `ChildEvent` are Codex-protocol-local decode types and
are named so they do not shadow the glossary: `CONTEXT.md` reserves _child work_
for `subagents::Work` and _stream entry_ for `subagents::Entry`, and these are
neither — they are what the wire said, before `Children` decides which of them
become entries. This ticket introduced no new product vocabulary, so `CONTEXT.md`
is unchanged.

What this ticket does **not** prove is the application: ticket 07 owns the
browser-driver acceptance run across all three providers.

`cargo fmt --check` is not run for this branch. `server/CLAUDE.md` records that
this tree has never been rustfmt-formatted and that the check fails on every
file.
