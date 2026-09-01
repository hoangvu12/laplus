# ADR-0056 — OpenCode turns reconcile after stream loss

Date: 2026-08-13
Status: Accepted; the supervision sentence amended on 2026-09-01

> **Amended 2026-09-01.** "Inspection failures ... remain supervised instead of
> ending the conversation" reads as supervision with no end, and supervision
> with no terminal condition is the wedge it was written to prevent: against a
> session history that answers only errors, the stopped turn never settles, and
> a conversation that cannot settle cannot accept the next turn either. The
> sentence below is kept as written because the half of it that is right is the
> half this amendment narrows — an inspection failure still may not end the
> conversation. What it may now do is end the _turn_: a history that stays
> unreadable for a bounded window settles the stopped turn as interrupted while
> the failure it already reported stands. That rung, and what releasing a queued
> prompt on it costs, belong to
> [ADR-0059](0059-a-stop-that-cannot-be-proved-still-ends-the-turn.md), which
> supersedes ADR-0058 in part.

An interrupted OpenCode event stream does not by itself complete, fail, or
repeat a turn. Laplus visibly enters turn recovery, asks OpenCode for the
session's current state and messages, merges only missing output, and
resubscribes while the provider remains busy; recovery continues until the
provider settles or the developer stops it. Missing sessions and terminal
authentication or protocol errors fail visibly while preserving partial work,
and no recovery path resends the developer's prompt. This favors durable,
idempotent recovery over a time limit that could misclassify healthy long work.

An interrupt uses the same recovery machinery but does not trust session
status as proof of completion. Laplus samples authoritative assistant message
output across a bounded quiet window; every changed snapshot restarts that
window, and only one unbroken quiet interval proves settlement. While that
proof is pending the turn remains running with a visible stopping activity, so
queued work cannot enter the stopping turn. Inspection failures and a
still-changing external server remain supervised instead of ending the
conversation; owned-server escalation is ADR-0058.
