# ADR-0016 — A cursor is answered at its two ends, because the middle needs a log

Date: 2026-07-28
Status: Accepted

## Context

Ticket 31 gave the UI the two HTTP snapshot routes it had been asking for since
it was first pointed at this server. It worked, and it made the payload travel
**twice**.

The mechanism is short. Holding an HTTP snapshot is what makes the client send a
cursor: `makeSubscribeInput` in `packages/client-runtime/src/state/shell.ts`
returns `{ afterSequence: httpSnapshot.value.snapshotSequence }` on a successful
load and nothing at all otherwise, and `state/threads.ts` has the same shape via
`canResume`. So before ticket 31 there was never a cursor on this wire; after it,
there is one on every subscription. This server ignored it and sent the snapshot
anyway. The client's own comment gives the motive the routes were built for —
the response "keeps the (potentially multi-KB) snapshot off the socket" — and
after ticket 31 it did not: it put a second copy on it.

`crate::orchestration`'s declared divergence read "`afterSequence` is honoured by
over-answering", which was generous even as written. `Shell::subscribe` did not
read the field at all, and `Threads::subscribe` read only its presence, for
ticket 28's refusal rule.

### What the reference server does

Readable in this repository — `apps/server` is not in the working tree under our
`sparse-checkout`, but it is in the object store: `git show HEAD:apps/server/…`.

`apps/server/src/ws.ts` answers a shell cursor by arithmetic:

```
replayGap = latestSequence - afterSequence
replayGap < 0 || replayGap > SHELL_RESUME_MAX_GAP  →  a fresh snapshot
otherwise                                          →  readEvents(after, gap)
```

`SHELL_RESUME_MAX_GAP` is 1,000, and the comment gives both reasons: past that,
"replaying every intervening event is far more expensive than a single snapshot",
and separately "a cursor ahead of this engine's authoritative state is also
invalid, so reset it with a snapshot". The thread subscription has neither guard
— with a cursor it replays and never asks whether the thread exists, which is
where ticket 28's refusal rule came from.

Two things follow that decided this.

**First, laplus already implements upstream's reset branch.** Sending the whole
snapshot to a cursored request is exactly what `replayGap < 0 || > 1000` does.
What was missing was never fidelity; it was the _other_ branch.

**Second, the other branch is `readEvents`, and it is not a feature — it is an
architecture.** `OrchestrationEngine.readEvents` is `eventStore.readFromSequence`
(`apps/server/src/orchestration/Layers/OrchestrationEngine.ts`). Upstream is
event-sourced: the log is the source of truth and every snapshot is a projection
of it, so replay costs nothing extra. Here the source of truth is SQLite rows and
in-memory state, and events are _announcements_ — `crate::store::Sequences` hands
out numbers, subscribers are broadcast to, and nothing retains what was said. A
replay log in laplus would be a second representation of every mutation, kept in
lockstep with the mutation itself, for a saving measured on a loopback socket.

**Third — and it is the one place laplus needs a guard upstream does not** —
upstream's sequences are monotonic forever, and this server's are not.
`Sequences` is seeded from the registry's stored number, which is the high-water
of _durable_ writes only; the counter lives in memory because a persisted one
would mean an `fsync` per streamed token. So a run reissues every number the run
before it did not write down, and a client cursor can legitimately sit _ahead_ of
this server's. `packages/client-runtime/src/state/threads.ts` seeds exactly such
a cursor from its persisted cache.

## Decision

**A cursor is answered at its two ends. Equal to the newest number this server
has handed out, the subscription opens with no snapshot. Anything else is
answered with the whole snapshot, as before.**

`crate::store::Sequences::caught_up` is the whole rule, and both subscriptions
call it. There is no replay log, no change to `packages/client-runtime`, and no
new state on a subscription.

Spelled out, because each part reads like an omission on its own:

- **Equality, not "at least".** A cursor behind cannot be replayed from; a cursor
  ahead is a previous run's number, not an early client. Both want the answer
  that replaces what the client holds, and it is the answer this server already
  gave every cursor.
- **A caught-up subscription may send nothing at all.** laplus advertises neither
  `shellResumeCompletionMarker` nor `threadResumeCompletionMarker`, so the real
  client sends a bare cursor and is owed no marker — and the pump sends no chunk
  for an empty description. The client is already written for this: `shell.ts`
  goes `live` off the HTTP snapshot, and `threads.ts` sets itself `live` on
  `!supportsCompletionMarker && canResume` without waiting for the socket. It is
  also what `fixtures/socket-wire/01-browser-session.ndjson` captures the
  reference server doing, at requests `3` and `4`.
- **The rule is re-read, not remembered.** `EventSource::describe` runs again
  whenever a subscriber falls a whole backlog behind, and that description must
  be a snapshot. Nothing tracks which call it is, because every event on every
  feed carries a number taken from `Sequences` — so falling behind is _itself_
  what makes a cursor stale. `a_subscription_that_opened_caught_up_is_re_described_once_it_has_not`
  is that invariant, and it is what would fail if an event were ever published
  without taking a number.
- **Ticket 28 is untouched.** The refusal still turns on the cursor's presence.
  A draft pane's HTTP snapshot 404s, so `canResume` stays false, so no cursor is
  sent, so the subscription is still refused and the client still polls.
- **A malformed cursor is no longer a resume.** `rpc::resume_cursor` takes a
  non-negative integer and treats anything else as absent, where before any
  non-null value counted. The contract says `NonNegativeInt`, so no conforming
  client is affected — and a client that sent nonsense used to be given an opened
  stream with nothing to fold events into, which is ticket 28's failure exactly.
  Both subscriptions read the field through that one function, because two
  readers would let them disagree about what a cursor is before either got as
  far as comparing it. `only_a_non_negative_integer_is_a_cursor` pins it.

## Consequences

- **The motive ticket 31 was built for is now true**, and pinned on the wire
  rather than argued: `a_shell_snapshot_read_over_http_does_not_travel_again_on_the_socket`
  asserts silence after a real client's payload, and its thread twin asserts the
  opening carries the marker and no conversation.
- **The saving is bounded and small, and that is the honest reading.** laplus
  binds to loopback and the shell runs the server inside the window's own
  process, so this is two JSON serialisations rather than a network. The reason
  to do it is that the client asked a documented question and was being answered
  with something else; `server.getConfig` travels twice on this same wire at
  80 KB and nobody has minded.
- **Whoever wants real replay is not blocked, and should read this first.** The
  cheap end was taken precisely because it does not prejudge the log. What it
  does is remove the only case anyone has actually observed, which is the case
  where the log would have replayed nothing.
- **`SHELL_RESUME_MAX_GAP` has no equivalent here and should not grow one.**
  Upstream needs a ceiling because it _can_ replay and sometimes should not. This
  server's threshold is zero, so the guard is the comparison.
