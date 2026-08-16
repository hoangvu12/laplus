Status: ready-for-human

# 05 — Bound requests to an OpenCode that stopped answering

Shipped in v0.1.7. Recorded here because it was found while scoping the stream
supervisor and it is _not_ that: no stream is lost in this failure, and the
supervisor would not have caught it.

## What it was

`OpenCodeClient` was built with `reqwest::Client::new()`, which has no request
timeout. Two awaits inherited that:

- `Driver::stop` asked OpenCode to abort **before** `owned.stop()`, the only
  call that kills the process tree. An OpenCode that had stopped answering its
  own port was therefore never reaped. Observed on one machine as 64 orphaned
  `opencode serve` processes holding 5.35 GB, accumulated over three days while
  laplus kept running.
- The session loop awaits `send` and `interrupt` _before_ its `select!`, so an
  unanswered request stopped it reading its own signals. No event normalized,
  Stop doing nothing, and the conversation showing Working until the process
  died.

The trigger is ordinary rather than exotic: an OpenAI-compatible proxy stalls
mid-stream, and OpenCode has no default `chunkTimeout` on that path
(opencode#37580, open). The socket stays open and the request is read, so
nothing below the transport ever errors.

## What shipped

`REQUEST_TIMEOUT` (30s) applied **per request**, not on the client —
`subscribe` is a stream that is meant to stay open. `ABORT_TIMEOUT` (5s) around
both aborts. `context: 0` from a model declared without a `limit` block now
reads as absence rather than a window of nought.

## Evidence

`an_unanswered_abort_is_reported_and_leaves_the_session_loop_reading` in
`tests/socket_opencode_turn.rs`, against a peer that accepts the abort and never
answers. Checked against the code without the fix rather than assumed:

| State                | Result                             |
| -------------------- | ---------------------------------- |
| both bounds          | passes                             |
| `ABORT_TIMEOUT` gone | still passes                       |
| both gone            | hangs; the harness kills it at 60s |

So `REQUEST_TIMEOUT` is the load-bearing bound. `ABORT_TIMEOUT` turns a 30-second
dead Stop button into a five-second one, which is worth having and is not what
breaks the deadlock.

## Why this is not closed

Not driven in a running laplus. The interrupt path needs a provider that
answers, and the one that reproduced this was returning HTTP 402 — driving it
would have tested a credit balance rather than the change. Confirmation that
Stop now stops is owed by a real session.

Two things this does **not** fix, both still open:

- A failed interrupt leaves `driving.turn.stopped` unset, so the reconciliation
  deadline is never armed and the turn keeps reporting as running. The developer
  gets a `turn.interrupt-failed` row and a loop that reads again, which is an
  improvement rather than an answer.
- Stream loss, which is what the rest of this effort is about.
