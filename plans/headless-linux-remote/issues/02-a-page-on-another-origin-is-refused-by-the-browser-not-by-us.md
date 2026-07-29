# 02 — A page on another origin is refused by the browser, not by us

**What to build:** the CORS headers and the preflight answer, so the desktop
application can add a remote environment and reach it.

**Status:** ready-for-agent

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
