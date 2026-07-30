# 07 — Settling and unsettling

**What to build:** the developer can settle a finished conversation so it leaves
the inbox and stops competing for attention with work that needs them, and can
unsettle one to pin it back when they decide it is not finished after all.

**A naming warning, first.** `CONTEXT.md` already defines **settling** as reading
a session status as a turn state — a property of a _turn_, owned by its own
module, with the rule mirrored in three places in this repository. This ticket
uses the same English word for something different: whether a _thread_ belongs in
the developer's inbox. The existing meaning has seniority. Add the thread-level
concept to the glossary as **inbox state**, with a cross-reference warning at
**settling**, and keep the contract's field names as they are — renaming contract
vocabulary is not on the table, so it is the prose and the Rust identifiers that
have to disambiguate. This collision is the one candidate for a new ADR in the
whole effort.

**The server does not classify.** Which threads _count_ as settled is already
computed by the bundled client runtime, which ships unmodified. This ticket
stores the override, enforces the invariants, and emits the events. It does not
decide what the inbox shows.

**The invariants are not optional.** The client has its own copy of them, and
that copy exists explicitly so the interface can refuse before a round trip — the
server's are the authoritative ones. Anything the client refuses to _classify_ as
settled must also be refused as a settle _target_, which is why the list below
matches the client's rule exactly rather than approximately.

The unanswered-request guard is not new logic. The work log module already derives
unanswered approvals and unanswered questions, and is already the source of the
two pending flags on the shell summary. Deriving it rather than counting is what
makes it survive a restart.

**Repeats re-emit rather than refuse.** Settling something already settled lands
on the same state, and the re-emission carries the _existing_ updated-at rather
than the current time, so a double-click neither rewinds the thread nor moves it
in a list ordered by when things changed.

The two directions are not symmetrical. A **user** unsettle pins the thread
active. The neutral reset — returning an overridden thread to no override at all
— is server-decided and is ticket 08; the contract lets a client send only the
user reason, so the neutral one cannot be forged.

**Blocked by:** 01 — Lifecycle fields reach the client as stored state.
06 — Archiving and unarchiving (settle refuses an archived thread, and at the
socket seam the archive command is the only way to make one).

**Status:** done

- [x] Both commands are parsed before the world is consulted; blank identifiers
      are refused.
- [x] An unknown thread is refused.
- [x] Settling an archived thread is refused.
- [x] Settling a thread whose session is starting or running is refused.
- [x] Settling a thread with an unanswered approval is refused.
- [x] Settling a thread with an unanswered question is refused.
- [x] Settling a thread whose turn has been requested but not yet adopted by a
      session is refused, within the same adoption grace the client uses.
- [x] Unsettling an archived thread is refused.
- [x] Every refusal carries a sentence naming both what went wrong and the thread
      it went wrong about, because the dispatch error carries nothing else
      machine-readable and the interface renders the sentence verbatim.
- [x] A settle records both the override and the time it settled at.
- [x] A user unsettle pins the thread active rather than clearing the override to
      neutral.
- [x] Settling twice is harmless and does not change the thread's updated-at.
- [x] Unsettling twice is harmless and does not change the thread's updated-at.
- [x] Both changes publish on the thread's own feed and reach the project list.
- [x] A subscriber on a second connection sees both.
- [x] Inbox state survives a restart, and a fresh subscriber agrees with a
      subscriber that watched it happen.

## What it turned out to be

Two changes in the vocabulary (`Change::Settled` and `Change::Unsettled`), two
parse arms, `Shell::settle` and `Shell::unsettle`, and — the part that is not
boilerplate — `crate::threads::Busy` and `crate::threads::Adoption`, which are
`canSettle` and `QUEUED_TURN_START_GRACE_MS` mirrored to where they are
authoritative. No migration and no store change: ticket 01 added the two columns
and every change already writes the whole thread row.

`Busy` is an enum of four rather than a boolean, and that is what keeps the guard
and the sentence one rule apiece: `Thread::busy` answers _which_ blocker in the
client's own order, and `Shell::settle` turns each into a sentence. The order is
load-bearing — an agent that has asked for permission is also running, and
"waiting for your decision" is the more useful of the two things to say.

`Adoption` holds the grace window as **two rendered stamps** rather than a
number of milliseconds. Every timestamp on this wire is `crate::clock`'s
fixed-width UTC shape, so it orders lexicographically; drawing the window once
meant the comparisons need no calendar and this crate still has no date parser.
`clock::iso_from_epoch_millis` is the one line that was added for it.

## What was decided along the way

**A repeat re-emits, and that needed the one change in this crate that does not
stamp the clock.** `Threads::apply_unless` now asks `Change::re_emitted_at`
before folding: a settle over an already-settled conversation reports the
`updatedAt` and `settledAt` the conversation already carried, so a double-click
neither rewinds it nor moves it in a list ordered by when things changed. The
archive commands refuse a repeat instead, and the difference is real rather than
inconsistency — an archive is a move between two lists, and a settle is a
standing answer.

**The capability flag is part of this ticket, not a note beside it.**
`capabilities.threadSettlement` was absent, and `useThreadActions.ts` refuses to
dispatch either command to a server that does not advertise it while
`SidebarV2.tsx` and `ChatView.tsx` hide the menu items outright. Answering both
commands and leaving the flag off would have been two commands nothing sends.
`socket_conformance.rs` loses the declaration that said so.

**And flipping it switches on more than two menu items**, which was found while
reviewing this and is written down rather than left to be discovered:
`SidebarV2.tsx` reads the same flag before letting `effectiveSettled` classify a
thread at all, so the client's **inactivity auto-settle** starts working too.
That derivation is the client's and ships unmodified, so this is not the
server-side auto-settle the spec puts out of scope — but its premise, that the
server un-settles on real activity, is only half true until ticket 08. The
blockers hold regardless, so live work is never hidden; what can happen in the
meantime is an auto-settled conversation re-settling once a new turn finishes.
The alternative was shipping two commands with no control that sends them, so it
is taken deliberately and named in `CONTEXT.md` and on `Capabilities`.

**The unsettle reason is required rather than defaulted.** The contract types it
as a single literal, and the two reasons leave the conversation in different
states — `user` pins it active, `activity` returns it to neutral — so a payload
without one is malformed rather than guessed at, and one saying `activity` is
refused rather than quietly pinned. That is what makes the neutral reset
unforgeable, which is the property ticket 08 depends on.

**The collision got the ADR the spec named it a candidate for**:
`docs/adr/0024`, which records that the turn meaning keeps the word, the contract
field names do not move, and it is the prose and the Rust identifiers that
disambiguate. `CONTEXT.md`'s **Inbox state** section grew the commands, the
invariants and the re-emission rule.

## Not done

**The window has not been driven**, at the requester's instruction and as with
tickets 03, 05 and 06. Every criterion above is asserted through the socket in
`tests/socket_settling.rs` or at the two unit seams; the sidebar's context menu,
the chat view's menu and the settled section of the inbox are unchecked.

**The queued-turn guard is unit-tested rather than driven.** It is not reachable
through this server's own socket: `thread.turn.start` writes the message and then
the turn, so the turn's `requestedAt` is never older than the message, and the
session is marked `starting` in the same command. The guard exists because it is
the client's rule and the client folds shells this server did not write, so it is
tested in `threads::tests` beside the window it is measured against — both ends
of the grace, an adopted turn, and a failed session.

**Nothing wakes a settled conversation yet.** The three activity resets are
ticket 08's, and `Change::Unsettled` already carries the reason they need. One
trap is written onto `Change::re_emitted_at` for whoever builds them: an
`Unsettled { reason: "activity" }` over a conversation with no override lands
there as a repeat and would publish a no-op event at a stale `updatedAt`, so the
guard the spec asks for — "if any override is set" — belongs at the call site.
