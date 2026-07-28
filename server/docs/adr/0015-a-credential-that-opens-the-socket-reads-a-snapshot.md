# ADR-0015 — A credential that opens the socket reads a snapshot, and nothing less will do

Date: 2026-07-28
Status: Superseded by [ADR-0019](0019-a-tunnel-dissolves-the-loopback-boundary.md)

> **What survives.** That these routes are exactly as strong as the socket, by
> construction rather than coincidence, is still the rule — they still call one
> function. What 0019 reverses is the posture that function _held_: an absent
> credential is now refused. A tunnel makes a request from anywhere look like a
> request from this machine, so "reachability is the boundary" stopped being
> true rather than stopped being convenient.

## Context

Ticket 31 added two HTTP routes the UI has been asking for since it was first
pointed at this server: `GET /api/orchestration/shell` and
`GET /api/orchestration/threads/{threadId}`. They answer with payloads the
socket already carries, and the client falls back to the socket when the fetch
fails — so the routes are a transport optimisation, not a capability.

That shape is what makes their authentication a decision rather than a detail.
The contract puts both endpoints behind `EnvironmentAuthenticatedAuth` and
declares `EnvironmentAuthInvalidError` — `missing_credential` /
`invalid_credential`, status 401 — for them, and ticket 31's acceptance criteria
copied that across: "a request with a missing or invalid credential returns 401
carrying the `auth_invalid` body."

**This server cannot honour that as written, and honouring it would have undone
the ticket.** Two facts, in the order that matters:

1. `crate::auth::authorize` accepts an **absent** credential, and always has.
   v1 has no identity store, so reachability is the boundary: the listener binds
   to loopback, non-local origins are refused, and every credential shape —
   including none — is recorded rather than verified.
2. The real client sends no credential here.
   `buildEnvironmentAuthHeaders` in `packages/client-runtime` returns `{}` when
   the connection carries no bearer or DPoP token, which is every primary local
   connection, i.e. every laplus window. Upstream's answer for that case is a
   session cookie, and this server issues none — `/api/auth/browser-session` is
   an unimplemented 404, and `/api/auth/session` reports `authenticated: true`
   without setting anything.

So a route that required a credential would 401 every request the shipped UI
makes. The client would log a warning and fall back to the socket, which is
exactly the failed round trip ticket 31 exists to remove — the feature would
have been implemented and inert, and its own tests would have passed if they had
been written with a credential in hand.

The same ticket says so itself, one bullet above the criterion: the routes
"should accept exactly what [`authorize`] accepts; a credential good enough to
open the socket must be good enough to read a snapshot, or the fallback becomes
the only working path again." The two criteria contradict each other, and only
one of them can be about this server.

## Decision

**The two snapshot routes authenticate through `crate::auth::authorize`,
unchanged and shared with the socket upgrade. Whatever opens a socket reads a
snapshot; nothing else does.**

Consequences of that, spelled out because each one reads like an omission on its
own:

- **A request with no credential is answered.** It is the case the shipped UI is
  in, not an edge one.
- **The one refusal is a non-local `Origin`,** and it matters more here than at
  the upgrade. Binding to loopback does not stop a page on another origin asking
  the user's own browser to fetch their project list, and a `fetch` is a much
  easier thing for such a page to make than a socket upgrade.
- **That refusal carries `EnvironmentAuthInvalidError` with
  `reason: "invalid_credential"`,** the same body `crate::auth::Rejection`
  renders for the upgrade — the closed union has no member for "wrong origin",
  and inventing one would produce a body the unmodified client cannot decode.
  The real cause stays in `Rejection::detail`, server-side.
- **The 401 does _not_ carry `Access-Control-Allow-Origin: *`,** which is the one
  place these routes deliberately differ from the upgrade. There the header lets
  a browser read the body rather than reporting a CORS error for a handshake it
  cannot see into. Here the request being refused _is_ the cross-origin one, and
  the header would be the only thing the refusal gave away.

## Consequences

- **The routes are exactly as strong as the socket and no stronger, by
  construction rather than by coincidence.** They call the same function.
  `every_credential_that_opens_the_socket_reads_a_snapshot` in
  `tests/http_orchestration.rs` drives both halves with one `ClientIdentity`, so
  a change that tightened one and not the other fails a test rather than
  silently re-routing the client back to the fallback.
- **This is not a new posture, and it must not become one by accretion.** It is
  the posture recorded on `crate::auth` applied to a second caller. The bound is
  still loopback plus the origin check, and it is still the whole bound. Adding
  a _write_ over HTTP would not inherit this decision — `dispatch` is in the
  contract, is deliberately not implemented, and would be a capability rather
  than a transport choice for the same data.
- **When an identity store arrives, this is one of the places it lands.** The
  socket upgrade and these two routes have to move together, and the shared
  `authorize` is what makes that a single edit rather than a search.
- **The ticket's criterion stays unmet on its own terms**, recorded here rather
  than quietly ticked. Anyone reading ticket 31's checklist should read this
  file before deciding it was missed.
