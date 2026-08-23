# Research: do subagent tool calls leak into the main thread?

Date: 2026-08-23

## Question and source policy

The report from a developer: _"it seems to show the subagent tool calls in
main thread? (claude or codex or opencode, I don't remember)… if the tool calls
belong to a subagent, don't show them in the main thread."_ This note answers,
per driver, whether the child's **inner** work (its reads, edits, bash runs,
greps) reaches the parent conversation timeline. One compact lifecycle row per
child in the parent is intended and is not counted as a leak; the prior art is
[research.md](research.md), whose routing rule this audit holds drivers to:
child events keyed by stable child id into the child stream, parent keeps only
lifecycle/index rows.

Evidence is local source and recorded fixtures. Claims are labelled PROVEN or
SUSPECTED. Every relevant server suite was run at HEAD (`44b416d`) and passes:
`protocol_golden` (7/7), `socket_turn -- subagent` (2/2),
`socket_opencode_turn -- subagent` (2/2), `socket_codex_turn
codex_subagent` (1/1), and the client `apps/web/src/session-logic.test.ts`
(65/65).

## Executive summary

**No driver leaks the child's raw inner tool calls into the main thread at
HEAD.** All three adapters route child-attributed events away from the parent
transcript before anything is published, and each route is pinned by a recorded
fixture plus a socket test that asserts the negative (the child's words are not
in the parent snapshot). The original Claude leak the user describes —
eleven of sixteen transcript entries belonging to the subagent — was real and
was fixed on 2026-08-04 (`cbae605`, "Give a Claude subagent a row, and stop it
speaking as the agent"), before the 0.1.9 release.

What remains are three narrower findings:

1. **PROVEN (mechanism), SUSPECTED (as the user's complaint): duplicated stale
   child rows in the parent work log.** Each driver redraws the compact child
   row once _per child event_, as a fresh activity appended to the thread
   ([opencode.rs:2090-2096](../../server/crates/laplus-server/src/opencode.rs#L2090-L2096),
   [codex.rs:690-701](../../server/crates/laplus-server/src/codex.rs#L690-L701),
   [protocol.rs:1857-1909](../../server/crates/laplus-server/src/protocol.rs#L1857-L1909)
   via [turn.rs:957-967](../../server/crates/laplus-server/src/turn.rs#L957-L967)).
   The client collapses those updates only when they are **adjacent** in the
   activity list ([session-logic.ts:839-877](../../apps/web/src/session-logic.ts#L839-L877)).
   Any other activity landing between two updates — the parent's own reasoning
   row ([opencode.rs:1456-1466](../../server/crates/laplus-server/src/opencode.rs#L1456-L1466),
   [codex.rs:587-592](../../server/crates/laplus-server/src/codex.rs#L587-L592)),
   a context-meter row — splits the chain, and the main timeline then shows
   **several "Subagent …" rows at once**, each frozen with a different stale
   preview of the child's latest command/prose. That is child tool activity
   visibly repeated in the main thread, matches the complaint's wording, and no
   test covers it (the existing tests filter rows by key and never assert a
   count of one;
   [socket_opencode_turn.rs:2727-2743](../../server/crates/laplus-server/tests/socket_opencode_turn.rs#L2727-L2743)).

2. **Codex draws more child-related rows in the parent than the other two, by
   design**: one operation row per `collabAgentToolCall`
   (Spawned/Sent input/Waited/Closed,
   [codex.rs:780-816](../../server/crates/laplus-server/src/codex.rs#L780-L816))
   beside the long-lived child row. Ticket 04 argues they are separate
   lifecycles, but a reader who has not read that argument sees a scatter of
   hammer-icon rows about the subagent in the parent transcript.

3. **Claude has one latent, unobserved leak route**: streaming deltas are not
   filtered by `parent_tool_use_id` because the envelope never decodes it
   ([protocol.rs:43](../../server/crates/laplus-server/src/protocol.rs#L43),
   [protocol.rs:1915-1934](../../server/crates/laplus-server/src/protocol.rs#L1915-L1934)),
   while the server launches the CLI with `--include-partial-messages` _and_
   `--forward-subagent-text` together
   ([agent.rs:248-263](../../server/crates/laplus-server/src/agent.rs#L248-L263)).
   Recorded evidence says the CLI forwards children as buffered envelopes only
   (fixture 22 has zero child `stream_event`s), so nothing leaks today; a CLI
   release that starts streaming child deltas would stream them straight into
   the parent's live text.

| Driver   | Child events into parent transcript | Duplicate/stale child rows in parent work log       |
| -------- | ----------------------------------- | --------------------------------------------------- |
| Claude   | DOES NOT LEAK (PROVEN)              | Unlikely in practice (see §2)                       |
| Codex    | DOES NOT LEAK (PROVEN)              | Most exposed (PROVEN mechanism, SUSPECTED observed) |
| OpenCode | DOES NOT LEAK (PROVEN)              | Exposed (PROVEN mechanism, SUSPECTED observed)      |

## 1. Claude — DOES NOT LEAK (PROVEN)

### Event-flow chain

```text
claude CLI stdout (stream-json)
  ├─ {"type":"assistant"/"user", "parent_tool_use_id":"toolu_…"}   ← child envelope
  │    protocol.rs reduce(): guard arms BEFORE the plain arms
  │    [protocol.rs:1990-1996](Assistant) / [2005-2011](User)
  │      └─ SessionState::subagent_spoke [protocol.rs:1687-1781]
  │           · requires task_started to have mapped call→task_id (:1688)
  │           · refuses finished children (:1694)
  │           · child ToolUse blocks recorded into SubagentDid::Called (:1706-1719)
  │           · returns Folded::SubagentProgress — never touches self.transcript
  │    turn.rs decide() [turn.rs:957-967]
  │      ├─ moved.row  → Change::Activity(worklog::subagent(...))   ← ONE compact row,
  │      │                keyed subagent:<task_id>                   updated in place
  │      │                [worklog.rs:462-464, 579-640]
  │      └─ moved.did  → child_stream(&moved) [turn.rs:612-…]       ← child's own stream
  │    session.rs spend() [session.rs:2245-2295]
  │      ├─ changes       → threads.apply (parent transcript/work log)
  │      └─ child_streams → threads.subagents().record             ← child store only
  ├─ {"type":"system","subtype":"task_started|task_progress|task_updated|task_notification"}
  │    [protocol.rs:1857-1909] → same Folded::SubagentProgress destination
  └─ plain assistant/user envelopes (no parent_tool_use_id)          ← parent only
       transcript arms [protocol.rs:2013-2077]
```

The match order is the whole guarantee: the
`Event::Assistant(env) if env.parent_tool_use_id.is_some()` and its `user`
twin precede the plain transcript arms, so a child envelope can never reach
`self.transcript.push`.

### Fixture evidence

- [22-background-subagent.ndjson](../../server/fixtures/claude-cli/22-background-subagent.ndjson#L34-L47):
  every child message carries `"parent_tool_use_id":"toolu_017Ss3w6XvCKZE63sbzSE8CD"`,
  including the child's own `Bash` tool_use (line 35) and its three refused
  `tool_result`s (lines 37, 41, 45).
- [22-background-subagent.expected.json](../../server/fixtures/claude-cli/22-background-subagent.expected.json):
  the folded `transcript` has **5 entries, all parent-level** — none of the
  eleven child envelopes leaked. The fixture README records the history:
  before the field was read, "eleven of this capture's sixteen transcript
  entries were the subagent's"
  ([README.md:64-79](../../server/fixtures/claude-cli/README.md#L64-L79)).
- [23-forwarded-subagent-text.ndjson](../../server/fixtures/claude-cli/23-forwarded-subagent-text.ndjson#L21-L24):
  the foreground case — the child's prompt (line 21) and answer (line 24)
  arrive as ordinary envelopes distinguished _only_ by `parent_tool_use_id`;
  the golden shows neither in the transcript.
- Socket end-to-end:
  [socket_turn.rs:1338](../../server/crates/laplus-server/tests/socket_turn.rs#L1338)
  `a_background_subagent_gets_its_own_row_and_stays_out_of_the_transcript` —
  passing at HEAD.

### Recent history

- `cbae605` 2026-08-04 "Give a Claude subagent a row, and stop it speaking as
  the agent" — the original fix; contained in release 0.1.9 (`99fcab9`,
  2026-08-18), so any build newer than mid-August already routes.
- `30e4adf` "Forward what a subagent says, and put it on that subagent's row";
  `f62c4cf` 2026-08-17 opened the child work stream.
- `303c38f` 2026-08-19 fixed the inverse misattribution (a background _shell_
  drawing a subagent row), not a leak.

### Residual risk (SUSPECTED, latent)

`Event::StreamEvent` does not decode or check `parent_tool_use_id`
([enum variant](../../server/crates/laplus-server/src/protocol.rs#L43),
[reduce arm](../../server/crates/laplus-server/src/protocol.rs#L1915-L1934)):
every `content_block_delta` appends to `live_text` and publishes a root
`AssistantDelta`. This is safe only because captured CLIs forward children as
buffered envelopes and never as deltas (fixture 22 contains no child
`stream_event`). A future CLI that streams child deltas under
`--include-partial-messages` + `--forward-subagent-text` would stream the
child's prose live into the parent transcript, and nothing would retract it —
the authoritative buffered copy goes to `subagent_spoke`, not to the reconcile
path.

## 2. Codex — DOES NOT LEAK child events (PROVEN); most exposed to duplicate rows

### Event-flow chain

```text
codex app-server notification
  ├─ params.threadId != root thread id
  │    fold_notification() diverts FIRST, before any item decoding
  │    [codex_protocol.rs:333-351]
  │      └─ child_notification → ChildNotification{thread_id, Working|Said|Ran}
  │           [codex_protocol.rs:210-230]
  │    codex.rs Children: acted()/reported()/operated()
  │      ├─ child streams  → Decided.child_streams
  │      │                 ([codex.rs:1109-1197])
  │      └─ root rows only for non-nested children
  ├─ root-thread item type "subAgentActivity" [codex_protocol.rs:414-423]
  │    ConversationFold::SubagentActivity [codex.rs:690-701]
  │      ├─ children.acted(...)            → child stream entry
  │      └─ ONE compact row (collaboration_agent_row keyed agent:<thread_id>,
  │         data.childId = thread id) — skipped entirely when nested
  ├─ root-thread item type "commandExecution" etc. [codex_protocol.rs:404-410]
  │    → ordinary PARENT work rows (the parent's own commands)
  └─ ConversationFold::NestedSubagentActivity [codex.rs:706-708]
       → child stream only; draws nothing in the root transcript
```

A child's command therefore cannot become a root `ConversationFold::
CommandStarted`: the `threadId` guard runs before the item-type match, and the
golden proves it.

### Fixture evidence

- [10-subagent-work.jsonl](../../server/fixtures/codex-app-server/10-subagent-work.jsonl#L31-L32):
  the child's `commandExecution` items arrive with
  `"threadId":"child-alpha-1111"` (lines 31–32), alongside five child
  messages and child turn boundaries.
- [10-subagent-work.expected.json](../../server/fixtures/codex-app-server/10-subagent-work.expected.json):
  the root fold's `commandExecutions` is **`[]`** and `assistantMessages`
  holds only the parent's own two messages. The fixture README states the
  assertion this pins: "the child's five messages, its command and its turn
  boundaries appear nowhere in the root conversation's state"
  ([README.md:153-156](../../server/fixtures/codex-app-server/README.md#L153-L156)).
- Real recording [09-subagent-spawn.jsonl](../../server/fixtures/codex-app-server/09-subagent-spawn.jsonl):
  establishes that a child's first status frame precedes the activity naming
  it, which is why the router diverts _every_ non-root thread id rather than
  only introduced ones
  ([README.md:120-124](../../server/fixtures/codex-app-server/README.md#L120-L124)).
- Socket end-to-end:
  [socket_codex_turn.rs:2407-2415](../../server/crates/laplus-server/tests/socket_codex_turn.rs#L2407-L2415)
  asserts `!said.contains("4")` — "the child's prose belongs to its own
  stream, not the conversation" — passing at HEAD.

### Where Codex _does_ put child material in the main thread (intended, but loud)

Every root-thread `subAgentActivity` item draws another
`Change::Activity(subagent_activity_row(...))`
([codex.rs:690-701](../../server/crates/laplus-server/src/codex.rs#L690-L701),
[row builder](../../server/crates/laplus-server/src/codex.rs#L922-L941)), whose
preview is `children.latest_of(...)` — **the child's latest command or line of
prose** — and every collaboration call draws its own operation row
([codex.rs:642-688](../../server/crates/laplus-server/src/codex.rs#L642-L688)).
Combined with the client's adjacency-only collapse
([session-logic.ts:854-877](../../apps/web/src/session-logic.ts#L854-L877)),
any parent activity between two reports of the same child (a reasoning row,
a context-meter row at [codex.rs:750](../../server/crates/laplus-server/src/codex.rs#L750),
a permission row) leaves the older row standing: the main timeline accumulates
multiple hammer-icon rows per child, each quoting an inner tool call. PROVEN as
a mechanism by the code above; SUSPECTED as what the user saw, because no
capture in `fixtures/` exercises the interleaving and no test asserts a single
row per child.

## 3. OpenCode — DOES NOT LEAK child events (PROVEN)

### Event-flow chain

```text
OpenCode SSE envelope (shared feed for all sessions)
  ├─ event_session(properties) != self.session_id
  │    [routing: opencode.rs:2410-2423], [session extraction: 1398-1414]
  │      └─ known child only → child_session_event [opencode.rs:1689-1793]
  │           ├─ message.part.updated type=text    → child_said   [:1797-1833]
  │           │    → NewEntry::said in CHILD stream; row line via child_did
  │           ├─ message.part.updated type=tool    → child_worked [:1844-1877]
  │           │    → NewEntry::worked in CHILD stream (commands/reads/edits/tools)
  │           ├─ permission/question asked/replied → child_asked/answered
  │           ├─ session.error / retry             → child notices
  │           └─ everything else falls through untouched
  │    return Some(decided) — the root match below is NEVER reached
  ├─ root-session message.part.updated [opencode.rs:2468-2495]
  │    normalize_subagent first [1559-1661]:
  │      · a `task` part ALWAYS resolves Row or TooEarly — never a tool row
  │        (TooEarly falls through to *nothing*; regression-pinned by
  │         a_subagent_is_not_also_drawn_as_a_tool_called_task)
  │      · records metadata.sessionId → call in subagent_sessions [:1592-1594]
  │      · Drawn::Row = worklog::subagent keyed subagent:<callID> [:1645-1660]
  │    NotASubagent → ordinary root tool_activity
  └─ root text/reasoning parts → emit_text → parent transcript/thinking rows
```

The diversion is total: a child event returns before the root `match`, and a
root `task` part is claimed by `normalize_subagent` before `tool_activity` can
draw it, so the dispatch appears exactly once, as the compact row.

### Fixture evidence

There is no committed SSE capture of a subagent; the evidence is the scripted
peer, which composes only recorded shapes:

- [socket_opencode_turn.rs:947-1040](../../server/crates/laplus-server/tests/socket_opencode_turn.rs#L947-L1040):
  child prose (`ses_child_1` text parts), four child tool parts
  (`bash` running/completed, `read`, `grep`, `edit`, failed `webfetch`),
  a child blocker and a legacy permission — all tagged `ses_child_1`, while
  the `task` part stays on `ses_owned_1`.
- Assertions, passing at HEAD:
  [2775-2780](../../server/crates/laplus-server/tests/socket_opencode_turn.rs#L2775-L2780)
  "a subagent's words reached the transcript" must NOT fire, and
  [3057-3062](../../server/crates/laplus-server/tests/socket_opencode_turn.rs#L3057-L3062)
  repeats it after reload; the child's tools exist only behind
  `orchestration.subscribeSubagent`
  ([3029-3042](../../server/crates/laplus-server/tests/socket_opencode_turn.rs#L3029-L3042)).

### Recent history — commit 44b416d checked, no regression

`git show 44b416d -- src/opencode.rs` (2026-08-22, "one message per text
part; idle reap in progress") touches only the **root** text-part emission
model (`emitted_parts` map→Vec, per-part message ids, `close_open_parts`) and
idle-reap plumbing. Child routing (`event_session`, the `next()` diversion,
`child_session_event`, `normalize_subagent`) is untouched by that commit; the
last change in that area was `303c38f` (adding the denylist classification
helper) and before it the ticket-02/06 series (`3d9f312`, `44aa589`,
`7d8df2a`). No regression window exists around 44b416d.

Residual theoretical gap (SUSPECTED, weak): `event_session` looks for
`sessionID` in exactly three places (properties, `part`, `info`)
([opencode.rs:1398-1414](../../server/crates/laplus-server/src/opencode.rs#L1398-L1414)).
An envelope carrying a child part with the id somewhere else would be treated
as a root event; every recorded shape tags one of the three, and OpenCode's own
source attributes parts to sessions, so this is a hardening note rather than a
leak.

The same duplicate-row exposure as §2 applies here: `child_did` redraws the
compact row on **every** child event
([opencode.rs:2090-2096](../../server/crates/laplus-server/src/opencode.rs#L2090-L2096))
while the parent's own reasoning draws thinking rows
([1456-1466](../../server/crates/laplus-server/src/opencode.rs#L1456-L1466));
interleaved, the client keeps both copies.

## 4. The shared client-side root cause

`deriveWorkLogEntries` folds the thread's activities and collapses lifecycle
updates only when the previous entry is adjacent with the same collapse key
([deriveWorkLogEntries](../../apps/web/src/session-logic.ts#L692-L709),
[collapse](../../apps/web/src/session-logic.ts#L839-L852),
[shouldCollapse](../../apps/web/src/session-logic.ts#L854-L877)). The server
never withdraws an activity — `threads.apply` appends — so "one row per child"
is a _client-side inference_ that holds only while the child's row updates are
contiguous. Messages do not break the chain (they are a different list), but
thinking rows, context-meter rows, permission rows and other children's rows
do. When that happens the parent timeline shows N stale copies of the child's
row, whose `detail` fields quote the child's inner commands — the visible
phenomenon the complaint describes. Nothing asserts otherwise: the unit test
that "proves the several row updates collapse to one" feeds them adjacently.

## Fix directions (one paragraph each, no implementations)

**Duplicate/stale child rows (all drivers; the recommended target).** Make
"one row per child" a server-side fact instead of a client inference: either
have `spend()` recognize a compact child row (it already identifies them by
`data.childId` at
[session.rs:2262-2281](../../server/crates/laplus-server/src/session.rs#L2262-L2281))
and _replace_ the previous activity for the same child rather than append, or
teach the client's derivation to keep only the newest entry per
`subagentChildId`/`toolCallId` regardless of adjacency. The second is smaller
and fixes reload too, since the full history is re-derived on every snapshot;
the first also shrinks the stored activity list. Either way, add the missing
assertion — after a turn with interleaved parent reasoning, exactly one
activity per child survives derivation.

**Codex row volume.** Consider demoting collaboration _operation_ rows
(spawn/wait/sendInput/close) out of the default work log — into the child row's
expanded body or the child stream itself — since ticket 04's separation is
about correctness of state, not about needing six hammer rows in the parent;
this is presentational and independent of the storage fix above.

**Claude streaming guard.** Decode `parent_tool_use_id` on the
`stream_event` envelope and drop deltas that carry one, mirroring the buffered
arms; cheap, inert against current CLIs, and closes the only unguarded path
into `live_text` before a CLI release opens it.

**OpenCode attribution hardening.** Optional: treat an unresolvable-session
envelope that still names a known child part/message id conservatively (count
it as drift) rather than defaulting it into the root path.

## Addendum, 2026-08-23: the piling mechanism confirmed and fixed

The per-driver verdicts above stand, but the "likely thing the user actually
saw" section undersold the mechanism: with **two or more concurrent children**,
the client's adjacency-only collapse can never fire at all, not merely
sometimes. A scratch reproduction against deriveWorkLogEntries proved it:
one child's eight consecutive updates collapse to one row; two children's
interleaved updates collapse to nothing — every child event stays its own row.
That is exactly what a /research dispatch of two parallel agents draws.

Root cause chain (all PROVEN):

- Every driver emits one activity per child event under one stable key per
  child: [../../server/crates/laplus-server/src/worklog.rs](../../server/crates/laplus-server/src/worklog.rs) subagent (data.toolCallId = subagent:<id>),
  [../../server/crates/laplus-server/src/codex.rs](../../server/crates/laplus-server/src/codex.rs) `agent_row_key`, Claude's task rows likewise.
- The client folded only _adjacent_ same-key entries
  (collapseDerivedWorkLogEntries in `apps/web/src/session-logic.ts`), so
  interleaving defeated it deterministically.

Fix (landed with this addendum): subagent rows now fold by stable identity
instead of adjacency. The first activity of a child anchors the row at its
delegation point and keeps its id/createdAt; every later update of the same
child folds into that anchor wherever it lands in the timeline; a terminal row
is never reopened (stragglers start their own row). Scoped strictly to
itemType === "collab_agent_tool_call" rows carrying an explicit toolCallId,
so ordinary tool rows keep their exact previous semantics. Regression tests in
`apps/web/src/session-logic.test.ts: interleaved children stay two rows;
terminal sticks; fallback-keyed rows remain adjacency-only.
