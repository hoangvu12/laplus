# 31 — Every snapshot is asked for over HTTP first, and refused

**What to build:** two HTTP endpoints the UI already asks for, so that loading a
conversation stops costing a failed round-trip before it starts.

The client loads the shell snapshot and each thread snapshot over HTTP, and
falls back to the socket-embedded snapshot when that fails. This server
implements neither route, so **every** snapshot takes the fallback, and each one
logs a 404 and a warning on the way:

```
error: Failed to load resource: the server responded with a status of 404 (Not Found)
log:   WARN Could not load the environment shell snapshot over HTTP; using the socket snapshot instead.
error: Failed to load resource: the server responded with a status of 404 (Not Found)
log:   WARN Could not load the thread snapshot over HTTP; using the socket snapshot instead.
```

**Status:** needs-triage

**Found by:** ticket 03, in `tools/ui-driver/probe-boot.mjs` output while closing
its last two criteria. Invisible in the window — everything renders — and
invisible on the socket, because the failure happens over HTTP.

## The two routes

From the vendored client, which is the only specification either has:

| Route | Loader |
|---|---|
| `GET /api/orchestration/shell` | `packages/client-runtime/src/state/shellSnapshotHttp.ts` |
| `GET /api/orchestration/threads/{threadId}` | `packages/client-runtime/src/state/threadSnapshotHttp.ts` |

Both go through `makeEnvironmentHttpApiClient` with
`buildEnvironmentAuthHeaders`, and the thread loader times out at 6 s
(`DEFAULT_THREAD_SNAPSHOT_TIMEOUT_MS`). The client's own comment gives the
motive:

> The response is gzip-compressible by the transport and keeps the (potentially
> multi-KB) snapshot off the socket.

So this is a **transport optimisation**, not a capability. The socket path
already carries the same data and already works.

## Why it is worth doing anyway

Two things make it more than a warning in a console nobody reads.

**It compounds with the draft poll.** A "New thread" pane subscribes to a thread
that does not exist yet, and the client is given
`retryExpectedFailureAfter: "250 millis"` for that subscription — correct, and
ticket 03 explains it. But each retry re-attempts the HTTP snapshot too, so an
open draft pane produces a sustained ~4/second 404 storm for as long as it is
open. Neither half is a defect on its own; together they are a server answering
four pointless requests a second all day.

**It is noise in the one place a UI bug shows up.** `tools/ui-driver` exists
because the client's console and frame log are the only way to see the UI half of
this application. Ticket 28 was found by reading exactly that output. A console
that is already full of 404s at four a second is a worse instrument, and this is
the repo's only instrument.

## Worth settling before starting

**Whether to implement them or to make the client stop asking.** Not obvious:

1. **Serve both routes.** They return data the server already assembles for the
   socket, so the work is a route, an auth header check and a serialisation —
   not new domain logic. Gets the gzip benefit the client wanted.
2. **Find the flag.** If some capability or config field turns the HTTP fast-path
   off, silencing it costs one field instead of two endpoints. Ticket 03 already
   found that `connectionProbe`'s absence changes client behaviour this way, so a
   switch is plausible — but nobody has looked, and if there isn't one, option 1
   is the answer by default.

Check 2 first; it is a grep, and it decides whether this ticket is small or
medium.

## Not to be confused with

`src/http.rs` currently serves `/api/auth/session` and
`/.well-known/t3/environment` only, both written from the contract with no
capture behind them — ticket 03 calls it the least well-founded code in that
ticket. These two routes are the first *evidence* of what else the UI wants over
HTTP, which makes them worth capturing properly rather than guessing at: the
reference server is the authority on the response shape, and
`tools/wire-capture/` records only `/ws`.
