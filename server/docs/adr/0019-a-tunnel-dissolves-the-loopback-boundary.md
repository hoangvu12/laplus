# ADR-0019 — A tunnel dissolves the loopback boundary, so the window carries a credential like everything else

Date: 2026-07-29
Status: Accepted
Supersedes: [ADR-0015](0015-a-credential-that-opens-the-socket-reads-a-snapshot.md)

## Context

Ticket 73 exists because the user wants to drive laplus from their phone, with
the PC reached through `cloudflared` on a custom domain. Today that fails at the
socket upgrade, because the browser sends `Origin: https://…` and
`crate::auth::authorize` refuses any host that is not loopback.

ADR-0015 recorded the posture that refusal sits in: **`authorize` verifies no
credential at all and accepts an absent one.** That was a real decision and it
was a sound one. v1 had no identity store to check a credential against and no
pairing flow to build one on, and the reasoning that made it safe was explicit —
reachability is the boundary. The listener binds to loopback, so only a program
already running as the user can reach the port, and such a program is already
the user. The origin check was there for the one thing loopback does not stop: a
page on another origin asking the user's own browser to connect on its behalf.

**A tunnel does not stretch that reasoning. It dissolves it.**

`cloudflared` runs on this machine and dials `127.0.0.1`. A request that came
from the far side of the world therefore arrives with the same peer address as
the window's, and with whatever headers its sender cared to send. There is no
signal at the HTTP layer that distinguishes the two.

That matters more than it first appears, because it defeats the rule ticket 73
itself proposed:

> A loopback origin keeps the permissive posture. A non-loopback origin must
> present a credential that verifies.

An `Origin` header is only ever sent by a browser, and only on WebSocket
upgrades and requests that are not `GET`. **A request with no `Origin` at all
was accepted unconditionally** — the check lived inside `if let Some(origin)`.
So the rule as written is bypassed by not sending the header:

```
wscat -c wss://the-tunnel.example/ws
```

no credential, no origin, and the caller has a shell running as the user. The
origin check has never been able to constrain a program rather than a page, and
before a tunnel existed it did not have to.

### What the reference server does

It refuses. `auth/EnvironmentAuth.ts:592-601`:

```ts
const credential = cookieToken ?? bearerToken ?? dpopToken;
if (!credential) {
  return Effect.fail(new ServerAuthMissingCredentialError({}));
}
```

There is no exemption, and none is needed for its own desktop window, because
that window holds a real credential. Electron gives the main process a private
channel into the renderer, and `PairingGrantStore.ts:314-330` uses it: a
`desktopBootstrapToken` seeded at boot as an unbounded-use grant with
administrative scopes, handed to the page inside the desktop process. The
comment there is explicit that the seed "stays inside the desktop process and
the rendered page".

So the difference between laplus and upstream was never the exposure model —
upstream has a `host` setting and Tailscale Serve support and expects to be
tunnelled. It was that **laplus's window was exempt from the check and
upstream's was not.**

### Why laplus could not simply copy the preload

laplus's shell has no Tauri commands at all, and its window is a webview pointed
at `http://127.0.0.1:4773/`. Everything the page knows, it got by making an HTTP
request to this server. Baking a boot secret into the served page would turn it
into "a thing you get by making an HTTP request" — and a tunneled request is an
HTTP request, so the phone-sized hole would reopen at the page load.

What laplus has instead is the **URL fragment**. A fragment is never sent to the
server; the browser keeps it and hands it to the page's JavaScript. The shell
chooses the URL its own window opens, so a credential placed there reaches the
window and nothing that merely reaches the server. That is the same property
Electron's preload has, obtained differently.

It is also not an invention: `issueStartupPairingUrl`
(`EnvironmentAuth.ts:911-921`) builds exactly this URL upstream, and the client
half — `setPairingTokenOnUrl` / `getPairingTokenFromUrl` in
`packages/shared/src/remote.ts` — already exists and is untouched.

## Decision

**Every request must present a credential that verifies. The desktop window
stops being exempt and starts carrying one.**

Four parts:

1. **`crate::auth::authorize` settles the origin and nothing else**, returning a
   `Presented` — the shape that arrived and the token to check. It is an
   obligation, not a permission. Verification is a database read and
   `crate::store` is the only file that speaks SQL, so the two halves meet in
   one function in `crate::server` and nowhere else.

2. **An origin is admissible if it is this machine, or a host the user named**
   in `remote-access.json` in the preferences directory (`crate::remote_access`).
   Empty by default; a file that will not parse admits nothing, so the failure
   mode of a typo is a phone that cannot connect rather than a machine that
   admits everybody.

3. **A request with no credential is refused**, with the contract's
   `missing_credential`. This is the reversal of ADR-0015 and the whole of this
   ADR's cost.

4. **The server mints a boot grant at startup** and the shell opens its window
   on `http://127.0.0.1:4773/#token=…` (`Server::window_url`). The grant is
   re-usable, expires in 24 hours, is revocable, and is excluded from the
   pairing-link list Settings shows.

Two routes take their credential in the request _body_ and so check only the
origin: `POST /api/auth/browser-session` and `POST /oauth/token`. They are how a
client holding nothing comes to hold something, so requiring a session would be
requiring the thing they exist to issue. What they accept is a pairing code —
minted by this server, single use, five minutes, revocable.

### Why the boot grant is re-usable when a phone's code is not

The window re-reads its credential out of the address bar on every page load.
A strictly single-use boot grant would let the developer press F5 exactly once
and then lock them out of their own window. Upstream hit the same wall and
answered it the same way — `remainingUses: "unbounded"`, with a comment saying
it is for reloads.

A code carried to a phone is the opposite case: it is read aloud off one screen
and typed into another, so the second use of one is somebody who should not have
it.

The exemption is from _spending_, not from checking. A re-usable row is still
refused when revoked or expired, which is what keeps it a credential rather than
a permanent hole. `revoke_pairing_link` had to learn the same exemption — without
it, stamping the boot grant on first use would have made the longest-lived
credential in the system the one nothing could withdraw.

## Consequences

- **ADR-0015's central claim is reversed.** "A request with no credential is
  answered. It is the case the shipped UI is in, not an edge one." It is no
  longer the case the shipped UI is in: the window pairs at boot. 0015's _other_
  claim — that these routes are exactly as strong as the socket, by construction
  — survives untouched and is stronger now, because there is something for them
  to be equally strong about.

- **This is the change that can lock the user out of their own window**, which is
  why the boot grant landed before the check did rather than after.
  `the_window_reaches_these_routes_with_the_credential_it_booted_with` and
  `the_boot_credential_survives_being_spent_so_a_reload_still_opens` in
  `tests/http_pairing.rs` are the two that would fail first.

- **The test harness pairs itself the way the window does** — it reads the boot
  code out of `window_url`'s fragment and trades it through the real routes.
  Nothing in the suite reaches around the policy it is setting up, which is what
  keeps two hundred tests from quietly proving the wrong thing.

- **`laplus-server` prints its boot URL.** The standalone binary has no window
  to open one in, and a browser pointed at it needs a credential like anything
  else. Upstream prints the same URL for the same reason.

- **A development server on another port is a different origin** and needs a
  line in `remote-access.json`, where before it needed nothing. This is a real
  cost to the inner loop and is the price of the origin no longer being
  advisory.

- **A tunnel is still not something to recommend casually.** What this ADR buys
  is that reaching the URL is no longer sufficient. It does not authenticate the
  tunnel itself, and Cloudflare Access or a Tailscale-only tunnel remains good
  practice in front of it — now as a second lock rather than the only one.

- **DPoP is refused rather than ignored.** The shape is read at the upgrade and
  advertised in the descriptor's `sessionMethods`, because the client may send
  one; accepting it as a bearer would be taking a credential while ignoring the
  proof that is the entire point of the scheme.

- **Scopes are still recorded and never enforced.** Nothing here changes that,
  and the boot grant's administrative scopes are what the window _reports_,
  not what it is permitted. Adding enforcement is work in `crate::rpc`.
