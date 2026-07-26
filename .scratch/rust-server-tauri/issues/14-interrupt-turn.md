# 14 — Interrupt a turn

**What to build:** The developer can stop the agent mid-turn when it is heading the
wrong way, and immediately send a correction. Interruption leaves the conversation
in a usable state — partial output is kept and clearly marked as interrupted,
rather than vanishing or masquerading as a complete answer.

**Blocked by:** 10 (One complete agent turn, streamed).

**Status:** ready-for-human

- [x] An in-flight turn can be interrupted from the UI — `thread.turn.interrupt`
      becomes a `control_request` with `{"subtype": "interrupt"}` on the agent's
      stdin
- [x] Streaming stops promptly rather than running to completion in the
      background — the CLI aborts, and `thread.turn-interrupt-requested` settles
      the turn on the click rather than on the agent's `result`
- [x] Output produced before the interrupt is retained in the transcript and
      marked as interrupted — the CLI hands the partial reply over whole, and the
      turn lands in `interrupted` state with it
- [x] A new message can be sent immediately afterwards in the same conversation —
      including *while the stopped turn is still winding down*, which is the one
      race this ticket creates
- [~] Interrupting during a tool call leaves no orphaned child process —
      **the CLI's, not this server's**, and recorded rather than tested. See the
      comments below
- [x] Interrupting when no turn is in flight is a no-op rather than an error — at
      all three layers, and for a stale `turnId` as well as for no turn at all
- [x] The agent subprocess survives the interrupt and is reused for the next turn
- [x] Tests drive interrupt-then-continue through the socket boundary —
      `tests/socket_interrupt.rs`, nine of them

## Comments

### The channel was in the binary again, and the request is one line

Ticket 13 found the permission channel by reading the 265 MB `claude` binary,
because nothing documented it. The interrupt channel is the same envelope going
the other way, and it was found the same way:

- **The request is a `control_request` on stdin** carrying
  `{"subtype": "interrupt"}` and an id this server mints. The CLI's own schema
  has an optional `reason` beside it — "@internal Why the turn was interrupted,
  forwarded to the turn's AbortSignal.reason. Tool implementations branch on
  it" — which this server does not send, because a developer pressing stop has
  no reason to give beyond having pressed stop.
- **No flag turns it on.** `--input-format stream-json` is itself a control
  channel; the CLI reads a `control_request` on it whether or not it is mid-turn.
  That is the whole difference from the permission channel, which needed
  `--permission-prompt-tool stdio`.
- **The answer is a `control_response` naming the same id**, carrying
  `{"still_queued": []}` — the turns the CLI is holding for a host that queues
  several. This server sends one at a time, so it is always empty.

### The finding that shaped the design: a stopped turn looks exactly like a failed one

`11-interrupted-turn.ndjson` ends `"is_error": true`, subtype
`error_during_execution`, `terminal_reason` `aborted_streaming`. There is nothing
in the CLI's output that distinguishes "the developer pressed stop" from "the
turn went wrong".

So the distinction has to be this server's own knowledge that it asked, and that
is `InFlight::stopped`. A server that read the wire and believed it would show a
developer a red error row for work they deliberately cancelled — every time.

`Ending` is where that is spent: three outcomes where the wire has two, worked
out once and read four times (the summary, the tone, the payload's `isError`, the
session status). It replaced three separate `match`es on the same pair of
booleans, which was three chances to disagree.

### Two things settle the turn, and the first one is the click

`thread.turn-interrupt-requested` is in the contract and the client's reducer
moves the latest turn to `interrupted` on it *immediately*
(`threadReducer.ts:204`) — no round trip to the agent. Publishing only the
session change would have meant the turn kept reporting itself as running for as
long as the agent took to wind down, which is exactly the criterion about
streaming stopping promptly.

The reducer requires `payload.turnId` and folds an event without one as
`unchanged`, so the event carries it. The session follows when the agent's
`result` arrives, as `interrupted` — one of the contract's own statuses, and one
`threads::settle` already knew how to move a turn on.

### The race this ticket creates, and the one line that answers it

Stopping the agent is what re-enables the composer. So the developer can dispatch
the next turn *before* the CLI has finished aborting the last one — and the
dispatch has already moved the session's `activeTurnId` on to the new turn by the
time the old turn's `result` lands. Settling the session then would announce a
turn that had just started as finished.

The driver therefore publishes the end-of-turn session change only when the
session is still describing the turn that is ending
(`threads.active_turn(..) == active`).
`a_correction_sent_while_the_old_turn_winds_down_is_not_settled_with_it` drives
it, with a script that pauses *after* being written to so the window is a second
wide rather than a race the test would lose. Deleting the guard fails that test
and nothing else — which is how it was found to be untested in the first place,
and why the test exists.

### Cancelling a permission is the same fact, and ticket 13 said so

Ticket 13's known costs: "A rejected request is not distinguished from a
cancelled one on the wire. Both are a `deny`; `cancel` adds `interrupt: true` …
Nothing here drives the interrupt half — that is ticket 14's territory."

It is one field now. A cancel that reaches the agent records the same
`InFlight::stopped`, publishes the same two changes, and ends the turn the same
way. `15-permission-cancelled.ndjson` is the recording that settles it, and its
turn ends identically to `11`'s — `[Request interrupted by user]` and all.

### An agent that will not stop

No recording contains one; the CLI acknowledged all four. The `control_response`
is read anyway, and a non-`success` answer publishes `turn.interrupt-failed` and
puts the turn *back* — because a turn marked stopped that nobody managed to stop
would report a normal ending as one the developer asked for. Beyond the ticket's
letter, and the alternative is silent: a stop button that reports success over an
agent still going. The same reasoning covers a write that fails, so the flag is
set only *after* the line has reached the child.

### The five recordings

`tools/interrupt-capture/record.mjs` made `11`–`14`;
`tools/permission-capture/record.mjs` grew a `cancel` decision and made `15`.
Neither can be produced by hand, and `11`–`13` cannot be produced by a clock
either — at four seconds the model was still thinking, and at twenty the whole
turn had finished. The recorder triggers on the thing it is waiting for: the
fortieth text delta, or the second `tool_use` block.

`13` is the one worth reading. The interrupt lands as the agent is opening its
second `Write`, and the recording contains **no half-announced call**: this
server reads tool calls off the buffered message that closes the block, and that
block never closed. So an interrupt cannot leave an invocation row with no result
in the ordinary case — it is reachable only in the narrow window between the
buffered message and the `tool_result`, and nothing in five recordings hit it.

The stand-in learned one thing from these: a recording containing a
`control_response` is replayed with a stop *before* it, where a `control_request`
gets one after. The asymmetry is the direction of the missing line — after a
question the server has to answer, before an answer the server had to ask for.

### What is not tested, and why

**"Interrupting during a tool call leaves no orphaned child process" is
delegated, not driven.** The child in question is the *tool's* — a `Bash` running
something — and the CLI owns it. `13` is the evidence that it kills it: the turn
ends 6.2 seconds in, on a run of writes that had eight more to do. The scripted
stand-in never spawns a tool child, so no test in this suite could observe one;
what the tests do assert is the half this server owns, which is that the `claude`
process is still alive after the stop and is reaped when the session really ends.

**The real UI has not been driven.** The spec's rule is that the window is
upstream's and is exercised manually at each build-order milestone; that pass has
not been run in this session. Everything the composer reads is driven through the
socket, including the contract event the stop button's effect depends on.

### The line budget

The server is at 20,265 lines against the spec's "roughly 20K" signal to stop and
re-evaluate — up about 800 for this ticket, the large majority of it documentation
and tests-in-module. **The threshold ticket 13 predicted has now been crossed.**
Ticket 13 said the re-evaluation "should happen before ticket 14 rather than after
ticket 18"; it did not, and this is the second ticket to say so. Eleven tickets
remain, one of them substantial (20, the turn and thread diffs).

The five recordings add about 190 KB to `fixtures/`, which is evidence rather
than code.

### What review caught

`/code-review` ran both axes.

The **Spec** axis found the two things that are now in the design: the contract's
own `thread.turn-interrupt-requested` was not being published at all, so the turn
kept reading as running until the agent's `result`; and the conditional session
settle was live but undriven by any test. It also correctly flagged the
orphan-child criterion as argued rather than tested, which is why the checkbox
above is `[~]` and the section above says so. Its one wrong call was that the
conditional settle is dead code — it read `Change::TurnRequested`, which does not
touch `activeTurnId`, and missed the `Change::Session` that `start_turn` publishes
a few lines later, which does.

The **Standards** axis found three things worth the pass: the recorder's usage
block named a fixture that had been renamed, the fixtures README claimed all five
recordings contained a `control_response` when `15` contains none, and the same
pair of booleans was being switched on in three places — which became `Ending`.
It also found `InFlight::stopped` being poked by four free functions, which is now
three small methods on `InFlight` (`stop`, `awaiting`, `carries_on`) with a unit
test of their own.
