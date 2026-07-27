# 13 — Permission prompts

**What to build:** When the agent asks permission to do something, the developer
is asked, and their answer decides what happens. Approving lets the agent proceed.
Rejecting returns control to the agent cleanly so the session continues — a
declined action must never kill the conversation.

This is the developer's control surface over what runs against their code, so both
paths need to be solid, not just the happy one.

**Blocked by:** 12 (Tool-use round-trips).

**Status:** done

- [x] A permission request from the agent surfaces in the UI describing what is
      being asked — an `approval.requested` activity carrying the `requestId`,
      the `requestKind` and the `detail` that `derivePendingApprovals` folds its
      pending-approval panel out of, plus the id of the tool call it is about
- [x] Approving allows the action to proceed and the turn to continue
- [x] Rejecting returns control to the agent and the session remains usable —
      the denial reaches the model as the tool's result, the turn ends `ready`
      rather than in error, and the *same* child takes the next turn
- [x] The conversation can continue with further turns after a rejection
- [x] A permission request left unanswered does not deadlock the session or leak
      the subprocess — and, the part the criterion does not say, does not leave a
      panel behind that the developer can never clear
- [x] The permission mode in effect is visible, so the developer knows how much
      latitude the agent has — **already true before this ticket**, from ticket
      10's `session.init` activity and the session's own `runtimeMode`; what this
      ticket adds is the test that pins it, and the argv assertion beside it
- [x] Scripted fake-agent captures cover approval, rejection, and an unanswered
      request — all three are **recordings**, and a fourth covers "always allow
      this session"
- [x] Tests drive all three paths through the socket boundary

## Comments

### The channel this needed is not in the CLI's help, and reading the binary is
### what found it

Everything before this ticket was the agent talking. A permission prompt is the
agent *asking*, and nothing in `--help`, in the spike's write-up, or in the six
captures already committed said how it does that. What upstream does is not
directly portable either: t3code hands the Agent SDK a `canUseTool` callback, and
lightcode has no SDK — that is the whole premise of the project.

So the answer came out of the 265 MB `claude` binary, and then out of four
recordings made against it. It is worth writing down what was found, because the
next person to look will not find it in documentation:

- **`--permission-prompt-tool <tool>` is a hidden flag**, `hideHelp()` in the
  CLI's own argument table, described as "MCP tool to use for permission prompts
  (only works with --print)". Its value is normally an MCP tool name. **`stdio`
  is a reserved value** meaning "ask me, on the pipes you already have" — the
  binary's dispatch reads `if (mode === "stdio") return createCanUseTool(...)`,
  and it is exactly what the Agent SDK passes when a host supplies a `canUseTool`
  callback.
- **The prompt is a `control_request`** on stdout with
  `"subtype": "can_use_tool"`, carrying the tool, its input, the `tool_use` id it
  belongs to, a one-line `description`, and `permission_suggestions`.
- **The answer is a `control_response`** on stdin, doubly nested: a
  `control_response` carrying a `success` result carrying the decision. The
  decision is `{"behavior": "allow", updatedInput?, updatedPermissions?}` or
  `{"behavior": "deny", message, interrupt?}`.

`tools/permission-capture/record.mjs` is the client that was written to prove it,
and it is committed because a permission capture cannot be made by hand:
everything after the request is a consequence of the answer.

### The first recording came back with no permission request in it

The prompt was "run the bash command `echo hello`", in the default permission
mode, and the CLI ran it without asking. That is not a bug in the setup — the
CLI classifies safe commands and does not prompt for them — but it is the kind of
thing that makes a feature look implemented when nothing is wired up. A `Write`
always asks. It is noted in the recorder for the next person who tries.

### What the four recordings settled

- **`07`, approved.** The input goes back as `updatedInput` and the tool runs.
- **`08`, declined.** The tool result is the deny *message*, with
  `"is_error": true`, and the turn's own `result` is `"is_error": false`. So a
  rejection is a failed tool call and a successful turn, which is precisely the
  ticket's "returns control to the agent cleanly" — and it means the server has
  nothing special to do to keep the session alive, provided it actually sends the
  denial.
- **`09`, unanswered.** Closing stdin closes the permission stream with it. The
  tool comes back as `Tool permission request failed: AbortError`, the model
  retries twice, gives up, and the turn ends normally. **Nothing hangs and
  nothing is orphaned**, which is most of the fifth criterion answered by the CLI
  rather than by this server.
- **`10`, for the session.** Two `Write` calls and one request, because
  `permission_suggestions` handed back as `updatedPermissions` made the CLI stop
  asking. That is what "Always allow this session" is; without it that button
  would silently mean "once".

### The client has no pending-approval collection — it folds one out of the work log

This was the load-bearing discovery on the *other* protocol, and it made the
ticket much smaller than it looked. `OrchestrationThread` has no approvals field
and there is no approvals subscription. `derivePendingApprovals`
(`apps/web/src/session-logic.ts`) walks the thread's activities, opens a pending
approval on `approval.requested` and closes it on the `approval.resolved` naming
the same `payload.requestId`. That is the entire contract.

So this ticket adds no new event type and no new shape to the wire. It adds two
activity kinds and one command (`thread.approval.respond`), and the panel appears.

Three consequences worth naming:

- **`payload.requestKind` is not optional in practice.** A request whose kind is
  none of `command`, `file-read`, `file-change` is *dropped from the panel
  entirely* — silently, so the developer would see a stopped conversation with
  nothing to click. `worklog::request_kind` therefore always answers with one of
  the three, and upstream's own fallback (a command) is the one that overstates
  rather than understates what is at stake. It is deliberately **not** the
  work-log classification beside it: a `Read` is a `dynamic_tool_call` as a row
  and a *file-read* as a question.
- **`hasPendingApprovals` is derived, not counted.** A separate counter would be
  a second answer to the question the client already answers for itself, and the
  two would agree until they did not. Deriving it also means it survives a
  restart for free, because the activities do.
- **An unresolved request is durable.** The activities are stored, so a request
  left open is a composer the developer cannot type into — on this run and on
  every run after it, since `isComposerApprovalState` disables the composer. That
  is what turned the fifth criterion from "does not deadlock" into the two extra
  things below.

### The two things the unanswered path needed beyond not hanging

- **The driver settles whatever is outstanding when it ends.** Normal exit,
  agent death, shutdown, the project being deleted — all of them run the same
  `Driving::settle`, which publishes `approval.resolved` with `cancel` for every
  request still open. Cancelled is what actually happened.
- **Answering a request the session does not know is refused *in the words the
  client recognises*, carrying the id.** `derivePendingApprovals` also drops a
  request when a `provider.approval.respond.failed` activity names it and its
  detail matches one of a fixed list of phrases — both, or nothing happens. That
  is the escape hatch for the one case the first bullet cannot cover: a server
  killed hard, leaving a panel with no settle behind it. The first click clears
  it. It is published from two places, because a decision can fail to land in two
  ways: the driver does not recognise the id, or there is no session to route it
  to at all.

`tests/socket_permissions.rs` drives both across a **restart**, because that is
where the failure would actually be felt.

### The decision channel is separate from the prompt channel, and that is the
### deadlock

The driver already had one channel of turns, and it deliberately does not read a
second prompt while a turn is in flight — a queued turn waits. An answer is owed
to the turn *in flight*. Putting both on one channel would have parked the
decision behind a prompt the driver was deliberately not reading, which is the
deadlock this ticket is about, arriving by the back door. So `Live` carries two
senders and the `select!` polls the decision arm unconditionally.

The agent is written to *before* the resolution is published, for the matching
reason: the panel closing is what the developer sees, and the write is what
unsticks the conversation. If the write fails the row still goes up, as a
failure, because the alternative is a panel that can never be cleared.

### Two vocabularies, joined in one place

The composer sends four decisions — `accept`, `acceptForSession`, `decline`,
`cancel`. The wire has two behaviours with a modifier on each. They are kept
apart: `protocol::Answer` is what `claude` accepts, `worklog::Decision` is what
the developer clicked, and `Decision::answer` is the only place they meet. A
mistake there is the worst mistake this feature can make — approving what was
declined — so it is one function with a test per decision, and the socket tests
assert on the line the *agent* was written rather than only on the row the client
would render. A server that published a decline and sent an allow would satisfy
every assertion of the second kind.

Two smaller decisions inside it:

- **The input goes back unedited.** `updatedInput` is where a host that wanted to
  alter the call would put its alteration; approving a call the developer did not
  read would be the opposite of the point.
- **"For the session" is the CLI's own suggestion handed back**, not a rule this
  server composed. The CLI knows what would stop it asking about this call, and
  inventing a broader rule than it offered would grant latitude nobody chose. A
  request that offered nothing is approved *once*, and the row says "Approved"
  rather than "Approved for this session" — a row claiming a rule the agent never
  received would have the developer expecting silence they will not get.

### `approval-required` still passes no `--permission-mode`, and now that is right

Ticket 12 left `permission_mode_for("approval-required")` returning `None` with a
comment saying it was ticket 13's to make right. It is right now, and the fix was
not to that function: the CLI's own default *is* to ask, and what was missing was
anywhere for the asking to go. `--permission-prompt-tool stdio` is passed on
every session — including `bypassPermissions`, where the CLI never asks and the
channel stays silent — because what the flag selects is where a prompt goes
rather than whether there is one. Making it conditional would mean a mode changed
mid-conversation could leave a running agent unable to ask.

### The session stays `running` while the developer decides

`OrchestrationSessionStatus` is `idle | starting | running | ready | interrupted |
stopped | error`. There is no `waiting`, and a status outside that union fails the
client's decode of the whole session — so a server that invented one would blank
the session rather than describe it. `running` is also true: the turn has not
ended. What tells the developer they are being waited on is the panel, which is
the thing designed to.

### `thread.approval.respond` answers with the log position, not a number of its
### own

Every other command here commits something and answers with the sequence it
committed at. This one commits nothing: the events it causes — the resolution
row, and whatever the agent does next — are published by the driver once the
decision has actually reached the child. Taking a sequence here would number a
change that had not happened, and the client drops anything at or below the
number it holds, so the row that *did* happen could be dropped as stale.

What the client needs from the answer is whether the decision landed, and that it
gets: a decision with no session behind it, or one this server cannot read, is a
typed failure with a sentence the composer shows.

### What review caught

`/code-review` ran both axes. The Spec axis found one real bug and one vacuous
test, and both were worth the pass on their own.

- **The escape hatch did not work — neither half of it.** The failure row was
  built with `Activity::failed`, whose payload is `{"detail": summary}` and
  carries **no `requestId`** — and `derivePendingApprovals` requires the id *and*
  the wording, both, before it will close a request. So the row rendered as an
  error beside a panel that stayed. Worse, in the case the hatch exists for — a
  session gone, a stale panel back from disk — `Threads::answer` returned an
  error *before* anything was published at all, so there was no row of any kind.
  The panel would have been permanent, which is exactly what the criterion above
  says must not happen.

  There is now one constructor for it (`worklog::unanswerable`), it carries the
  id and the phrase, and it is published from **both** places a decision can fail
  to land: the driver, for an id it does not recognise, and the dispatch, for a
  conversation with no session. `a_decision_for_a_conversation_with_no_session_still_clears_the_panel`
  drives the second across a restart, because a restart is the only way to reach
  it. The socket tests previously asserted on the row's `summary`, which is not
  the field the client reads; they assert on `payload.requestId` and
  `payload.detail` now.

- **The subprocess-leak assertion was vacuous.** It read `live_agents()` on the
  *restarted* server, where the count is zero because nothing ever ran there.
  `TestServer::stop` consumes the server, so the gauge has to be read before it —
  which meant ending the session some other way. Deleting the project does it,
  and the two halves are now two tests: one drives the leak against a live
  server, the other drives the durable panel across a restart.

- **The panel and the tool row no longer collapse into one.** See the known costs
  below; the review found it and it is now pinned by an assertion rather than
  argued about.

The Standards axis found the module's own rule being broken by the module next
door: `threads.rs` was matching `"approval.requested"` and `"approval.resolved"`
as string literals that `worklog.rs` had just minted, which is precisely what
`Kind` exists in that file to prevent. The fold moved to `worklog::unanswered`,
beside the constants and beside the client code it mirrors, and `to_shell_value`
calls it. Three smaller things went the same way: a `Call` was being constructed
with a knowingly-invalid id purely to borrow its formatting method (now a free
`request_line`), `resolved` allocated a whole `Answer` to ask a yes/no question
(now `Decision::remembers`), and `request_kind` had an arm identical to its own
fallback.

### Known costs

- **A permissioned tool call renders as two rows, not one.** The CLI announces
  the `tool_use` block *before* it asks about it, so the work log reads
  `tool.updated`, `approval.requested`, `approval.resolved`, `tool.completed` —
  and the client collapses an invocation into its result only when the two are
  **adjacent** (`collapseDerivedWorkLogEntries` merges with the row before it,
  and only for `tool.*` kinds). The approval rows are between them, so ticket
  12's one-row-per-call becomes two rows for any call that needed permission, the
  first left showing as still running.

  Nothing on this side fixes it. Renaming the approval rows to `tool.*` is what
  would make them adjacent, and it is also what would take them out of
  `derivePendingApprovals` — the panel is worth more than the collapse. Holding
  the invocation back until the decision is made would work and is worse: it
  would mean the developer cannot see what the agent is about to do while being
  asked whether it may. The order is asserted in
  `approving_lets_the_action_proceed_and_the_turn_finish` so that a change to it
  is a failing test rather than a discovery.
- **A request the CLI abandons while the session lives on stays open.** The
  driver settles outstanding requests when it *ends*; nothing settles one the
  agent stopped waiting for mid-session. No recording contains that — in `09` the
  CLI abandons the request only because stdin closed, which is the session
  ending — so there is nothing to drive it against, and answering it would
  produce the unanswerable row rather than a wedge.
- **A rejected request is not distinguished from a cancelled one on the wire.**
  Both are a `deny`; `cancel` adds `interrupt: true`, which the CLI honours by
  stopping the turn. Nothing here drives the interrupt half — that is ticket 14's
  territory, and it is where an interrupted turn learns to say what it left
  unfinished. The decision is sent correctly today; what is untested is what the
  CLI does with `interrupt` on a turn this server is driving.
- **A stale panel needs one click to clear.** See "the two things the unanswered
  path needed". The alternative — the server rewriting stored activities at
  restore — would mean minting sequences for changes nobody made, at boot,
  silently. One refused click is the smaller cost and it is the mechanism
  upstream built.
- **`hasPendingApprovals` is derived on every shell summary.** A linear scan of
  the thread's whole work log, per change that reaches the project list. Deltas
  and activities do not reach it, so this is a handful of scans per turn over a
  list that is hundreds long at worst. It is the cost of there being one
  definition rather than two; a cached count is what to reach for if a thread's
  work log ever gets long enough to notice.
- **`AskUserQuestion` is not handled.** It arrives as a `can_use_tool` request
  like any other, so it renders as an approval rather than as the question it is,
  and approving it lets the model ask itself. `hasPendingUserInput` stays `false`
  for that reason. The `user-input.*` half is a separate contract with a separate
  panel and it is not this ticket's.
- **`ExitPlanMode` is likewise just a tool.** Upstream denies it deliberately and
  captures the plan; here it is a permission request like any other. Ticket 12
  noted that rendering a plan as a plan was "ticket 13's" — it is not, on
  inspection: a plan is `turn.proposed.*` and `proposedPlans`, which is a third
  contract again.
- **The real window.** The spec's rule is that the UI is upstream's and that the
  real one is driven manually at each build-order milestone. That pass has not
  been run in this session. Everything the panel reads is driven through the
  socket, including the two kinds and the one id it folds on.

### The line budget

The server is at 19,458 lines against the spec's "roughly 20K" signal to stop and
re-evaluate — up about 1,300 for this ticket, of which roughly half is
documentation and tests-in-module. Ticket 12 said the 20K signal would be reached
before the remaining tickets were, and that is now a few hundred lines away rather
than a prediction. Twelve tickets remain, one of them substantial (20, the turn
and thread diffs). **This is the point the spec asked to stop and re-evaluate at**,
and it should happen before ticket 14 rather than after ticket 18.

The four recordings add about 110 KB to `fixtures/`, which is evidence rather
than code.
