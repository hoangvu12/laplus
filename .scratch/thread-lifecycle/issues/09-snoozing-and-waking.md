# 09 — Snoozing and waking

**What to build:** the developer can snooze a conversation until a time they
choose, so something they cannot deal with now comes back when they can — and can
wake it by hand if they chose the time badly.

**Snooze is an overlay, not a destination.** A snoozed thread stays active in the
data model; it is only suppressed from the inbox until its wake time passes or the
thread demands attention. That is why snooze does not belong in the same
vocabulary slot as archived or settled.

**Snooze never touches the agent.** A running session is snoozable. Snooze is a
decision about the developer's attention, not an interruption of the work — which
is the one thing that makes it different from stopping a session.

**There is no scheduler to build.** This is the most important thing to
understand before starting, because the obvious implementation is wrong:

- **A snooze expires by being read.** Once the wake time is in the past, the
  stored fields simply stop classifying the thread as snoozed. No server event
  fires when a wake time passes and nothing needs scheduling.
- **A raised hand does not clear the stored fields.** A snoozed thread whose agent
  becomes blocked on the developer, whose session fails, or whose run completes
  after the snooze was set stops _classifying_ as snoozed without spending the
  snooze. That derivation already ships in the bundled client runtime.

So the server stores two timestamps, guards them, and emits the events. The one
thing that genuinely clears a snooze is the developer re-engaging: **sending a new
message spends the return ticket.** A session starting or erroring deliberately
does not, because the snooze never paused the agent in the first place.

A wake time that is not strictly in the future is refused rather than quietly
normalised — it would create a thread that is snoozed and awake at once, carrying
snooze state it can never leave. The same comparison catches an unparseable time,
which must never be persisted.

**Note:** ticket 08 landed first and there is nothing to merge. It did not touch
`Shell::start_turn` at all: the reset lives in `Threads::apply_unless`, which every
change already goes through, so the trigger is an arm in
`Change::wakes_the_inbox` and a guarded emission in `Threads::wake_the_inbox`. The
snooze clear wanted here is a second arm and a second emission in the same two
places — and the guard is the same shape, asked through the refusal
`Threads::commit` takes so that "is there a snooze to clear?" is decided under the
lock the fold runs under. `Thread::wants_waking` is the settle half of that guard
and reads `Shelf::holds`; the snooze half wants the same archived reading, because
`thread.snooze` is refused on an archived conversation too.

**Blocked by:** 01 — Lifecycle fields reach the client as stored state.
06 — Archiving and unarchiving (snooze refuses an archived thread, and at the
socket seam the archive command is the only way to make one).

**Status:** done

- [x] Both commands are parsed before the world is consulted; blank identifiers
      are refused.
- [x] An unknown thread is refused.
- [x] Snoozing an archived thread is refused.
- [x] A wake time in the past is refused, with a sentence naming the time.
- [x] A wake time equal to now is refused — the comparison is strictly future.
- [x] An unparseable wake time is refused and never persisted.
- [x] Snoozing a thread with an unanswered approval is refused.
- [x] Snoozing a thread with an unanswered question is refused.
- [x] Snoozing a thread whose session is running succeeds — a live session is not
      a blocker — and the agent is not disturbed.
- [x] A snooze records both the wake time and the time it was snoozed at.
- [x] Waking by hand clears both.
- [x] Requesting a turn on a snoozed thread clears the snooze.
- [x] A session starting or erroring does **not** clear the snooze.
- [x] No timer, task or scheduled work is introduced anywhere for snooze expiry.
- [x] Snoozing twice is harmless and does not churn the thread's updated-at;
      waking a thread that is not snoozed is harmless the same way.
- [x] Both changes publish on the thread's own feed and reach the project list.
- [x] A subscriber on a second connection sees both.
- [x] Snooze state survives a restart, and a fresh subscriber agrees with a
      subscriber that watched it happen.

## What it turned out to be

Two changes in the vocabulary (`Change::Snoozed` and `Change::Unsnoozed`), two
parse arms, `Shell::snooze` and `Shell::unsnooze` — and, as the note predicted,
no migration and no store change, because ticket 01 added the two columns and
every change already writes the whole thread row.

Three things were not boilerplate:

**`crate::clock::epoch_millis_from_iso`**, the inverse of the renderer and the
first reader on this wire that has to go that way. Every other timestamp here was
rendered by `crate::clock` or by the registry's `strftime`, so comparing two of
them is a string comparison and needs no calendar — `Adoption` is that argument
written out. A wake time is the exception: it is the one value on this wire a
*client* originates, and it has to be judged before it is stored. It validates
the calendar by **rendering the parsed instant back and comparing**, which is
what makes the two functions inverses by construction rather than by two lists of
rules kept in step — and is what turns `2026-13-45T09:00:00.000Z` into a refusal.
That string is the case worth naming: it sorts *after* every real timestamp, so a
server comparing strings without a parser would have read it as comfortably in
the future and stored it.

**`crate::threads::Attention`**, which is `canSettle` and `canSnooze` as one
ordered list read twice. The subtlety is that it skips the session *check* rather
than filtering the answer: a conversation can be working and holding an unadopted
turn at once, `busy` answers with the first blocker in the client's order, so
dropping a `Session` from the answer would have reported nothing at all and let a
snooze hide the queued turn behind it. `a_queued_turn_behind_a_live_session_…` is
the test for exactly that.

**`crate::threads::Woken`**, which is what `Change::wakes_the_inbox` became. It
now answers a *list* of resets rather than a bool, because the two do not share
triggers — and each carries its own guard and its own refusal sentence.
`Threads::wake_the_inbox` became `Threads::woken_by`, a loop over that list, and
the answer is the last number reached, which is the shape it already had.

## What was decided along the way

**A repeat is keyed on the wake time, not on being snoozed.** This is where
snooze parts company with settle, which asks about the override alone. Snoozing
to the moment a conversation is already asleep until is the double-click and
re-emits; choosing a *different* time is a new decision and restamps both fields.
The second half is not tidiness: the client measures a raised hand against
`snoozedAt` (`threadRaisedHandWhileSnoozed` — a session that failed or a turn that
completed after the snooze wakes it early), so a second snooze carrying the first
one's stamp would have been woken immediately by the work the developer had just
decided to sleep through. `Lifecycle::asleep_until` is asked by both
`Change::re_emitted_at` and the fold so a repeat cannot be a repeat to one of them
and not the other.

**An unparseable wake time takes the same branch as an elapsed one, and not the
same sentence.** The spec asked for one comparison and it is one comparison: a
string this server cannot place on a clock is not one it can call future. But
"that moment has passed" is a lie about a time this server simply does not read —
and since the sentence *is* the whole diagnostic, the two are told apart
(`Unusable`). Both name the time, because a snooze is sent from a preset menu and
"that time will not do" without saying which time is not something a developer
can act on.

**"Strictly future" is pinned where the two instants can be made the same one.**
The comparison takes the instant to compare against rather than reading the clock
(`wake_time`), which is what makes the boundary testable at all: through a socket
a client samples its clock, sends, and this server reads its own afterwards, so a
wake time of "now" has *always* already elapsed by the time the guard sees it —
and a dispatch-level test of that criterion cannot tell `>` from `>=`. Found by
review; the criterion had been ticked on two tests that would both have passed a
non-strict comparison.

**One refusal sentence for two commands.** `Shell::settle` and `Shell::snooze`
differed by a gerund and nothing else, and the copy had to carry a `Busy::Session`
arm `Attention::Snoozing` cannot produce — a sentence written to be unreachable.
`would_hide` is the one cascade, and every arm in it is reached by one caller or
the other. The *order* the blocker was chosen in stays `Thread::busy`'s, which is
the client's; this only turns the answer into words.

**`thread.unsnooze` refuses an archived conversation**, which the criteria above
do not list. It is one rule with the two that do: `Thread::wants_unsnoozing`
reads `Shelf::holds` exactly as `wants_waking` does, so an archived conversation
keeps its snooze through any amount of work — and a command that could clear
state the activity reset refuses to touch would lose the developer's decision the
moment they unarchived it. The sibling `thread.unsettle` refuses one for the same
reason.

**The two reset seams were renamed, which is wider than the note asked for.** It
said "a second arm and a second emission in the same two places", and that would
have worked — but `wakes_the_inbox` returning `true` for a change that spends only
the snooze, and `wake_the_inbox` emitting something that is not an inbox reset,
would both have been names that lie. `Change::wakes` and `Threads::woken_by`, with
`Woken` naming the two resets, is the same two places under names that survive
there being two. The rename cost twelve citations elsewhere in the crate, which
review caught: nine were intra-doc links rustdoc reported unresolved.

**The capability flag is part of this ticket**, as it was for ticket 07:
`useThreadActions.ts` refuses to dispatch either command to a server that does not
advertise `capabilities.threadSnooze`. Flipping it also draws the sidebar's
snoozed section and its "Woke" indicator, both from derivations that ship
unmodified in the client. Unlike settlement there is no premise here waiting on a
later ticket — a snooze expires by being read, so nothing has to happen for one
to end. `socket_conformance.rs` loses the declaration that said the commands were
missing.

## Not done

**The window has not been driven**, at the requester's standing instruction and
as with tickets 03, 05, 06 and 07. Every criterion above is asserted through the
socket in `tests/socket_snoozing.rs` or at the two unit seams; the sidebar's
snooze presets, its snoozed section and the "Woke" indicator are unchecked.

**`thread.snooze` is refused on a queued turn, and that is not driven through the
socket** — it is `crate::threads::fold`'s unit test, for ticket 07's reason: the
state is not reachable through this server's own dispatch, because
`thread.turn.start` writes the message and then the turn in one command. The
guard exists because it is the client's rule and the client folds shells this
server did not write.
