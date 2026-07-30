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

**Status:** ready-for-agent

- [ ] Both commands are parsed before the world is consulted; blank identifiers
      are refused.
- [ ] An unknown thread is refused.
- [ ] Snoozing an archived thread is refused.
- [ ] A wake time in the past is refused, with a sentence naming the time.
- [ ] A wake time equal to now is refused — the comparison is strictly future.
- [ ] An unparseable wake time is refused and never persisted.
- [ ] Snoozing a thread with an unanswered approval is refused.
- [ ] Snoozing a thread with an unanswered question is refused.
- [ ] Snoozing a thread whose session is running succeeds — a live session is not
      a blocker — and the agent is not disturbed.
- [ ] A snooze records both the wake time and the time it was snoozed at.
- [ ] Waking by hand clears both.
- [ ] Requesting a turn on a snoozed thread clears the snooze.
- [ ] A session starting or erroring does **not** clear the snooze.
- [ ] No timer, task or scheduled work is introduced anywhere for snooze expiry.
- [ ] Snoozing twice is harmless and does not churn the thread's updated-at;
      waking a thread that is not snoozed is harmless the same way.
- [ ] Both changes publish on the thread's own feed and reach the project list.
- [ ] A subscriber on a second connection sees both.
- [ ] Snooze state survives a restart, and a fresh subscriber agrees with a
      subscriber that watched it happen.
