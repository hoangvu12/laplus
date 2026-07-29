# 02 — A page on another origin is refused by the browser, not by us

**What to build:** the CORS headers and the preflight answer, so the desktop
application can add a remote environment and reach it.

**Status:** done — all five acceptance criteria met, the last by a drive. What
that drive uncovered is under "What the drive found" below and wants a ticket of
its own.

> `done` rather than the `ready-for-human` its four siblings wear:
> `server/docs/agents/triage-labels.md` reserves that label for work needing
> human implementation and says outright it is not for "finished, please review".
> The siblings predate this reading and are not relabelled here.

**Depends on:** nothing technically. In practice do 01, 03, 04 and 05 first —
this is the only ticket the phone does not need.

## Why

Adding a remote environment in Settings runs `preparePairingRegistration`
(`packages/client-runtime/src/connection/onboarding.ts`), which does three
things in order:

1. `resolveRemotePairingTarget(input)` — pure, no network
2. `fetchRemoteEnvironmentDescriptor` → `GET /.well-known/t3/environment`
3. `bootstrapRemoteBearerSession` → `POST /oauth/token`

The desktop window's page is served by its _own_ server on
`http://127.0.0.1:4773`. Every one of those calls goes to a different origin,
and this server answers none of them with an `Access-Control-Allow-Origin`. The
attempt dies at step 2 — the _first_ network call — with a CORS error, and the
user sees "could not reach the backend" for a server that answered fine.

Note what is **not** wrong: `environment_descriptor` is the one handler that
never calls `authorized`, so it would answer. The response simply never reaches
the page.

This is structural rather than an oversight to grep for. The router in
`crate::server` has **zero** `.layer()` calls — no middleware of any kind — and
`laplus-server`'s dependencies contain no HTTP middleware crate. The only
`Access-Control` string in the whole crate is the comment on the upgrade's 401
explaining why _that_ refusal does not carry one.

## Read the reference first

`pingdotgg/t3code:apps/server/src/httpCors.ts` is the entire upstream surface,
fourteen lines:

```ts
export const browserApiCorsAllowedMethods = ["GET", "POST", "OPTIONS"] as const;
export const browserApiCorsAllowedHeaders = [
  "authorization",
  "b3",
  "traceparent",
  "content-type",
  "dpop",
] as const;
export const browserApiCorsHeaders = {
  "access-control-allow-origin": "*",
  "access-control-allow-methods": browserApiCorsAllowedMethods.join(", "),
  "access-control-allow-headers": browserApiCorsAllowedHeaders.join(", "),
} as const;
```

Copy the header set exactly. `dpop` and the two tracing headers are listed even
though this server refuses DPoP and traces nothing: a preflight that omits a
header the client may send fails the request rather than degrading it, and the
client is built from the contract, not from what this server implements.

## Write it, do not add a dependency

The workspace manifest is explicit that a dependency has to earn its way in, and
this is fourteen lines of constant headers plus one `OPTIONS` handler.
`tower-http` is in `Cargo.lock` only via `reqwest`, which belongs to the shell's
updater plugin — it is not a direct dependency of `laplus-server` and should not
become one for this.

## What to build

1. **A `crate::http` function returning the header set**, so the spelling lives
   in one place. `crate::http` is already the module that is free of `axum` and
   holds this kind of policy.
2. **The headers on the routes a cross-origin browser calls**:
   `/.well-known/t3/environment`, `/api/auth/session`, `/oauth/token`,
   `/api/auth/browser-session`, `/api/auth/websocket-ticket`,
   `/api/orchestration/shell`, `/api/orchestration/threads/{threadId}`.
3. **An `OPTIONS` answer for each of them.** A JSON body and an `Authorization`
   header both force a preflight, so without this nothing above matters. The
   router currently registers no `OPTIONS` handler anywhere.
4. **Leave `/ws` alone.** A WebSocket handshake is not governed by CORS, the
   client reaches it with `?wsTicket=` precisely because a browser cannot set a
   header on one, and the existing comment argues correctly that echoing `*` on
   the upgrade's 401 would let any page read a refusal it provoked. That
   argument does not extend to the routes above, where the refused request _is_
   the one being helped.
5. **Do not send `Access-Control-Allow-Credentials`.** It is invalid with `*`,
   and the remote path is bearer-based end to end — `bootstrapRemoteBearerSession`
   returns an `access_token` the client stores, and the cookie is for the
   same-origin case only.

## What this does and does not widen

It widens nothing about who may do what. Origin is not part of the decision
anywhere in this server — `auth::authorize` never reads it, and
`UpgradeRequest.origin` is populated at the edge and consulted by nothing. A
credential is the boundary, before this ticket and after it.

What `*` does mean is that any page anywhere may _make_ these requests and read
the answers. Every one of them either needs a credential it will not have, or is
the descriptor, which is public by design so that a client holding nothing can
discover what it is talking to. Upstream ships `*` for the same reason.

## Acceptance criteria

- A test drives `GET /.well-known/t3/environment` and asserts the three headers
  are present on the response.
- A test drives `OPTIONS` against each listed route and asserts a success status
  with the three headers, without a credential.
- A test asserts `/ws` still has no `Access-Control-Allow-Origin` on its 401 —
  the existing decision, now pinned rather than described in a comment.
- `Access-Control-Allow-Credentials` appears nowhere.
- A real desktop laplus can add a remote environment pointed at a second laplus
  on another host and reach the point of pairing. This one is a drive, not a
  test — `server/tools/ui-driver/` is the harness.

## Out of scope

- `devOrigin`. Upstream's is for its own dev server; laplus's dev loop points
  Vite at this server and reaches it same-origin.
- Narrowing `*` to a configured list. That is an allowlist by another name, and
  the allowlist was removed on purpose.

## What landed

Fourteen lines of constants and a five-line wrapper, as the ticket said — with
one shape decision it did not anticipate, and one finding from the drive that
matters more than the code.

`http::browser_api_cors_headers` is upstream's three headers as `&'static str`
pairs, and `crate::http` stays free of `axum`: the names are turned into a
`HeaderName` at the one place that owns a response.

**`server::cross_origin` wraps a `MethodRouter`, and is the first `.layer()` in
this router.** The ticket asked for headers on seven routes plus an `OPTIONS`
handler for each; written literally that is fourteen edits across seven handlers
with some twenty return points between them — every refusal, every typed 404,
every `Err` out of `authorized`. A _refused_ cross-origin request that forgot its
headers is unreadable, which is the exact bug this ticket is about, so the
spelling that cannot forget won:

```rust
.route("/oauth/token", cross_origin(post(token_exchange)))
```

`cross_origin` adds `.options(preflight)` and `middleware::map_response`, both
`axum`'s own — **no dependency was added**, and `tower-http` is still in
`Cargo.lock` only through the shell's updater plugin. Applied route by route and
never to the whole `Router`, so `/ws`, the asset fallback and `/api/assets/…` are
untouched. The preflight answers `204`.

The three pairing-_management_ routes (`/api/auth/pairing-token`, and the two
`pairing-links`) deliberately do not wear it. They are Settings minting and
revoking codes for the backend it is sitting on, same-origin; a remote
environment's Settings is a question this ticket does not ask.

`tests/http_cors.rs` is four tests: the descriptor carries the three headers, all
seven routes answer a preflight with no credential, `/ws` still carries none on
its 401, and a 401 from a snapshot route is readable by the page — which is what
the layer buys over a line in each success path. The `/ws` one could not be made
to fail honestly, so it was checked by mutation: wrapping `/ws` in
`cross_origin` fails it, as it should.

**One existing test asserted the opposite, and had said so in advance.**
`http_orchestration.rs`'s `these_routes_check_the_credential_and_not_the_origin`
ended by pinning the _absence_ of `Access-Control-Allow-Origin`, under a comment
naming this ticket as the thing that would change it. It now asserts the header
and says why the second half is still defensible: a page anywhere may read this,
and what stands between it and the project list is the credential.

868 tests pass on Windows (`cargo test -p laplus-server --no-fail-fast`). The one
failure under load, `socket_diffs::a_turn_the_developer_stopped_is_not_offered…`,
is the known flake and passes in isolation.

### Driven, not only tested

Two real servers, a real Chrome, no Tauri needed: a different port is a different
origin, which is all a browser means by cross-origin. `tools/ui-driver/`
`remote-pairing.mjs` is the harness and is committed.

```
LOCALAPPDATA=…\lc-a laplus-server.exe --ui apps/web/dist --port 5773   # the page
LOCALAPPDATA=…\lc-b laplus-server.exe --ui apps/web/dist --port 5774   # the remote
node tools/ui-driver/remote-pairing.mjs http://127.0.0.1:5773/#token=… http://127.0.0.1:5774 <code>
```

It walks the chain `preparePairingRegistration` walks, from a page on 5773
against 5774, and then opens the socket. Every step 200, three preflights
answered 204, and the socket returned a `server.getConfig` `Success` — the whole
remote path, from a foreign origin.

**The control is what makes that mean anything.** Pointed at a laplus built
_before_ this change — the desktop application already running on 4773, whose
binary predates it — the same drive dies where the ticket said it would:

```
FAILED MissingAllowOriginHeader GET /.well-known/t3/environment
error: Access to fetch at 'http://127.0.0.1:4773/.well-known/t3/environment'
       from origin 'http://127.0.0.1:5773' has been blocked by CORS policy
```

**And then the real Add Environment form, which found the thing worth knowing.**
Driving `ConnectionsSettings.tsx` — Settings → Connections → Add environment →
Remote link — Chrome preflighted the _descriptor_ `GET`, which the fetch chain
above never did, and the preflight asked for exactly:

```
access-control-request-headers: b3, traceparent
```

The client's HTTP layer attaches both to every request, and neither is
CORS-safelisted. **So a header list written from what this server implements —
which traces nothing — would have failed the very first call even with
`Access-Control-Allow-Origin: *` present.** The ticket's instruction to copy
upstream's list verbatim, including two headers this server has no use for, is
not tidiness; it is the difference between working and not. That is now observed
rather than argued.

### What the drive found that this ticket does not fix

Past both calls — descriptor read, bearer minted, dialog closed on the success
path — **the environment does not appear under "Remote environments".** This is
not CORS: the wire log is four calls, all 200 or 204.

Every laplus server reports `environment_id: "local"` (`config.rs:404`, a
constant), and the client's registry is a `ReadonlyMap<EnvironmentId, …>`
(`packages/client-runtime/src/connection/registry.ts:65`) whose `local` key is
already the primary connection's. `savedEnvironments` in
`ConnectionsSettings.tsx:1474` filters out `PrimaryConnectionTarget`, so the
remote has nowhere to be listed. The registration succeeded and had no slot to
occupy.

**This wants a ticket and it is not this one.** The fix is not obviously a
constant swap: the id is what a stored connection profile is keyed by, so
changing it has to answer what happens to profiles already saved under `local`.
`ssh -L 5773:localhost:4773` plus this ticket remains the way to reach a remote
laplus from a desktop window today, which is what to tell anyone who asks.
