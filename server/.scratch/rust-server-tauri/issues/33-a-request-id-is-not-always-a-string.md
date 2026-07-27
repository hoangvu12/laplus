# 33 — A request id is not always a string, and this server drops the ones that are not

**What to build:** a socket that answers a client whose request ids are numbers.

**Status:** done

**Found by:** ticket 32, the first time the shell was built against `laplus` at
its own HEAD rather than the commit the vendored checkout was pinned to.

## What happens

The UI boots, opens the socket, sends `server.getConfig` — and is never
answered. It sits with no configuration at all: no projects, no environment, no
provider. `Ping`/`Pong` keep working, because those carry no id.

```
→ {"_tag":"Request","id":0,"tag":"server.getConfig","payload":{}}
→ {"_tag":"Ping"}
← {"_tag":"Pong"}
```

`id` is `0`, a JSON number. Every capture in `fixtures/socket-wire/` has `"0"`,
a string, and [`crate::wire::ClientMessage`] declares `id: String`, so the frame
fails to deserialize and is counted as malformed rather than answered.

## Why it is ours rather than upstream's

`effect` moved from `4.0.0-beta.78` to `4.0.0-beta.102` between the pinned
commit and laplus's HEAD, and its RPC client now emits numeric ids. But the
protocol has always permitted both — `effect/unstable/rpc/RpcMessage.d.ts`:

```ts
export type RequestId = Branded<string | number, "~effect/rpc/RpcMessage/RequestId">
```

So this is not a breaking change we have to absorb. It is a **latent bug**: the
server required one of two legal encodings, and the captures happened to show
the one it required. Any current Effect client hits it, upgraded UI or not.

Everything else in the envelope is unchanged — `Request`, `Ack`, `Interrupt`,
`Chunk`, `Exit`, `Defect`, `Ping`/`Pong`, and the `Success`/`Failure` causes all
match what this server already speaks. This is one field.

## What "fixed" means

Accepting a number is half of it. The other half is **echoing back the
representation that arrived**: the client keys its in-flight requests by the id
it sent, so a reply of `"0"` to a request of `0` is a reply nothing is waiting
for — the same silence, one step later.

Two ids that differ only in representation are the *same id* for correlation.
A client will not mix them, but if one did, answering the request it meant is
better than answering neither.

## Acceptance

- A `Request` with a numeric id is answered, and its `Exit` carries a numeric
  `requestId`.
- A `Request` with a string id is answered exactly as before — every fixture in
  `fixtures/socket-wire/` still conforms.
- `Ack` and `Interrupt` correlate to a subscription opened with either form.
- The real UI, driven headless, gets its configuration and renders the sidebar.

## Comments

### 2026-07-28 — agent. Done

`RequestId` in `crate::wire`, parsing either shape and keeping the one it
parsed. Equality and hashing go through a single canonical spelling, so the
registry of open subscriptions agrees with itself whichever way an `Ack` names
one; serialization does not, so a reply is addressed the way the request was.

Small, in the end: five call sites outside `wire.rs` — `Subscriptions::{start,
acknowledge,interrupt}`, its map, and `Server::defer`. Every existing test and
every fixture passed untouched, which is the point worth keeping: string ids
were never wrong, only insufficient.

Two tests at the socket rather than at the type, because the type was not what
failed here — the server was answering a frame it had already thrown away. One
sends a numeric `Request` and requires a numeric `requestId` back; the other
opens a subscription with a numeric id, feeds it an `Ack` and cancels it.

Then the real UI: it boots, gets its configuration, and renders the project,
its threads and the composer. Which is the whole bug, seen from the only place
it was ever visible.
