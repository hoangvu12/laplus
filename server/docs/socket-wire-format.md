# The socket wire format, as captured

What the t3code UI actually sends and receives over its one WebSocket endpoint,
recorded from the reference TypeScript server rather than inferred from type
definitions. Every claim below is backed by a fixture in `fixtures/socket-wire/`.

The transport framing is laplus's primary risk: it is undocumented and comes
from `effect/unstable/rpc`, an explicitly _unstable_ module. This document and
its fixtures are what later work conforms to.

## How the captures were made

```
Chrome (unmodified UI)  ─┐
                         ├─► recording proxy :3999 ─► reference server :3773
scripted RPC client     ─┘        (tools/wire-capture/proxy.mjs)
```

1. `pnpm install` and `pnpm build:web` at the repository root.
2. Start the reference server against a throwaway data directory so nothing
   touches a real installation. This repository no longer carries a copy, so the
   step means checking out `github.com/pingdotgg/t3code` beside this one and
   installing its dependencies — its server is `apps/server/`:
   `T3CODE_HOME=$TEMP/laplus-wire-home node apps/server/src/bin.ts --port 3773 --host 127.0.0.1 --no-browser`
   It logs `Listening on http://127.0.0.1:3773` and a one-time pairing URL.
3. Start the recording proxy:
   `node tools/wire-capture/proxy.mjs --listen 3999 --upstream 127.0.0.1:3773 --out-dir .scratch/wire-capture/raw --label capture`
   It is a byte-transparent TCP proxy: it forwards everything untouched and
   writes one NDJSON file per connection.
4. Point the real UI at the proxy by opening the pairing URL on the proxy's
   origin: `http://127.0.0.1:3999/pair#token=<token>`. The server serves the
   built UI, so the page, its assets and its socket all route through the proxy.
   The UI connects and stays connected; nothing about it was modified.
5. Mint a bearer token for the scripted client —
   `node apps/server/src/bin.ts auth session issue --json` against the same
   `T3CODE_HOME` — then drive the deliberate cases with
   `tools/wire-capture/scripted-client.mjs --scenario <unary|error|stream|orchestration>`,
   which speaks the same socket through the same proxy. The `orchestration`
   scenario is the one that produces stream deltas and the back-pressure
   evidence.
6. Curate the raw recordings into fixtures with `tools/wire-capture/curate.mjs`
   (see _What is redacted_ below). The raw recordings stay in
   `.scratch/wire-capture/raw/`, which is gitignored because it holds live
   session tokens.

## The endpoint and the upgrade

One endpoint: `GET /ws`, a normal RFC 6455 WebSocket upgrade. Everything the UI
does goes through it; there is no REST surface behind it.

The client's upgrade request, verbatim from `01-browser-session.ndjson`:

```
GET /ws HTTP/1.1
Host: 127.0.0.1:3999
Connection: Upgrade
Upgrade: websocket
Origin: http://127.0.0.1:3999
Sec-WebSocket-Version: 13
Sec-WebSocket-Key: sXlZ+AnHRboR6K8AWi8sxw==
Sec-WebSocket-Extensions: permessage-deflate; client_max_window_bits
Cookie: t3_session=<signed token>
```

and the response:

```
HTTP/1.1 101 Switching Protocols
Upgrade: websocket
Connection: Upgrade
Sec-WebSocket-Accept: 4r6P2FwZUaGRAAhK1UpVFBYHVvM=
```

Two things to carry into the Rust server:

- **No subprotocol is negotiated.** The client sends no
  `Sec-WebSocket-Protocol` and the server returns none.
- **`permessage-deflate` is offered and declined.** The browser always offers
  it; the reference server answers with no `Sec-WebSocket-Extensions` header, so
  every frame on the wire is uncompressed. Declining it is the compatible
  behaviour and keeps captures readable.

### The credential at upgrade

The client presents a credential in one of two shapes, and the server accepts
either. Both are `base64url(claims).base64url(signature)` — two segments, not
three-segment JWTs.

| Shape                      | Where                   | Claims observed                               | Used by                                                                |
| -------------------------- | ----------------------- | --------------------------------------------- | ---------------------------------------------------------------------- |
| `t3_session` cookie        | `Cookie` request header | `v, kind, sid, sub, scopes, method, iat, exp` | the browser UI (`kind: "session"`, `method: "browser-session-cookie"`) |
| `wsTicket` query parameter | `GET /ws?wsTicket=…`    | `v, kind, sid, iat, exp`                      | non-browser clients (`kind: "websocket"`, ~5 min TTL)                  |

A third shape, an `Authorization: Bearer` header, is accepted by the reference
server's `authenticateWebSocketUpgrade` (read from
`t3code/apps/server/src/auth/EnvironmentAuth.ts`, not captured). The browser
never uses it — the ticket exists precisely because the browser WebSocket API
cannot set request headers.

With no credential the upgrade is refused before the socket opens
(`06-upgrade-rejected.ndjson`):

```
HTTP/1.1 401 Unauthorized
content-type: application/json
access-control-allow-origin: *

{"_tag":"EnvironmentAuthInvalidError","code":"auth_invalid","reason":"missing_credential","traceId":"…"}
```

v1 accepts these shapes without verifying them against any identity store, so
what matters here is the shape and the failure mode, not the signature.

## Frames

Every message is a **single unfragmented WebSocket text frame** holding one JSON
object. No fragmentation was observed at any size: every `ws-message` record in
every fixture has `frames: 1`. The two largest captured messages are a
79,972-byte `subscribeServerConfig` snapshot chunk and the 79,953-byte
`server.getConfig` response, and both arrived as one frame with `FIN` set. No
binary frames were observed. There is no length prefix, no delimiter and no
envelope of any kind above the WebSocket frame: the framing _is_ the WebSocket
framing.

Client frames are masked and server frames are not, per the RFC. The `ws-frame`
records in every fixture carry `fin`, `rsv`, `opcode`, `masked` and
`payloadLen`, so this is checkable rather than asserted.

## The message vocabulary

Six message tags were observed. All are discriminated by `_tag`.

### Client → server

**`Request`** — starts a call. Used identically for unary methods and for
streaming subscriptions; nothing in the envelope distinguishes them.

```json
{
  "_tag": "Request",
  "id": "0",
  "tag": "server.getConfig",
  "payload": {},
  "traceId": "1091713e6fd4a7ca567589e5537d499a",
  "spanId": "9f2023d48d079987",
  "sampled": true,
  "headers": []
}
```

- `id` is a **decimal string**, assigned by the client, starting at `"0"` and
  incrementing by one per connection. Unary calls and subscriptions share one
  id space.
- `tag` is the method name, e.g. `server.getConfig`,
  `orchestration.subscribeShell`, `projects.readFile`, `vcs.listRefs`.
- `headers` is an array of `[name, value]` pairs; the UI always sent `[]`.
- `traceId` / `spanId` / `sampled` are optional. The browser sends them; the
  scripted client omits them and the server is content.

**`Ack`** — `{"_tag":"Ack","requestId":"1"}`. Streaming back-pressure, not call
completion, and **genuine back-pressure rather than an advisory** — see _`Ack` is
load-bearing_ below. The UI sent exactly one `Ack` per `Chunk` received, for
every subscription.

**`Interrupt`** — `{"_tag":"Interrupt","requestId":"0"}`. Cancels an in-flight
call; this is how a subscription is unsubscribed.

**`Ping`** — `{"_tag":"Ping"}`. The UI sends one every ~5 s (measured gaps:
5008, 5012, 5005, 5016 ms).

### Server → client

**`Exit`** — the terminal response for a request id. Success:

```json
{"_tag":"Exit","requestId":"0","exit":{"_tag":"Success","value":{ … }}}
```

Failure, carrying a _cause array_:

```json
{"_tag":"Exit","requestId":"0","exit":{"_tag":"Failure","cause":[
  {"_tag":"Fail","error":{"_tag":"ProjectReadFileError","cwd":"…","failure":"operation_failed", … }}]}}
```

**`Chunk`** — one batch of stream values:
`{"_tag":"Chunk","requestId":"1","values":[ … ]}`. `values` is a non-empty array
and **does batch**: `subscribeServerLifecycle`'s single chunk carried two values
(`ready` then `welcome`) in one frame, while every other chunk captured carried
one. A conforming client must iterate `values` rather than assume one value per
chunk.

**`Defect`** — a connection-level failure not attributable to a declared error
type. An unknown method tag produces one, and note that it carries **no
`requestId`** — the caller's request is left without an `Exit`:

```json
{ "_tag": "Defect", "defect": "Unknown request tag: no.such.method" }
```

**`Pong`** — `{"_tag":"Pong"}`, in reply to `Ping`.

## Correlation

Requests and responses correlate by `requestId` alone, never by ordering. This
is not a theoretical concern — it is visible in `01-browser-session.ndjson`,
where the UI has ids `7, 8, 9, 10` in flight at once and the server answers
`7, 8, 10, 9`. A Rust server may complete concurrent calls in any order; a
client that assumed FIFO would already be broken against the reference server.

## Error tagging

Errors are not a separate message type. A failure is an `Exit` whose `exit._tag`
is `"Failure"` and whose `cause` is an **array** of entries, each one of:

| Entry                                 | Meaning                 | Observed                             |
| ------------------------------------- | ----------------------- | ------------------------------------ |
| `{"_tag":"Fail","error":{…}}`         | a declared, typed error | ✔ `03-typed-error.ndjson`            |
| `{"_tag":"Interrupt","fiberId":2494}` | the call was cancelled  | ✔ `04-streaming-subscription.ndjson` |
| `{"_tag":"Die","defect":…}`           | an undeclared defect    | not observed in an `Exit`            |

The typed error inside `Fail` is itself `_tag`-discriminated — here
`ProjectReadFileError` — and carries the contract's declared fields plus a
nested `cause` chain of plain `{name, message, cause}` objects down to the
originating `ENOENT`. The nested chain is descriptive; the `_tag` and the
declared fields are the contract.

## Streaming, from first chunk to termination

A subscription's whole life, verbatim from `04-streaming-subscription.ndjson`:

```
C>S {"_tag":"Request","id":"0","tag":"subscribeTerminalMetadata","payload":{},"headers":[]}
S>C {"_tag":"Chunk","requestId":"0","values":[{"type":"snapshot","terminals":[]}]}
C>S {"_tag":"Ack","requestId":"0"}
C>S {"_tag":"Interrupt","requestId":"0"}
S>C {"_tag":"Exit","requestId":"0","exit":{"_tag":"Failure","cause":[{"_tag":"Interrupt","fiberId":2494}]}}
```

The shape to reproduce:

1. A subscription is an ordinary `Request`. There is no `subscribe` verb.
2. Values arrive as `Chunk`s under the same `requestId`. The first chunk is
   typically a snapshot (`{"type":"snapshot",…}`, `{"kind":"snapshot",…}`,
   `{"_tag":"snapshot",…}` — the key varies by method) and later chunks are
   deltas.
3. The client `Ack`s each `Chunk`.
4. The stream ends with an `Exit` for the same `requestId`. **Client-initiated
   unsubscribe terminates as a `Failure` with an `Interrupt` cause, not as a
   `Success`** — a client must treat that as a normal end, not an error.

Some subscriptions accept a `requestCompletionMarker: true` flag and answer with
a `{"kind":"synchronized"}` chunk once catch-up is done
(`orchestration.subscribeShell`, `orchestration.subscribeThread`). That is a
payload-level convention, not part of the framing.

### `Ack` is load-bearing

The server sends **at most one un-acknowledged `Chunk` per request** and stops
until the client acknowledges it. `05-orchestration-and-backpressure.ndjson`
demonstrates this deliberately:

1. Subscribe to `orchestration.subscribeShell`; a `snapshot` chunk arrives and
   is acknowledged.
2. A `{"kind":"synchronized"}` chunk arrives and is **deliberately not**
   acknowledged.
3. `orchestration.dispatchCommand` creates a project. The server answers
   `Exit`/`Success` with `sequence: 5`, so the change is committed.
4. Two seconds pass with **zero further chunks** — the shell change is queued
   behind the missing `Ack`, not lost.
5. The `Ack` is sent, and the `{"kind":"project-upserted","sequence":5,…}` delta
   arrives immediately.

A Rust server that ignores `Ack` and pushes freely will not fail visibly against
the UI — the UI acknowledges everything — but it changes the memory profile of a
busy subscription from bounded to unbounded. A Rust _client_ that fails to `Ack`
will simply stop receiving after one chunk.

This fixture is also the only capture of a subscription emitting real deltas
after its snapshot, and the only capture of the orchestration surface the spec
calls the core.

## The connection handshake

There is none. The socket opens and the client's first frame is already a
`Request` — no hello, no version exchange, no capability negotiation at the
transport level.

Capability negotiation happens one level up, in the payload: the UI's first call
is `server.getConfig`, and it can do nothing until a well-formed response
arrives. That response is large (~80 KB) and carries the environment
descriptor, auth descriptor, `cwd`, keybindings, providers, available editors,
observability settings and server settings. Getting exactly that one method to
satisfy the unmodified UI is the thinnest slice that proves the transport.

The UI's opening sequence, in order:

```
server.getConfig                    → Exit/Success
subscribeServerLifecycle            → Chunk (ready, welcome)
subscribeServerConfig               → Chunk (snapshot)
orchestration.subscribeThread       → Chunk (synchronized)
orchestration.subscribeShell        → Chunk (synchronized)
subscribeTerminalMetadata           → Chunk (snapshot)
subscribeVcsStatus                  → Chunk (snapshot, then remoteUpdated)
projects.readFile / assets.createUrl / server.discoverSourceControl / vcs.listRefs
```

## The fixtures

| File                                       | What it holds                                                                                                                                                                                                                                                 |
| ------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `01-browser-session.ndjson`                | The unmodified UI's **boot sequence**: upgrade, `server.getConfig`, six subscriptions, four concurrent unary calls answered out of order, then the idle keepalive/poll loop. It is what the UI does on connect; it is not a record of a user driving the app. |
| `02-request-response.ndjson`               | A single successful `server.getConfig` request/response, plus `Ping`/`Pong`.                                                                                                                                                                                  |
| `03-typed-error.ndjson`                    | A `projects.readFile` that fails with a typed `ProjectReadFileError`, and an unknown method tag answered with `Defect`.                                                                                                                                       |
| `04-streaming-subscription.ndjson`         | The minimal subscription lifecycle: first chunk, `Ack`, client `Interrupt`, terminal `Exit`.                                                                                                                                                                  |
| `05-orchestration-and-backpressure.ndjson` | The orchestration surface driven end to end: shell subscription, snapshot, a withheld `Ack` stalling the stream across a committed change, then `project-upserted` and `project-removed` deltas and an `Interrupt`.                                           |
| `06-upgrade-rejected.ndjson`               | An upgrade attempt with no credential: `401` and its JSON body; the socket never opens.                                                                                                                                                                       |

Each line is one record, of these types:

| Record               | Carries                                                              |
| -------------------- | -------------------------------------------------------------------- |
| `connection-opened`  | the client's remote address                                          |
| `http-request`       | the upgrade request's `method`, `target`, `headers` and raw `head`   |
| `http-response`      | the response `statusLine`, `headers` and raw `head`                  |
| `http-response-body` | the body of a **refused** upgrade (a 101 has none)                   |
| `ws-frame`           | one WebSocket frame's `fin`, `rsv`, `opcode`, `masked`, `payloadLen` |
| `ws-message`         | the assembled payload as `text` — the bytes as they crossed the wire |
| `error`              | a transport failure, e.g. the client resetting the connection        |
| `connection-closed`  | which side ended it                                                  |

Every record also carries `seq` (order within the connection) and `tMs`
(milliseconds since the connection opened).

### What is redacted

Two things: the credentials presented at upgrade, and account email addresses.

**Upgrade credentials.** The `t3_session` cookie value and the `wsTicket` query
parameter are replaced by a marker naming the token's claim names and length, so
the shape the permissive local handshake must accept stays legible while the
signed value does not enter the repository.

**Account emails.** A provider's `auth.email` names a person, and this
repository is public, so every address is masked to `redacted-<digest>` at the
same domain — `curate.mjs`, `maskEmail`. This is the one redaction that reaches
_inside_ a frame, so `ws-message` text is no longer byte-for-byte what crossed
the wire. Two properties limit the damage, and both are tested:

- **The mask is the same width as the address it replaces**, so every
  `payloadLen` in these fixtures is still the frame's true byte count, and the
  sizes quoted above are still these files' sizes.
- **One account masks identically everywhere and two accounts stay two
  accounts**, because the mask is a digest of the address, so a fixture that
  distinguished two provider logins still does.

Addresses that already name nobody are left alone — anything at a `.invalid` or
`.example` domain, which includes the `…@laplus.invalid` git author the server
mints for its own checkpoint commits.

Nothing else is altered; `ws-frame` records pass through untouched.

Two exposures remain, deliberately. The `assets.createUrl` response in
`01-browser-session.ndjson` contains a signed asset grant: it is a server
response and therefore part of the protocol surface being documented, it expired
an hour after capture, and it grants nothing beyond a favicon path on the
machine that produced it. And local absolute paths (`C:\Users\ADMIN\…`) appear
throughout, as `cwd` values the UI genuinely sends — they name a directory
layout rather than a person, and they are load-bearing evidence of what the
client puts on the wire.

## Open questions

Observed but not understood, or not observed at all. Recorded rather than
guessed at.

1. **What bounds a `Chunk` batch?** Still unknown _of the reference server_:
   batches of 1 and 2 values were captured, and whether its maximum is time- or
   count-driven was never established. (It coalesces shell events over a 50 ms
   / 512-item window — `t3code/apps/server/src/ws.ts` — but no capture
   exercised the ceiling, so the observed maximum remains 2.)

   **laplus now has a policy, chosen in ticket 04:** a batch is whatever has
   accumulated behind the outstanding `Ack`, capped at 64 values — so it is
   count-driven, and the count is the same one that bounds the backlog. Those
   being the same number is what makes 64 the true maximum: a subscriber that
   falls further behind than that is sent one fresh snapshot instead of its
   backlog, so no chunk can ever carry more. See
   `crates/laplus-server/src/subscriptions.rs`.

2. **How deep is the `Ack` window?** That the reference server stops at one
   un-acked chunk is settled (see _`Ack` is load-bearing_). Whether one is its
   fixed window or simply all this workload produced was not established: no
   capture generated two changes while an `Ack` was outstanding.

   **laplus's window is exactly one**, which is the conservative reading —
   a client written against a window of one works against any deeper window,
   and the reverse is not true.

3. **`Eof`, `ClientEnd` and `ClientProtocolError` were never seen.** The
   `effect/unstable/rpc` vocabulary defines all three. Whether the UI can ever
   provoke them is unknown; the Rust server currently has no reason to emit
   them.
4. ~~**`Defect` carries no `requestId`.**~~ **Answered in ticket 03 — it tears
   the session down.** The client handles the frame as
   `clearEntries(Exit.die(message.defect))`
   (`effect/unstable/rpc/RpcClient.ts`, `effect@4.0.0-beta.78` in the vendored
   checkout), which fails _every_ in-flight request and _every_ open
   subscription on that socket with a die rather than a typed failure. The
   connection supervisor then reconnects on a 1/2/4/8/16-second backoff.

   An `Exit` is scoped by comparison: `decodeExit(...)` is wrapped in
   `matchCauseEffect`, so even an error payload that fails to decode is written
   back under the same `requestId` and nothing else is touched.

   This is why **laplus does not send `Defect`**, and it is the one place
   the Rust server deliberately does not follow a capture. The reference server
   can afford `Defect` because it implements every tag its client sends, so one
   only ever answers a tag no real client uses; laplus implements a fraction
   of the vocabulary while it is being built, so `Defect` would be the normal
   answer to the UI's own boot sequence. Unimplemented methods come back as
   `Exit`/`Failure` with a `Fail` cause carrying a typed error. See
   `crates/laplus-server/src/rpc.rs` and
   `crates/laplus-server/tests/socket_conformance.rs`, which pins both the
   captured behaviour and the divergence.

   **Which** typed error is a second question, and ticket 39 answered it
   differently from ticket 03. An `Exit` costs one request only while the client
   can decode the error inside it, and the client decodes each one against the
   union that method declares in `packages/contracts/src/rpc.ts`. A single
   `ServerMethodNotImplementedError` for every method is in no union at all, so
   `/settings/diagnostics` and `/settings/source-control` showed the schema
   decoder's complaint about the refusal instead of anything about the feature.
   A refusal now carries a tag the method it names declares —
   `EnvironmentAuthorizationError`, which every union in `rpc.ts` contains — with
   the same sentence about what was refused. `ServerMethodNotImplementedError`
   survives for a tag the contract does not name at all, which is what
   `no.such.method` still gets and what the conformance test pins.
   `crates/laplus-server/src/refusals.rs` holds the per-method table and the
   test that reads it back out of the contract; `docs/adr/0017` is the decision
   and its cost, of which the sharpest is that `session.ts` turns this tag on
   `server.getConfig` or `server.probe` into a refused connection rather than an
   empty page.

   Read from source, not captured — no recording provoked a `Defect` against
   the real UI.

5. **Does a subscription ever end in `Exit`/`Success`?** Every termination
   captured was client-initiated and came back as `Failure`/`Interrupt`. Natural
   completion of a server-side stream was never observed.

   Still open, and laplus has still not had to answer it. Ticket 17 was the
   first candidate — a terminal whose shell exits — and it turned out not to be
   one: an exited terminal is still a terminal, showing what it said and what it
   exited with, so `terminal.attach` stays open and ends the captured way when
   the client unsubscribes. Every stream laplus serves is of that shape, so
   the question is deferred rather than resolved.

6. **What happens to the user-driven surface?** `01-browser-session.ndjson` is
   the boot sequence only; no capture drives the UI through opening a file,
   starting a thread or running a turn. Nothing in the framing suggests those
   would differ — they are the same `Request`/`Exit`/`Chunk` vocabulary — but
   that is an inference, not a capture. Tickets 04 onward will exercise them.
7. **Is `defect` always a string?** The one capture shows
   `"defect":"Unknown request tag: …"`. The type is `unknown`, so it may be an
   arbitrary JSON value.
8. **Request-id reuse.** Ids were strictly monotonic within a connection and
   never reused. Whether the server would tolerate reuse after an `Exit` is
   untested.
9. **`Sec-WebSocket-Extensions`.** The reference server declined
   `permessage-deflate` in every capture. Whether it can ever accept it — and
   would therefore compress payloads — was not established. The recording proxy
   assumes uncompressed frames and would need extending if it can.

## Corroborating source

The captures are the contract; this is where to read when a capture surprises
you. `effect/unstable/rpc/RpcMessage.ts` in the vendored checkout
(`effect@4.0.0-beta.78`) defines the encoded message types — `RequestEncoded`,
`AckEncoded`, `InterruptEncoded`, `Eof`, `Ping`, `ResponseChunkEncoded`,
`ResponseExitEncoded`, `ExitEncoded`, `Pong`, `ClientProtocolError` — and every
tag observed above matches it. The module is marked unstable, which is exactly
why the fixtures rather than the types are the thing to conform to.

Server side: `t3code/apps/server/src/ws.ts` mounts `GET /ws` with
`RpcServer.toHttpEffectWebsocket(WsRpcGroup)` over `RpcSerialization.layerJson`.
Client side: `t3code/packages/client-runtime/src/rpc/session.ts` builds
`RpcClient.makeProtocolSocket` over the same `RpcSerialization.layerJson`.
