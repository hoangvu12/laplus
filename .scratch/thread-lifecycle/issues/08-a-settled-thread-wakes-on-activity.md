# 08 — A settled thread wakes itself on real activity

**What to build:** a conversation the developer settled comes back on its own the
moment there is real work in it again — so settling can never hide something that
needs them.

Without this, ticket 07 ships a known hole. A settled thread whose agent later
asks for permission would sit outside the inbox while blocked on a decision only
the developer can make. That is the exact failure the settle invariants refuse to
create at settle time, and it must not be reachable a minute later either.

**Real activity resets any override.** Not only does a settled thread wake — a
thread the developer pinned _active_ returns to neutral, so it can settle again
once the burst of work goes stale. Both directions go through the same event, with
the server-decided neutral reason. The contract lets a client send only the user
reason, so this reset cannot be forged by a client.

Three trigger points, each guarded so nothing is emitted unless an override is
actually set:

| When                                                  | What wakes            |
| ----------------------------------------------------- | --------------------- |
| a turn is requested                                   | any override is reset |
| the session becomes starting or running               | any override is reset |
| an approval or a question is appended to the work log | any override is reset |

The second and third are guarded narrowly on purpose. A session status arriving
_after_ the fact — ready, stopped, error — must not fight the developer's explicit
settle, so only a session coming alive counts. And only a request that blocks on
the developer wakes a thread; ordinary work-log rows do not, or a settled
conversation would wake on every tool call.

Two of the three are internal paths this server already owns — the session-set
change and the activity-append change — so this is a guarded emission beside an
event that already fires, not a new mechanism.

**Note:** the turn-requested site is also touched by ticket 09, which clears a
snooze there. The two tickets are independent — neither gates the other — but they
edit the same place, so whichever lands second should expect to merge.

**Blocked by:** 07 — Settling and unsettling (there is no override to reset until
settle exists).

**Status:** done

- [x] Requesting a turn on a settled thread returns it to neutral, and the thread
      reappears in the inbox.
- [x] Requesting a turn on a thread pinned active returns it to neutral.
- [x] Requesting a turn on a thread with no override emits no reset.
- [x] A session becoming starting or running on a settled thread returns it to
      neutral.
- [x] A session status of ready, stopped or error does **not** reset an override.
- [x] An approval request appended to the work log of a settled thread returns it
      to neutral.
- [x] A question appended to the work log of a settled thread returns it to
      neutral.
- [x] An ordinary work-log row — a tool call, a thinking row — does **not** reset
      an override.
- [x] Every reset carries the server-decided neutral reason, never the user one.
- [x] A reset is published on the thread's own feed and reaches the project list.
- [x] Where a reset accompanies another event, both are published, and the
      dispatch answers with the last of their sequences — the same shape a turn
      request already uses when it commits several events.
- [x] The reset survives a restart.

## What it turned out to be

**One seam, not three.** Every one of the three triggers is already a
`Threads::apply`, so the guarded emission went inside `apply_unless` rather than at
three call sites — which is what made the two internal paths free: the thirty-odd
places in `crate::turn` that publish a session change or a work-log row wake a
settled conversation without any of them mentioning the inbox.

- `Change::wakes_the_inbox` is the whole decision, and reading it is how a reader
  finds all three triggers at once.
- `Thread::wants_waking` is the guard the spec asks for, on all three triggers
  rather than once per trigger.
- `Threads::wake_the_inbox` is the emission, and the guard travels through the
  refusal `Threads::commit` already takes for the archive commands — so it is
  asked under the lock the fold runs under and before a sequence is taken, and two
  triggers arriving at once cannot both find an override and both emit. The cost
  is `NOTHING_TO_WAKE`, a refusal sentence no client reads, and it is a constant
  rather than a `format!` because refusing is the _ordinary_ answer: most work
  happens in a conversation nobody has settled.
- `apply_unless` grew a private `commit`, which is its old body. The wake is a
  second change against the same conversation, so it had to be able to publish
  without asking whether _it_ wakes anything.

`SessionStatus::is_working` is the narrow half of the session trigger, and it is
one function rather than a second `matches!` because `Busy::Session` reads the
same two statuses: **the settle an agent blocks is the settle a starting agent
undoes.** A test asserts the two readings agree by construction — an agent is
working exactly while `settles_turn_as` has no answer — so a status added upstream
cannot land in one and miss the other. `crate::worklog::blocks_on_the_developer`
is the narrow half of the third, asked of one row as it is appended rather than of
a whole work log: a conversation with a request already open was woken by the row
that opened it.

## What was decided along the way

**The reset follows the change that caused it.** Publishing it first would have
answered the same criteria, and it would also have published a wake beside a
change that was then refused. So `apply_unless` commits, and only then wakes — and
the answer becomes the later of the two numbers, which is what "the dispatch
answers with the last of their sequences" already meant for a turn.

**An archived conversation is not woken**, which the ticket did not ask about and
had to be decided. `Shelf::holds` is now read here as well as by both settle
commands, so the filter and the rule stay one rule: a turn in an archived
conversation is allowed — archiving is about the developer's list, never about the
agent — and the first draft cleared, on that turn, an override `thread.unsettle`
itself refuses to touch, so the developer's settle was gone the moment they
unarchived. Nothing is hidden by leaving it: the client's `effectiveSettled`
checks its activity blockers _before_ it reads either field, so a conversation
unarchived while its agent is busy does not classify as settled whatever the
fields say. Driven at the wire, because nothing refuses a turn in an archived
conversation.

**Nothing had to be added at the turn-request site**, which is worth knowing for
ticket 09: `Change::TurnRequested` reaches `apply_unless` like everything else, so
the snooze clear that ticket needs is a second arm in `wakes_the_inbox` and a
second guarded emission, not a change to `crate::orchestration::Shell::start_turn`.
The merge the two tickets were warned about did not materialise.

**Two of the three triggers are not reachable through this server's socket**, and
this is the same discovery ticket 07 made about the queued-turn guard.
`thread.turn.start` requests the turn _before_ it marks the session starting, and
every work-log row of a turn arrives after that — so by the time a session or an
approval could reset an override, the turn request already has. They are asserted
in `threads::tests`, beside the predicate that decides them, with the negative
halves that are the actual risk: five statuses that must _not_ undo a settle, and
a tool row that must not either. `tests/socket_activity_resets.rs` drives the
first trigger on the wire — the reason, the ordering beside the turn, the sequence,
both feeds, a restart, and the archived conversation.

**Two tests assert agreement rather than a list**, which is the only way the
narrow halves stay narrow: `is_working` is checked against `settles_turn_as` over
all seven statuses, and `blocks_on_the_developer` against the two work-log folds
over nine rows. Both would otherwise be a second copy of a rule this repository
already keeps one of, agreeing until somebody added a status or a request kind to
the other.

**The capability flag's second effect is now honest.** `Capabilities` and
`CONTEXT.md` both said the client's inactivity auto-settle rested on a premise
that was "only half true until ticket 08". Both now say it rests on this.

## Not done

**The window has not been driven**, at the requester's instruction and as with
tickets 03, 05, 06 and 07. The inbox reappearing in the sidebar is unchecked; what
is asserted is the state and the events, at the socket and at the two unit seams.
