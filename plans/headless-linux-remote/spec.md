# laplus without a window: a server on Linux, clients everywhere else

> _Written 2026-07-29 from a source read of `9959d6c`. Every claim below cites
> the file it was read from, because the interesting result of that read was how
> much of this already exists._
>
> _`reference/` was deleted the same day, so upstream is no longer readable in
> this tree. Citations to it use the form `crate::auth`'s own comment already
> uses — `pingdotgg/t3code:apps/server/src/…` — and have to be read on GitHub.
> Everything upstream that these tickets actually depend on is quoted inline._
>
> _This lives in `plans/` rather than the `.scratch/<feature-slug>/` location
> `server/docs/agents/issue-tracker.md` documents, because `.scratch/` was
> emptied on 2026-07-29 and writing here would have resurrected it mid-commit.
> The file shapes are the tracker's — `spec.md`, `issues/NN-slug.md`, a
> `Status:` line per ticket — so this can move back under `.scratch/` unchanged
> if that is where it should end up._

**What this is for:** run `laplus-server` on a Linux box, and drive it from a
phone's browser and from the desktop application on another machine.

**Status:** needs-triage

## The shape of the thing

Two clients, and they are not the same problem:

| Client              | How it reaches the server                                                    | What it needs                          |
| ------------------- | ---------------------------------------------------------------------------- | -------------------------------------- |
| **Phone (browser)** | Loads the page _from_ the Linux box, so page and API share an origin         | The server must serve the UI           |
| **Desktop app**     | Page is served by its _own_ loopback server; it fetches a second, remote one | Cross-origin requests must be answered |

The phone case is the smaller of the two and does not depend on the other. It
should land first.

## What already works, verified

This is the part worth reading before planning anything, because the remote
story is much further along than the absence of a Linux build suggests.

**Origin is not checked. At all.** `auth::authorize` returns a `Presented` built
only from the credential — `wsTicket` query parameter, then `Bearer`, then
`DPoP`, then the session cookie, then `Absent`. It never reads
`UpgradeRequest.origin`, which is populated at `server.rs:1272` and consulted by
nothing. The allowlist that ADR-0019 describes was removed afterwards; that ADR's
text is stale on this point and `crate::auth`'s own module comment is the current
statement.

So a LAN address, a `trycloudflare.com` hostname and a tailnet name all reach
this server without anything being written down. **No allowlist work is in
scope, anywhere in this effort.**

**A credential is the whole boundary.** `authorized` in `crate::server`: absent
is 401 `missing_credential`; a `wsTicket` is consumed (single use, spent at the
upgrade); a bearer or session cookie is checked against the session table; DPoP
is refused because this server implements no proof-of-possession. A database
error is deliberately _not_ a 401.

**The pairing routes all exist** — `/oauth/token`,
`/api/auth/browser-session`, `/api/auth/websocket-ticket`,
`/api/auth/pairing-token`, `/api/auth/pairing-links`,
`/api/auth/pairing-links/revoke`, and `GET /.well-known/t3/environment`.

**The descriptor is unauthenticated**, which is what makes bootstrapping
possible at all: `environment_descriptor` is the one handler that never calls
`authorized`. A client that holds nothing can still discover what it is talking
to.

**The client half is complete and untouched.**
`preparePairingRegistration` (`packages/client-runtime/src/connection/onboarding.ts`)
does exactly three things: resolve the target, `GET /.well-known/t3/environment`,
`POST /oauth/token` — then stores a `BearerConnectionProfile`. At connect time
`authorization/remote.ts:90` trades the bearer for a ticket and `:118` opens
`wss://…/ws?wsTicket=…`. Every one of those is a route this server answers.

**The UI has the screens.** `apps/web/src/components/auth/PairingRouteSurface.tsx`
and `apps/web/src/routes/pair.tsx` are the phone's pairing screen;
`ConnectionsSettings.tsx` has "Remote environments → Add environment → Remote
link", gated on nothing (only the SSH card checks `desktopBridge`).

**The LAN address is already computed.** `endpoints::lan_address` asks the
routing table by `connect`ing a UDP socket at TEST-NET-3 and reading back the
local address — no interfaces enumerated, no dependency. `advertised_host`
returns it whenever exposure is network-accessible.

**Binding wide already works.** `RemoteAccess::bind_address` answers `0.0.0.0`
when `remote-access.json` says `network-accessible`, and `Server::bind` uses it.

**The server code is written cross-platform throughout.** Every
`std::os::windows` use sits under `#[cfg(windows)]` with a `not(windows)` twin
(`files.rs`, `filesystem.rs`, `process.rs`); `config.rs:535` falls back to
`XDG_DATA_HOME` then `$HOME/.laplus`; `terminal.rs:1345` falls back to `$SHELL`
then `/bin/zsh`, `/bin/bash`, `/bin/sh`; `PATHEXT` handling is empty off Windows.

## What is missing

Four things, and none of them is the security model.

1. **The plain binary serves no UI.** `laplus-server/src/main.rs` passes
   `Assets::none()`, so `Assets::files` is empty, so `Assets::resolve` declines
   every path and the `asset` fallback answers `404`. A phone gets a bare 404 at
   `/`, not a pairing screen. → ticket 01
2. **No CORS, structurally.** The router has zero `.layer()` calls and
   `laplus-server` depends on no HTTP middleware crate. The only
   `Access-Control` string in the crate is a comment explaining why the 401 does
   not carry one. The desktop app dies on the _first_ call of
   `preparePairingRegistration`. → ticket 02
3. **The boot URL names loopback.** `Server::reachable_from` rewrites `0.0.0.0`
   to `127.0.0.1` — correct for a window on the same machine, useless printed on
   a headless box. `endpoints::advertised_host` already knows the right answer
   and is called only from `laplus-shell/src/main.rs:242-247`. → ticket 03
4. **The exposure switch is a Tauri command.** `set_network_exposure` lives in
   the shell, so a headless server has no way to turn network access on but to
   hand-write `remote-access.json`. → ticket 04

And one unknown: **nobody has ever built this on Linux.** Rust CI is
`windows-latest` only. → ticket 05

## Order

```
05 (Linux build)  ──┐
01 (serve the UI) ──┼──►  phone works
04 (exposure)     ──┤
03 (print the URL)──┘

02 (CORS) ───────────────►  desktop app → remote server
```

Ticket 02 is the only one the phone does not need, and the only one that is
about a second origin. If the desktop-app-as-remote-client is not wanted, 02 can
be dropped without touching the others.

## Decisions taken here, so the tickets do not each re-argue them

**The UI is loaded at runtime, not embedded.** The workspace `Cargo.toml` keeps
`default-members = ["crates/laplus-server", "xtask"]` and excludes the shell
precisely because the shell embeds `apps/web/dist`, and a fresh clone must be
able to run `cargo test` without a `pnpm` build first. Making `laplus-server`
embed the bundle at compile time would destroy that property for the crate the
comment exists to protect. Upstream reaches the same answer from the other
direction — `ServerConfig.staticDir` and `resolveStaticDir()` find `client/` or
`apps/web/dist` at runtime.

This is cheap because `Assets` was already built for it: `files` is a
`BTreeMap<String, Cow<'static, [u8]>>`, so owned bytes need no new type. Only
`version: Option<&'static str>` has to widen.

**CORS is hand-written, not a dependency.** The workspace manifest is explicit
that a dependency has to earn its way in, and upstream's entire CORS surface is
fourteen lines (`pingdotgg/t3code:apps/server/src/httpCors.ts`). `tower-http` appears
in `Cargo.lock` only through `reqwest`, which belongs to the shell's updater
plugin, and is not available to `laplus-server` as a direct dependency today.

## Out of scope

- **HTTPS.** This server speaks plain HTTP and ADR-0022 already says so. Put
  Tailscale or a tunnel with TLS in front of it; that is the answer, not a
  certificate loader here.
- **DPoP.** Refused deliberately, ticket 73's decision, unchanged.
- **Tailscale integration.** Upstream drives `tailscale serve` over its Electron
  bridge and advertises a MagicDNS endpoint; `crate::endpoints` says why laplus
  advertises none. A tailnet name reaches this server anyway.
- **An origin allowlist.** Removed on purpose. See above.
- **The desktop application on Linux.** `laplus-shell` is Tauri and would need
  WebKitGTK; it is excluded from `default-members` and stays excluded. This
  effort puts the _server_ on Linux.

## The cost, stated plainly

ADR-0022 already wrote the honest version and it is not softened by anything
here: turning this on puts a process that runs `claude` as you, with your
terminals and your filesystem behind it, on a network. What makes it defensible
is that reaching the port is not the same as being let in — an absent credential
is refused, a pairing code is twelve characters and single-use with a five-minute
life, and a session is a row that can be revoked.

Two things get worse on a headless box and should be said in the documentation
ticket 04 writes:

- **`auth.clients` is still not implemented**, so a session handed to a device
  cannot be revoked from the UI. On a machine you sit in front of that was
  "nice, not load-bearing" (ticket 73). On a server you do not, it is closer to
  load-bearing.
- **There is no window to notice anything.** Every failure mode here is a log
  line on a box nobody is looking at.
