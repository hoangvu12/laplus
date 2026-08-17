# 03 — Show Claude subagent work streams

**What to build:** Give Claude users the same compact-row-to-read-only-tab experience, preserving every child message and work event Claude truthfully exposes, including background subagents that continue after the root becomes quiet and their eventual outcomes.

**Blocked by:** 01 — Open an OpenCode child work stream

**Status:** ready-for-human

- [x] A newly captured Claude subagent has stable child identity, assignment, state, stream reference, and an ordered persisted work stream independent from the parent transcript.
      — `socket_turn::a_claude_child_work_stream_replays_and_then_continues_live`: the opening snapshot carries `childId: "ab80091070230889d"` — the CLI's own `task_id`, which is also the `agentId` it addresses a running agent by, so nothing is minted here — with `name: "general-purpose"`, `assignment: "Count to three slowly"` and `state: "working"`, and the compact row carries the same id as `data.childId`. The stream is the ordered `[(1, message), (2, command), (3, command), (4, command), (5, message), (6, outcome)]`, and the same test's thread snapshot proves the parent transcript holds none of it. `protocol::a_childs_assignment_is_what_it_was_asked_for_rather_than_what_it_is_doing` pins the one field the CLI says two different things about: the head's assignment is `task_started`'s description and never `task_progress`'s "Running Pause 3 seconds", which stays what the row shows as latest activity.
- [x] The existing Claude compact row remains the summary and launcher, showing meaningful live activity followed by a truthful terminal preview.
      — `a_claude_child_work_stream_replays_and_then_continues_live` reads every compact row the conversation published for this child and asserts both halves against the recording: while the child worked the row said `"Running Pause 3 seconds"` (the CLI's own account) and `"1"` (the child's own words), and the last row it published carries the report — `"I counted to 3…"` — rather than either of them. That is ticket 02's cross-provider rule ("terminal state replaces stale activity") proven for Claude, which ticket 02's notes left to this one; the rule itself is `worklog::a_terminal_row_replaces_stale_activity_with_what_came_back`. The same test asserts what makes the row a launcher: `itemType: "collab_agent_tool_call"`, `title: "Subagent general-purpose"`, `data.taskId` and `data.childId`. `socket_turn::a_background_subagent_gets_its_own_row_and_stays_out_of_the_transcript` (unchanged by this ticket) covers the row's running-then-completed statuses and its `data.summary`.
- [x] Clicking the row opens or activates a normal read-only child tab using the shared main-agent message/work presentation.
      — The only Claude-specific part of this is the row carrying the launcher reference, and that is asserted at the socket: `a_claude_child_work_stream_replays_and_then_continues_live` (`payload.data.childId == "ab80091070230889d"` on a `collab_agent_tool_call` row) and `a_claude_child_work_stream_replays_after_the_server_restarts` (the restored conversation still carries it). Everything downstream is provider-neutral code proven by ticket 01: `session-logic.ts` reads `payload.data.childId` into `subagentChildId` for any provider, `SubagentLauncher.test.tsx` drives a real click through the same `openSubagentSurface(threadRef, childId)` composition `ChatView` performs (`opens that child's work stream as a right-panel tab`, `activates the tab it already has rather than duplicating it`), and `SubagentStreamPanel.test.tsx` proves the surface is read-only and renders through the main agent's own message and work-entry components. **No duplicate DOM test was written for Claude on purpose** — there is no Claude-specific code on that path, and a second copy would assert the client re-reads a differently-spelled string. Note also that the spawning `Agent` tool call keeps its own row with no `childId`, so the thing that is not a stream offers no launcher.
- [x] Claude child prose, exposed tools/work, errors, and terminal outcome appear in chronological order without being duplicated as parent messages.
      — `a_claude_child_work_stream_replays_and_then_continues_live` folds the whole stream the way a client does and asserts the ordered list above: the child's "1", the three `Bash` calls each with its command line, `status: "failed"` and the CLI's own refusal as `detail`, the closing message, and the report as the terminal entry. None of it is in the thread's `messages`. `protocol::a_childs_work_reaches_its_stream_as_the_child_reported_it` pins the translation itself, including that a `tool_use` and the `tool_result` that claims it are **one entry that moves** rather than two rows to be paired by eye, and that a message carrying only a tool call draws no row. `turn::a_childs_tool_is_the_kind_of_work_it_actually_is` pins the kind each tool lands on, including the two the needle order decides — `NotebookRead` is a read rather than a file change, and `TodoWrite` is the agent's scratchpad rather than one.
      **Limitation, and it is a shared-model question rather than a Claude one:** a child that ends with a bare `task_updated` and **no** `task_notification` reaches a terminal state with no `outcome` and no terminal entry, so its tab shows the work and stops. See the note below — closing it needs `subagents::Streams::record` to let a report replace an empty outcome, which is a change to a file this ticket was told not to touch while ticket 04 is in flight. No capture contains the case, and the compact row still says "Completed with no result" / "Failed with no reported reason", so nothing is claimed falsely; the stream is simply less complete than the spec's "preserve the complete terminal entry" asks.
- [x] A background Claude child can continue appending to its stream after the parent root output becomes quiet or its immediate turn settles.
      — Both halves of the criterion, and only one of them can be recorded. **Root output quiet** is the recording's: `a_claude_child_work_stream_replays_and_then_continues_live` asserts on the conversation's own event order that the root's reply ("the subagent is off counting to 3 in the background") is published _before_ the first row carrying the child's own words, which is what eleven lines of forwarded subagent arriving after a `message_stop` look like from outside. **Turn settled** is `socket_turn::a_background_child_keeps_working_after_its_turn_has_settled`: it reads past the settle of the developer's own turn, opens the child's stream there and finds it `working`, then watches it gain a message, a `read` entry with its query and output, and its outcome — all with nothing in flight. That one is written rather than recorded for the reason the pre-existing `a_subagent_that_finishes_after_the_turn_still_gets_its_report_to_the_developer` gives: in `22-background-subagent` the child finishes inside its turn, and a capture cannot be made to have the other timing on demand. Every line of it is a shape taken from that recording.
- [x] Reopening or reloading the child tab replays the same recorded content and resumes live continuation without gaps or duplicates.
      — `a_claude_child_work_stream_replays_and_then_continues_live` opens the subscription while the child is genuinely working, folds replay and live together, and asserts not only the folded result but **every entry id the wire carried**, so the claim is not merely that the client's fold is idempotent; a fresh connection then replays the identical ids. `a_claude_child_work_stream_replays_after_the_server_restarts` drives a real restart — two processes over one database file — and asserts the ids, the order, the outcome, and that a work entry comes back whole (title, status, command line, output) rather than as prose that survived and work that did not. Mutation-checked: making the boot restore in `Shell::new` restore nothing turns it red.
- [x] Claude details or hierarchy that are not present on the protocol are omitted rather than inferred.
      — See **What Claude cannot prove** below for the four omissions and the evidence for each. Asserted rather than merely intended: the same test asserts `parentChildId` is `null`, `a_childs_prompt_is_not_one_of_the_things_the_child_said` asserts the prompt is not one of the child's messages, and `protocol::a_childs_work_reaches_its_stream_as_the_child_reported_it` asserts an empty `thinking` block draws nothing.
- [x] Child protocol boundaries cannot incorrectly settle or clear the root turn.
      — `a_claude_child_work_stream_replays_and_then_continues_live` counts exactly one `turn.completed` for a recording that contains a subagent's whole life — started, three commands, two messages, two endings — _and_ the two trailing `result` lines a background subagent costs. `a_background_child_keeps_working_after_its_turn_has_settled` covers the other order: after the turn has settled, the child's work and its ending produce exactly one further ending, and that one is the turn the agent's own report opened. Structurally, `turn.rs`'s subagent arm reads the turn to attribute a row and never takes it.
- [x] The recorded background-subagent fixtures prove live state, post-root activity, terminal result, and replay through the external orchestration boundary.
      — `22-background-subagent` drives `a_claude_child_work_stream_replays_and_then_continues_live` (live `state: "working"` with entries already recorded, the child's work arriving after the root went quiet, the report as the terminal entry, and replay on a fresh connection) and `a_claude_child_work_stream_replays_after_the_server_restarts`. `23-forwarded-subagent-text` drives `a_childs_prompt_is_not_one_of_the_things_the_child_said`, which is the shape `22` does not contain. All three go through `orchestration.subscribeSubagent` and `orchestration.subscribeThread` and read nothing but what a client is sent.

## What Claude cannot prove, and is therefore not drawn

This is criterion 7's substance. `turn::child_stream` carries the same list at the call site.

- **Hierarchy.** A `task_id` names a child of _this conversation_ and says nothing
  about a child of a child, so `Head::parent_child_id` stays `None`. A subagent
  that delegates further appears as what the wire shows — an `Agent` tool call in
  its own stream, drawn as a `tool` entry with no launcher, because a row
  offering to open a stream this server does not hold would be worse than an
  honest one. Ticket 06 owns hierarchy; nothing here fills it in.
- **Child-owned blockers.** A permission request from a subagent's tool arrives
  naming the tool and the session, never the subagent, so a child's request stays
  the root agent's request and `ApprovalRequest::subagent` stays `None` — spec
  story 52's "providers without child-attribution metadata retain their truthful
  root behaviour". This is evidence rather than assumption:
  `22-background-subagent`'s child is refused three times and **every refusal
  reaches this server as a `tool_result` on the child's own wire**, not as a
  request anyone could answer.
- **Reasoning.** A forwarded `thinking` block carries `"thinking": ""` and a
  signature — lines 38, 42 and 46 of the same capture — so there is nothing to
  record even before asking whether the entry vocabulary has a home for one.
- **A separate warning channel.** Claude has no child-level notice event. A
  child's error is the failed tool result that reports it, recorded as failed
  work with the error as its detail rather than lifted out into a second
  `notice` entry saying the same thing twice.

## Notes

**No shared file was touched.** `subagents.rs`, `worklog.rs`, `approval.rs`,
`orchestration.rs`, `packages/contracts/src/orchestration.ts` and
`SubagentStreamPanel.tsx` are unchanged: no new entry kind, no new payload field,
no contract change. The one file outside this ticket's own that moved is
`server/CONTEXT.md`, which gained the **Subagent moved** glossary entry. The vocabulary built from OpenCode's evidence fitted Claude's
wire as it stood, which is worth recording as a fact about the model rather than
about this ticket.

**The reducer now answers with `SubagentMoved` rather than `SubagentTask`.** One
value for both places a child is shown — the compact row's line, when the event
tells the row anything, and the child's identity, assignment and own work — so
the two cannot drift apart. `SubagentTask` itself is deliberately unchanged, so
the OpenCode driver and the work log compile untouched.

**A child ends twice on this wire and the bare ending comes first.** The
`task_updated` settles the child's state; the `task_notification` a millisecond
later spends the stream's one terminal entry on what the child actually returned.
Concluding on the bare one would spend the terminal entry on a silence and then
discard the answer, because `Streams::record` keeps the first outcome it is
given. The cost of this choice is the one case no capture contains: a child that
ends with a bare `task_updated` and _no_ notification reaches a terminal state
with no outcome and no terminal entry. Its compact row still says "Completed with
no result" / "Failed with no reported reason".

**The shared-model change that would close that gap, for whoever arbitrates
this at merge.** `subagents::Streams::record` keeps the first outcome it is
handed (`if stream.head.outcome.is_none()`). If it instead let a _reported_
outcome replace an `Empty` one — the terminal entry is already keyed `outcome`,
so it would upsert in place rather than appending a second — then this adapter
could conclude on the bare ending and improve it a millisecond later, and no
Claude child could reach a terminal state without a terminal entry. That is one
rule in a file ticket 03 was told not to touch while ticket 04 is in flight, and
Codex may want the same rule, so it is written down here rather than taken
unilaterally.

**Two duplications this ticket deliberately did not unify**, both flagged by the
standards review. `child_entry_kind` and `child_work` in `turn.rs` mirror
`opencode.rs`'s pair: the _classifier_ is per-provider on purpose and the repo
has already decided that — `worklog::opencode_item_type` sits beside
`worklog::Kind::of` saying "its first-party rules intentionally differ from
Claude's broader classifier" — and the needle sets do differ (`terminal`,
`notebook` ordering, Claude's `TodoWrite`). The _bound_ on a child's tool output
(`CHILD_OUTPUT` and `bounded`, ten lines) is genuinely provider-neutral and is
duplicated; it belongs on `subagents.rs` beside `Work`, and unifying it there
while ticket 04 writes a third copy it cannot see would collide. Both are
recorded here rather than left for a reader to rediscover.

**The fixture is paused, not edited.** `ScriptedAgent::replaying_paused_before`
inserts two `PAUSE` markers before a named line of a committed capture; every
line of the recording is replayed, in order, from the same file the golden tests
read. It exists because a recording of a subagent that ran for twenty seconds
replays in milliseconds, so a test that wants to open a child's stream _while the
child is working_ would otherwise have nowhere to stand. Nothing asserts elapsed
time; the pause buys the window.

**Fidelity gap, shared with OpenCode.** A child's `WebFetch`, MCP call or image
view lands on the generic `tool` kind where the same tool in the root transcript
gets a `web_search`, `mcp_tool_call` or `image_view` row — the same work-entry
component, a less specific icon. Widening the four-member entry vocabulary is a
decision about the shared model rather than about Claude, so it was not taken
here; `child_entry_kind` in `opencode.rs` carries the same note.

What this ticket does **not** prove is the application: ticket 07 owns the
browser-driver acceptance run.

`cargo fmt --check` is not run for this branch. `server/CLAUDE.md` records that
this tree has never been rustfmt-formatted and that the check fails on every
file.
