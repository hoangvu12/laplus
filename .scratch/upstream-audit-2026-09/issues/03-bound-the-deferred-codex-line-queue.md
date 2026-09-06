# 03 — Bound the deferred Codex line queue

Status: needs-info

**What to build:** a bound on `AppServer::deferred`, or evidence that it does not
need one. Measure before changing anything.

## Why this is not simply "port upstream's fix"

Upstream's [`#8187`](https://github.com/pingdotgg/t3code/pull/8187) bounds its
Codex and ACP readers to a **sliding** queue of 32, caps concurrent request
handlers at 32, and stops termination blocking on handler cleanup. Checked
against this tree, two of those three do not describe it:

- `codex.rs` reads into `async_mpsc::channel(OUTPUT_QUEUE)` with
  `OUTPUT_QUEUE = 256`. A `tokio::sync::mpsc` channel is **bounded and applies
  backpressure**: the reader task awaits `send`, so nothing is dropped and
  nothing grows. Upstream's was unbounded before `#8187`; laplus's never was.
- Every inbound app-server request is answered `-32601` by
  `protocol::unsupported_request`. There is no handler pool, so there is nothing
  to cap and nothing for termination to wait on.

## What is actually unbounded

`AppServer::deferred`, a `VecDeque<String>`. `request()` drains the bounded
channel looking for its own response and pushes everything else — every
notification, every request, every response to a different id — onto `deferred`,
which the session loop drains later through `next_line()`.

So its size is bounded by _how long a request is in flight_, not by a number:
`RESPONSE_TIMEOUT` is 10s and `STARTUP_RESPONSE_TIMEOUT` is 30s. Under a Codex
that streams hard through a long `turn/start` or a slow handshake, that is
however many lines Codex can emit in that window, each one an owned `String`.

## Why it was not just capped

The obvious fix — a sliding queue, as upstream chose — **drops protocol
notifications**, and in this codec a dropped notification is a lost message, tool
call or turn terminal in somebody's conversation. Upstream took that trade on the
same structure; it is a worse trade here, where the queue is the only path those
events have. Failing the in-flight request on overflow instead turns a memory
problem into a visible error, which is a behaviour change that wants evidence
behind it.

## What to measure first

1. Peak `deferred` length across a real Codex turn that streams heavily, and
   across the startup handshake, which issues `initialize`, `account/read`,
   `model/list` and `skills/list` back to back.
2. Whether any path holds a request in flight for materially longer than its
   timeout.
3. Bytes, not just lines: these are whole JSON-RPC lines and some carry a turn's
   accumulated items.

`server/fixtures/codex-app-server/` has the recordings to build a burst from, and
`tests/socket_codex_turn.rs` is where a case would go.

Only then decide between: leave it, cap it and fail the request, or cap it and
drop with a counter. Do not ship the third without an argument for why a lost
conversation event is acceptable.
