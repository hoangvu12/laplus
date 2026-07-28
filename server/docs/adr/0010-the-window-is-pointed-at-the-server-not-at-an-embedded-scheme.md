# ADR-0010 — The window is pointed at the server, not at an embedded scheme

Date: 2026-07-27
Status: Accepted

## Context

Ticket 23 puts lightcode in a desktop window. The window has to get the UI from
somewhere, and Tauri's own answer is the obvious one: declare `frontendDist`,
let `generate_context!` embed the bundle, and let the webview load it from
Tauri's asset scheme. That is what the size spike did, and what every Tauri
tutorial does.

On Windows that scheme is **`http://tauri.localhost`**, and the server is on
**`http://127.0.0.1:4773`**. Two origins. Three things break at once, and each
of them is worse than it first looks:

- **The socket upgrade is refused.** `crate::auth` accepts loopback origins
  only, and `tauri.localhost` is not one — it is an ordinary hostname that
  happens to end in `.localhost`. A test pinned this before the ticket started
  (`the_origin_check_matches_on_host_and_ignores_the_scheme`), with a note that
  ticket 23 might widen it. Widening it would have widened it for a real browser
  too, on a server whose entire security model is "reachability is the
  boundary".
- **The UI's boot fetches would miss.** `/.well-known/t3/environment` and
  `/api/auth/session` are requested _relative_, so they would go to the scheme
  handler and 404. The only fix is to rebuild upstream's bundle with
  `VITE_HTTP_URL` and `VITE_WS_URL` baked in — a Node build of vendored code
  this project has pinned as reference material and does not own.
- **`localStorage` would belong to the scheme.** The UI keeps the developer's
  layout, drafts, sidebar state and last-open thread there, and browsers scope
  it per origin.

## Decision

**The server serves the UI, and the window is pointed at
`http://127.0.0.1:<port>/` like any other page.**

`crate::ui` holds the policy — what a path resolves to, what content type it
gets, what may be cached — and the shell's build script generates the payload as
a static table from `t3code/apps/web/dist`. The two are split so that the policy
is tested against three files while the four hundred stay out of every test
binary.

Two consequences are load-bearing enough to be part of the decision:

- **The port is fixed** (`crate::launch::DEFAULT_PORT`). The port is part of the
  origin, so an ephemeral one would hand the app a different origin on every
  launch and silently lose everything in `localStorage` each time. A port
  already in use is a refusal with a sentence; that is the trade.
- **Nothing is cross-origin, so nothing was widened.** The auth test above
  stands unchanged, and now records why rather than predicting a change.

## Consequences

- **Upstream's bundle ships exactly as upstream built it.** No rebuild, no
  environment variables, no Node in the build. Source maps are dropped — 37 MB
  of the 54 MB — and nothing else is touched.
- **The window is a browser pointed at a local server, and that is a feature.**
  A developer can open the same URL in Chrome and get the same application, with
  the same state, because it is the same origin. That is how every ticket before
  this one was driven and it keeps working.
- **`Server` gained a parameter rather than the shell gaining a server.** Every
  caller passes `Assets::none()` except the shell. A server with no UI answers
  404 at `/` exactly as it did before, which is what keeps `http_boot.rs` true.
- **A missing file is a 404 and a missing _route_ is the page.** The distinction
  is drawn on whether the last path segment has an extension. Getting it wrong
  in the generous direction would serve HTML where a script was asked for, and
  the webview would run it — so the rule is deliberately mean, and the server's
  own `/api` and `/.well-known` prefixes are excluded from it entirely.
- **The bytes are copied once per request.** `crate::ui` is free of `axum`, like
  `crate::auth` and `crate::http`, so the handler copies at the edge rather than
  handing out an `axum::body::Bytes`. At most a few megabytes, once per window,
  over loopback.
- **This is Windows-shaped reasoning.** `tauri.localhost` is the Windows
  spelling; macOS uses `tauri://localhost`, which _would_ pass the origin check.
  The decision still holds there — the other two problems are platform-neutral —
  but the first bullet would not be the one that decided it.
