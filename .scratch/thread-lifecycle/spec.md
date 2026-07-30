# Thread lifecycle and checkpoint revert

Status: ready-for-agent

Evidence and provenance: `.scratch/contract-parity/ledger.md`. This spec covers
items 1–4 of that ledger. Items 5–8 are separate efforts.

## Problem Statement

A developer using laplus reaches for ordinary conversation management and finds
it dead. Renaming a thread does nothing. Archiving does nothing. Settling a
finished conversation so it leaves the inbox does nothing. Snoozing does nothing.
Changing a conversation's runtime or interaction mode after it has started does
nothing. Stopping a session does nothing. Reverting the working tree to how it
looked before a turn — the thing the checkpoint machinery was built for — does
nothing.

The controls are all present, because the UI is upstream's and upstream's server
answers them. Thirteen of them dispatch a command this server refuses with
`Command not implemented by this server: <kind>`. Some surface that sentence to
the developer under a generic failure heading; others fail silently because the
UI optimistically assumes success. Either way the developer learns that parts of
the application are decoration, and there is no way to tell from the interface
which parts.

The two worst consequences are not cosmetic:

- **The inbox cannot be cleared.** Every conversation ever started stays in it
  forever. There is no archive, no settle, no snooze, and no delete, so the list
  grows without bound and the one conversation that needs attention is buried
  among the finished ones.
- **A turn cannot be undone.** This server already photographs the whole working
  tree at every turn boundary and already serves both diffs from those
  photographs. The developer can _see_ exactly what a turn changed and has no way
  to put it back.

## Solution

The server answers all thirteen commands, persists the state they imply, and
publishes each as an event carrying its sequence, so both the project list and
the thread's own feed stay in step with what the developer did.

The developer gets: rename a thread or a project; archive and unarchive; settle a
finished conversation and unsettle it; snooze one until a chosen time and wake it
early; delete one; change runtime and interaction mode mid-conversation; stop a
session; and revert the working tree to any turn boundary the thread has a
checkpoint for.

The inbox then behaves the way the interface has been promising: finished work
leaves it, snoozed work comes back when asked, and a conversation that needs the
developer raises its hand and returns to the top regardless of what it was told
earlier.

**Most of the reasoning is already in the tree and must not be rebuilt.** The
bundled `@t3tools/client-runtime` already carries the whole read-time
derivation — which threads count as settled, which count as snoozed, when a
snoozed thread has raised its hand, when it woke, and whether a settle or snooze
is even offerable. That code ships unmodified (ADR-0012) and is authoritative for
what the developer sees. This work is therefore narrower than it looks: the
server stores the lifecycle fields, enforces the invariants, and emits the
events. It does not classify.

## User Stories

1. As a developer, I want to rename a thread, so that a conversation I will come
   back to is findable by what it is about rather than by its opening sentence.
2. As a developer, I want to rename a project, so that the sidebar reads in my
   own vocabulary rather than the folder's.
3. As a developer, I want to archive a thread, so that finished work leaves my
   working list without being destroyed.
4. As a developer, I want to unarchive a thread, so that a conversation I
   archived too eagerly comes back intact.
5. As a developer, I want archived threads kept out of the project list, so that
   the list is the work I am actually doing.
6. As a developer, I want to browse my archived threads separately, so that I can
   find something I archived months ago without it cluttering the list today.
7. As a developer, I want to settle a finished conversation, so that it leaves my
   inbox and stops competing for attention with work that needs me.
8. As a developer, I want to unsettle a conversation, so that I can pin something
   back to my inbox when I decide it is not finished after all.
9. As a developer, I want a settled conversation to wake itself when its agent
   starts working again, so that settling can never hide live work from me.
10. As a developer, I want a settled conversation to wake itself when its agent
    asks me for permission, so that a request for my decision is never hidden
    behind a decision I made earlier.
11. As a developer, I want a settled conversation to wake itself when I send it a
    new message, so that re-engaging with it is enough to bring it back.
12. As a developer, I want to be refused when I try to settle a conversation whose
    agent is starting or running, so that I cannot hide work in progress from
    myself.
13. As a developer, I want to be refused when I try to settle a conversation with
    an unanswered permission request or question, so that I cannot park the agent
    waiting on me somewhere I will not look.
14. As a developer, I want to be refused when I try to settle a conversation whose
    turn I just requested and no session has picked up yet, so that work in the
    gap between asking and starting is not hidden either.
15. As a developer, I want to snooze a conversation until a time I choose, so that
    something I cannot deal with now comes back when I can.
16. As a developer, I want a snoozed conversation to stay out of my inbox until
    its wake time, so that snoozing actually buys me quiet.
17. As a developer, I want a snoozed conversation to come back on its own when
    the wake time passes, so that I do not have to remember to look for it.
18. As a developer, I want a snoozed conversation to come back early if its agent
    becomes blocked on me, so that snoozing never delays a decision only I can
    make.
19. As a developer, I want to wake a snoozed conversation by hand, so that I am
    not held to a wake time I chose badly.
20. As a developer, I want snoozing to leave the agent alone, so that snoozing is
    a decision about my attention and not an interruption of the work.
21. As a developer, I want to be refused when I try to snooze to a time in the
    past, so that I do not create a conversation that is snoozed and awake at
    once.
22. As a developer, I want to be refused when I try to snooze a conversation with
    an unanswered request, so that snooze cannot hide the agent waiting on me.
23. As a developer, I want to delete a thread, so that a conversation I started
    by mistake stops taking up space in my list.
24. As a developer, I want a deleted thread's transcript and checkpoints kept
    rather than destroyed, so that a deletion I regret is recoverable and the git
    refs a turn wrote are not orphaned.
25. As a developer, I want commands against a deleted thread refused, so that a
    stale client cannot go on driving a conversation I removed.
26. As a developer, I want to change a conversation's runtime mode after it has
    started, so that I can loosen or tighten what the agent may do without
    starting over.
27. As a developer, I want to change a conversation's interaction mode after it
    has started, so that I can move between planning and acting in the
    conversation I am already in.
28. As a developer, I want a mode I set to survive a restart, so that the picker
    shows what is actually in force next time I open the window.
29. As a developer, I want a mode I set to apply to the next turn rather than the
    one already running, so that changing it mid-turn does not change the rules
    under the agent's feet.
30. As a developer, I want to stop a session, so that I can end an agent process
    that is idle or wedged without interrupting a turn or closing the window.
31. As a developer, I want stopping a session to leave the conversation intact,
    so that I can resume it afterwards.
32. As a developer, I want to revert my working tree to how it looked before a
    given turn, so that I can undo a turn that went wrong without reading its
    whole diff and unpicking it by hand.
33. As a developer, I want a revert to restore files the turn created, modified
    and deleted, so that "before that turn" means the whole tree and not just the
    parts git was already tracking.
34. As a developer, I want to be told when a revert cannot be performed, so that
    a failed revert is not mistaken for a completed one.
35. As a developer, I want a revert to be refused when the thread has no
    checkpoint for that turn, so that I am not offered an undo with nothing
    behind it.
36. As a developer, I want the thread list to update the moment I act, so that
    archiving or settling from one window is reflected without a reload.
37. As a developer, I want my action reflected in another window I have open, so
    that two views of the same conversation do not disagree.
38. As a developer, I want every lifecycle change to survive a restart, so that
    the state I curated is still there tomorrow.
39. As a developer, I want a repeated action to be harmless, so that a
    double-click or a client retry does not corrupt what I meant.
40. As a developer, I want a repeated action not to churn ordering, so that
    settling something twice does not move it in a list sorted by when it changed.
41. As a developer, I want a refused command to say what went wrong and which
    thing it went wrong about, so that the sentence the interface shows me is
    worth reading.
42. As a developer on a phone over a tunnel, I want the same lifecycle actions to
    work, so that the mobile-shaped surface is not a second-class one.

## Implementation Decisions

### Vocabulary: inbox state is not settling

`CONTEXT.md` already defines **Settling** as reading a session status as a turn
state — a property of a _turn_, mirrored from upstream in three places and owned
here by `crate::settling`. The commands in this spec use the same English word
for something else entirely: whether a _thread_ belongs in the developer's inbox.

These must not be conflated, and the existing meaning has seniority. The glossary
gains a distinct entry — **inbox state** — for the thread-level concept, with a
cross-reference warning at **Settling**. The field names stay as the contract
spells them (`settledOverride`, `settledAt`), because renaming contract
vocabulary is not on the table; it is the prose and the Rust identifiers that
disambiguate.

This is the one decision here worth an ADR, and the collision is the reason.

### The lifecycle is stored, not derived

Six fields join the thread read model and stop being hardcoded nulls:
`archivedAt`, `settledOverride`, `settledAt`, `snoozedUntil`, `snoozedAt`,
`deletedAt`. They are already declared on the contract's thread and thread-shell
types, and the shell summary already carries everything else the client's
derivation reads — pending approvals, pending user input, the latest turn, the
session, and the latest user message time. Publishing these six is the whole of
what the client is waiting for.

**Classification stays in the client.** `effectiveSettled`, `effectiveSnoozed`,
`threadRaisedHandWhileSnoozed`, `threadWokeAt`, `canSettle` and `canSnooze`
already exist in the bundled client runtime and are not reimplemented here. In
particular:

- **There is no server-side snooze timer.** A snooze expires by being read: once
  the wake time is in the past the stored fields simply stop classifying as
  snoozed. Nothing fires, and nothing needs scheduling.
- **A raised hand does not clear the snooze fields.** It stops the thread
  classifying as snoozed, which the client already computes. The server does not
  emit an unsnooze for it.

The server does implement the settle and snooze **invariants**, because the
client's versions are explicitly a twin that exists to avoid a round trip; the
server's are authoritative.

### Schema: one appended migration

`crate::store` versions its schema by `user_version`, with one entry per version
and appending as the only supported edit. This work appends exactly one entry
adding the six nullable columns to the threads table. A released migration is
history and is not rewritten.

Nullable with no default is deliberate: an existing row read back must be
indistinguishable from a thread that has never been archived, settled, snoozed or
deleted, which is what a NULL already means everywhere else in this table.

### Commands, events and sequences

Each command joins the command vocabulary in `crate::orchestration` — parsed
before the world is consulted, so a malformed payload is refused at the door and
by the time the shell has one only the world can still refuse it. Each gains a
dispatch arm answering with the sequence it committed at, and a corresponding
variant in the change vocabulary in `crate::threads` so it reaches subscribers.

**One command may commit several events.** Starting a turn already does this and
is the precedent: it answers with the last of its numbers, and every one has been
published by the time the client reads the answer. The lifecycle resets below use
the same shape.

Each new change declares whether it reaches the project list. All of these do —
archived, settled, snoozed, renamed and deleted are exactly what the list
renders — which is the opposite of a delta or an activity.

### Invariants

Mirrored from upstream, which is the specification for the client we ship.

| Command                                                  | Refused when                                                                                                                       |
| -------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------- |
| `thread.archive`                                         | the thread is already archived                                                                                                     |
| `thread.unarchive`                                       | the thread is not archived                                                                                                         |
| `thread.settle`                                          | archived; session is `starting` or `running`; an unanswered approval or question exists; a requested turn has not been adopted yet |
| `thread.unsettle`                                        | archived                                                                                                                           |
| `thread.snooze`                                          | archived; the wake time is not strictly in the future, or is unparseable; an unanswered approval or question exists                |
| `thread.unsnooze`                                        | archived                                                                                                                           |
| `thread.delete`                                          | the thread is unknown or already deleted                                                                                           |
| `thread.meta.update`, `project.meta.update`              | the subject is unknown; the new title is blank                                                                                     |
| `thread.runtime-mode.set`, `thread.interaction-mode.set` | the thread is unknown; the mode is not one the contract names                                                                      |
| `thread.session.stop`                                    | the thread is unknown                                                                                                              |
| `thread.checkpoint.revert`                               | the thread is unknown; no checkpoint exists for that turn                                                                          |

A running session **is** snoozable. Snooze governs attention, never the agent.

The unanswered-request guard is not new logic: `crate::worklog` already derives
unanswered approvals and unanswered questions from the work log, and is already
the source of the two pending flags on the shell summary. The guard is those two,
and deriving it rather than counting is what makes it survive a restart.

### Idempotence by re-emission

A repeat of settle, unsettle, snooze or unsnooze is not refused. It re-emits, and
folding the event a second time lands on the same state. A re-emission carries the
_existing_ updated-at rather than the current time, so a duplicate neither rewinds
nor churns a list ordered by when things changed.

This is deliberately not the same thing as command-id idempotence, which this
server does not have — see the existing note on `commandId` not being remembered.

### Lifecycle resets: three guarded emissions

Real activity resets any override. A settled thread wakes; a thread pinned active
returns to neutral so it can settle again once the burst of work goes stale.

| Trigger                                             | Emits                                                                              |
| --------------------------------------------------- | ---------------------------------------------------------------------------------- |
| a turn is requested                                 | unsettled(activity) if any override is set, **and** unsnoozed(activity) if snoozed |
| the session becomes `starting` or `running`         | unsettled(activity) if any override is set                                         |
| an approval or question is appended to the work log | unsettled(activity) if any override is set                                         |

Two are internal paths this server already owns — the session-set and
activity-append changes — so the reset is a guarded emission beside an event that
already fires, not a new mechanism.

Snooze is cleared **only** by a new turn. A session starting or failing does not
spend the snooze, because snooze never paused the agent; a raised hand is handled
by the client's read-time derivation instead.

The reason on an unsettle is `activity` for all three. The contract lets a client
send only `user`, so the neutral reset cannot be forged — and the distinction is
load-bearing: a user unsettle pins the thread active, while an activity unsettle
returns it to neutral.

### Deleting is soft

Delete stamps the deletion time and keeps the row, its transcript, its work log
and its checkpoints. Three reasons: the checkpoint refs a turn wrote are real git
objects that a hard delete would orphan; the cascade on the threads table would
take the transcript with it irreversibly; and the contract carries a deletion time
on the thread, which is only meaningful if the thread survives to carry it.

A deleted thread is excluded from the project list and its subsequent commands
are refused. Whether the client also needs it withheld from the archived-shell
answer is the one behaviour to confirm against the client's own reducer during
implementation rather than assume.

### Revert restores a photograph

ADR-0008 already establishes that a turn is a photograph of the working tree, and
`crate::checkpoints` already writes one at every turn boundary under a ref of its
own, covering tracked, staged and untracked files. A revert is therefore a
restore of a photograph this server already took, not new machinery — which is
why this is small despite sounding large.

The command is answered in two stages, following upstream: the dispatch records
that a revert was asked for and answers immediately, and the restore itself is
deferred, because it touches a disk and the socket's only reader must never wait
on one. Completion is published as its own event. The contract already declares
both the request and the completion.

A revert names a turn, and the checkpoint for a turn the thread does not have is
a revert with nothing behind it — refused rather than attempted, for the same
reason a checkpoint row is never published before its tree has been written.

### The two modes are already stored

Runtime mode and interaction mode already have columns and are already published;
they are simply write-once at thread creation today, with per-turn overrides
arriving on a turn request. These two commands make them editable in their own
right. No migration, no new field, no new event shape beyond the two changes —
which is why they are the first thing to build.

### Archived threads need their own answer

The project list excludes archived threads, so reaching them needs the archived
shell snapshot the contract declares and this server does not answer. It is built
by the same snapshot builder the subscription and the HTTP endpoint already share,
filtered to archived threads: a second builder would let the world the client
draws depend on which transport answered first.

## Testing Decisions

### What a good test is here

It asserts on the decision the code made, in the vocabulary a client speaks —
never on how the decision was reached. Two rules this repository has already paid
for:

- **No test asserts on elapsed wall-clock time.** Timeouts exist to catch a hang,
  not to enforce a budget.
- **The suite is run with `--no-fail-fast`, and its output is redirected to a file
  and grepped**, never piped into `head`, which kills cargo mid-run and orphans
  the git children it spawned.

A green suite is also not evidence the application works. Every command here is
attached to a control a developer clicks, so the work is not done until the window
has been driven — the headless browser in the server's tools directory is how, and
it is the only way the UI half can be checked.

### Primary seam: the socket

One new integration binary beside the existing socket suites, using the same test
server and socket client. Each command is driven as a real
`orchestration.dispatchCommand` and asserted three ways:

1. the sequence it answers with,
2. the events that reach a subscriber on the project list and on the thread's own
   feed, including a subscriber on a _second_ connection,
3. what a **fresh** subscriber sees — which is what proves the state was persisted
   rather than merely broadcast.

Restart survival reuses the harness's ability to start a server against an
existing database. Refusals are asserted on the sentence, because the contract's
dispatch error carries a message and nothing else machine-readable, so the
sentence _is_ the diagnostic.

Prior art, to be followed closely rather than reinvented: the existing project
command suite is the same shape end to end — helpers that build a command payload,
a helper that opens the shell subscription, a helper that reads the list as a
fresh subscriber would, and a helper that extracts a refusal.

The turn, interrupt, permission and user-input suites are the prior art for the
cases that involve a live agent, which the settle and snooze invariants do.

### Second seam: command parsing

Payload validation is arithmetic on a string and is already unit-tested in place
for the seven commands that exist — blank identifiers, malformed answers. The new
commands extend that, covering blank identifiers, a wake time that is not in the
future, an unparseable wake time, a blank title, and a mode the contract does not
name.

This is a second seam rather than more socket tests because each of these is one
sentence about one payload; asserting them end to end would be a connection and a
dispatch per sentence, and the socket seam would grow to dozens of cases that
never reach the world.

### Third seam: the migration

The store already has the test that opening an already-migrated database applies
no migration twice. The appended migration wants its companion: an existing
database at the previous version gains the six columns, and rows written before it
read back as never archived, settled, snoozed or deleted.

### Not a seam

The client's classification is not retested here. It arrives as a dependency, has
its own suite upstream and in this repository's TypeScript packages, and a Rust
test asserting what the client concludes would be a fourth copy of a rule this
repository already keeps three of.

## Out of Scope

- **The other 27 unimplemented RPC methods.** The preview subsystem, the server
  admin and diagnostics cluster, worktrees and pull, the review diff preview, and
  the two remaining stream subscriptions. All are in the ledger with their own
  sizing; none is a dependency of this work.
- **Deleting `orchestration.replayEvents` from the contract**, and correcting the
  stale "26 of 71" figure in the three documents that carry it. Both are ledger
  items, both are trivial, and neither belongs in a feature branch about the
  thread lifecycle.
- **Proposed plans.** The thread read model hardcodes an empty plan list and a
  false pending-plan flag. Publishing a plan needs the plan-exit tool answered
  rather than merely reported, which is its own effort, and a flag nothing could
  act on would put a badge on a thread with nothing behind it.
- **Command-id idempotence.** A re-dispatched command is refused rather than
  answered with the sequence the first one committed at. This is safe — neither
  application can happen twice — and making a retry answer identically is work for
  a ticket that has a client which retries.
- **An event log.** Replay from an arbitrary cursor still needs one and this
  server still keeps none; a cursor is answered at its two ends, per ADR-0016.
  Nothing here changes that.
- **Auto-settle.** Upstream settles some threads without being asked, on a
  staleness rule tied to change requests. That rule reaches into source-control
  hosting, which this repository removed on purpose. Only the explicit commands
  and the three activity resets are in scope.
- **The provider-event vocabulary audit.** Whether the agent driver emits
  everything the contract's event types allow is a separate question the ledger
  records as unaudited.

## Further Notes

**ADRs this touches.** ADR-0008 (a turn is a photograph of the working tree) is
the model a revert restores. ADR-0016 (a cursor is answered at its two ends)
bounds what the new events have to support. ADR-0002 (threads is not split into a
log and a registry) is why the lifecycle fields belong on the thread rather than
in a second table. ADR-0012 (the UI is a fork we own) is why the client's
derivation is treated as a dependency rather than as something to reimplement.
The settling-versus-inbox-state collision is a candidate for a new one.

**Suggested build order**, cheapest real payoff first, each line shippable on its
own:

1. the two mode commands — columns exist, no migration
2. checkpoint revert — the photographs already exist
3. session stop — no schema change
4. the migration, then archive, unarchive, settle, unsettle, snooze, unsnooze and
   delete, plus the two rename commands and the archived shell snapshot, with the
   three activity resets

Items 1–3 are independent of the migration and can land first, which keeps the
schema change out of the way until the cheap wins are in.

**Where the parity evidence lives.** `.scratch/contract-parity/ledger.md` records
how the gap was measured and how to re-measure it, the upstream commits that
removed surface on purpose, and this ledger's own limits. Read it before
concluding that something not mentioned here is missing by accident.
