# 15 — Session resilience: errors, process death, and drift counters

**What to build:** Things go wrong without taking the app down. An agent error is
reported in the conversation where the developer can see it and retry, not in a
crash. If the agent subprocess dies unexpectedly, the window survives and says so.
An unrecognised event from a newer CLI increments a counter instead of killing the
session.

The drift counters are the project's early-warning system for its most externally
volatile dependency: when the CLI's format moves, that should be learned from a
number, not from a bug report.

Long-session behaviour belongs here too — context compaction must not lose the
visible transcript.

**Blocked by:** 10 (One complete agent turn, streamed).

**Status:** ready-for-human

**The encoder choice this ticket owned is settled: `error`.** See ADR-0004. A
turn cut short by a dead agent is reported as a session error rather than as one
that stopped, because nobody asked for it and because `lastError` is the only
place the developer is told the agent went away — and a session that is not in
`error` has nowhere to put a sentence.

- [x] An error reported by the agent appears in the conversation and the session
      stays usable — `turn.completed` is styled as a failure and carries the
      agent's own words, which the CLI puts in `result.errors`, in `result`, or
      in neither
- [x] A turn can be retried after an error — the retry is answered by the *same
      process*, which is what says the error ended the turn and not the session
- [x] Unexpected death of the agent subprocess is reported in the UI and does not
      crash the server or the window — a `session.failed` row quoting whatever
      the child managed to say on stderr, and the socket still answers afterwards
- [x] A session whose subprocess died can be restarted without losing the
      transcript — the next turn starts a replacement and hands it `--resume`,
      observed as the argv the second process was given
- [x] An unrecognised event type increments a drift counter and the session
      continues
- [x] A malformed line increments a parse-error counter and the session continues
- [x] Drift and parse-error counts are exposed where they can actually be seen —
      in the `turn.completed` **summary**, which the work log renders, and not
      only in a payload key it does not
- [x] Context compaction during a long session leaves the visible transcript
      intact — and is reported, because a developer whose agent has just
      forgotten half the conversation would otherwise experience it as the agent
      losing the thread
- [x] Rate-limit notifications from the agent are surfaced rather than silently
      swallowed
- [x] Scripted fake-agent captures cover error results, abrupt exit mid-stream,
      unknown event types, and malformed lines
- [x] Tests drive each failure mode through the socket boundary —
      `tests/socket_resilience.rs`

## Comments

### The counters existed; what they did not do was count anywhere visible

`turn.completed` has carried `unknownEvents` and `parseErrors` in its payload
since ticket 10. The UI's work log renders a row's `summary` and its `detail`
(`session-logic.ts`), not arbitrary payload keys — so the numbers were on the
wire and in front of nobody. An early-warning system nothing warns from is not
one, and this criterion was therefore already "done" in the only sense that does
not matter.

What changed is a clause in the sentence: *"Turn completed in 2.0s · 2
unrecognised events and 1 unreadable line"*. Two decisions fell out of writing it:

- **The clause reports the turn's drift; the payload reports the session's.**
  `Drift` is a copyable pair with a `since`, and `InFlight` records where the
  session stood when the turn began. Reporting the running total in every
  sentence would have a clean turn claiming the format had moved, which trains
  the developer to skip the turn where it had.
- **A clean turn says nothing.** Nearly all of them are clean, and a clause
  asserting that on every turn is noise with the same effect.

### Three things the CLI says that this build was throwing away

Each was `Folded::Nothing` before, and each is a different kind of loss:

- **A failed `result`'s reason.** `last_error` was `"Turn failed in 0.4s"`, which
  is not something a developer can decide anything from. The CLI puts the reason
  in an `errors` array, or in `result`, or in neither and only the subtype —
  `ResultEvent::complaint` reads all three in that order.
- **`system`/`compact_boundary`.** It fell to `SystemEvent::Other`, which is
  silence. Compaction is a fact about what the *agent* can still see; the
  transcript is this server's own copy and is deliberately untouched by one. But
  a follow-up that refers to something plainly on screen may now be answered by
  an agent that no longer has it, and without a row the developer has no way to
  know that happened.
- **`rate_limit_event`.** Only the two standings that change what the developer
  can do are surfaced. The CLI emits one whenever its view of the account moves,
  which includes moving back to fine, and a row saying nothing is wrong on a
  schedule nobody chose is how a work log becomes unreadable.

The shapes of all three were read off the `claude` binary, the same way the
permission and interrupt channels were — `rate_limit_info`'s fields are the API's
own response headers in the CLI's camel case, and `compact_metadata` carries the
trigger and the token counts either side. `fixtures/claude-cli/16`–`18` are
hand-written, because none of the three can be produced to order on a healthy
account, and the README says which is which.

### A reply the agent died in the middle of is now settled rather than dropped

This was not on the list and is the change with the widest blast radius, so it is
worth stating plainly. A delta owes the database nothing — that is ticket 11's
design and the whole of why the disk is not in the streaming path — so a reply cut
short had **nothing** on disk, and stayed `streaming: true` in memory for the life
of the thread. The developer came back to a prompt nobody had answered, and while
the app was still open, to a reply the UI renders as still growing.

The driver now settles the message on its way down, which is the moment it knows
no buffered message is coming. It sends an **empty** buffered message, which is
the case where the accumulation stands rather than being replaced — so what the
developer saw stream is what is kept, and no reconciliation is recorded for one
that never happened. Forging a message out of the deltas and comparing it against
them would have reported the streaming assumption as checked on the one kind of
turn where nothing checked it.

`a_turn_the_app_closed_during_does_not_come_back_running` (ticket 11's) asserted
the old behaviour and now asserts this one. Its load-bearing half — the turn does
not come back `running` — is unchanged. The hard-kill case still loses the tail,
because the driver never runs at all. Ticket 11's own file carries a note where
it argued the old cost, so the reversal is recorded where a reader of *that*
ticket will meet it rather than only here.

### What review caught

Two axes, and both found the same defect from opposite directions.

- **An unreadable `rate_limit_event` payload was folded to silence.** It went to
  `Folded::Nothing` through the same branch as a healthy account, incrementing
  nothing — in the one module whose entire premise is that unknown variants
  degrade to counters, and on the one shape here that was read off the binary
  rather than recorded. A CLI that renamed `rate_limit_info` would have been
  invisible. It is now counted, and a unit test pins the difference between "the
  account is fine" and "this build cannot tell".
- **Drift between turns was reported by nobody.** The count was anchored to the
  start of a turn, so anything folded while no turn was running — which is
  exactly where a rate-limit notice and a compaction boundary arrive — was
  subtracted straight back out. The anchor moved from the turn to *the last
  report*, which also gave the death path something to say: a turn that never
  ends emits no `turn.completed`, so a session that died having been talking in
  an unreadable dialect now says both.
- **An unknown standing was described as a known one.** `status != "allowed"`
  meant any future literal rendered as "close to its usage limit". It is now
  named verbatim, because the word the agent used is the only thing that was
  actually read.

Smaller: `RateLimit` and `Compaction` had a `Serialize` nothing serialized;
`OnResume`'s new arms were wildcards where a fifth variant would have silently
got no turn offset; and the compaction argument was written out five times, of
which two were load-bearing and three were restatement. `CONTEXT.md` gained
*Standing*, which the change had introduced as a concept and not as a word.

### The one failure mode with no capture and no possible one

An agent that crashes has, by definition, not finished writing the recording. So
`harness::agent::DIES` is a marker that makes the stand-in complain on stderr and
exit, mid-sentence, and `ScriptedAgent::resuming_after_a_death` is what a
per-process turn counter cannot express: a replacement process has to answer turn
*two*, and its own counter starts at zero, so the scripts are keyed to the
conversation rather than to the process when `--resume` is present.

### Not verified here

- **The real window.** Same position as tickets 10–14: the spec makes UI
  rendering upstream's, and the manual pass at this milestone has not been run.
  Everything the server owes it is driven through the socket.
- **A real rate limit, a real compaction, a real API failure.** None can be
  produced to order against a healthy account, which is why `16`–`18` are
  hand-written from shapes read off the binary rather than recorded. If a `claude`
  release moves any of them, the golden files say so.
- **A `claude` that is killed by the operating system rather than exiting.** The
  stand-in exits; a `SIGKILL`-equivalent would reach this server as the same
  thing — output that stops — with no last words to quote.

### The line budget, which is now the thing to look at

The server is at **21.3K lines** of `src/`, against the spec's "roughly 20K" —
which the spec names as the signal to stop and re-evaluate rather than as a limit.
It was already over before this ticket: 20.6K at ticket 16, up from 17.0K at
ticket 11. This ticket added about 700 lines to `src/` and about 700 to the tests.

Nine tickets remain and two of them (17/18, terminals; 19–21, git) are subsystems
rather than refinements. Worth a decision before either is started, and it is not
one to take inside a ticket.
