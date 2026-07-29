# ADR-0022 — A verified credential is what lets the listener leave loopback

Date: 2026-07-29
Status: Accepted
Amends: [ADR-0019](0019-a-tunnel-dissolves-the-loopback-boundary.md)

## Context

Ticket 73 put "making the server bind off-loopback" out of scope in as many
words, and gave the reason: _"The tunnel runs locally and does not need it.
Keeping the bind is a second lock on the door."_ That was right when it was
written, and both halves of it have since changed.

**The first lock is now real.** ADR-0019 removed the exemption that let a
request carrying nothing open a socket. Every request presents a credential that
verifies against the store, the window included. Before that, "keeping the bind"
was not a second lock — it was the only one, and calling it second was generous.

**And the tunnel turned out to be the awkward path, not the easy one.** It needs
`cloudflared` or Tailscale installed and running, a hostname that changes on
every restart of a quick tunnel, and — because that hostname resolves to
somebody else's edge and arrives here as an `Origin` this machine has never
heard of — a line in `remote-access.json` naming it. The user's words for that
file were "a bit too manual", and the specific request was to do what T3 Code
does: a switch in Settings.

Upstream has always had one. `EnvironmentAuthPolicy.ts:18` settles its auth
policy from `isRemoteReachableHost(config.host)`, its desktop binds whatever
host that reports, and `ConnectionsSettings.tsx` draws the switch that moves it.
laplus already ships that panel — it is the same `apps/web` — and the switch was
rendered permanently disabled with a tooltip saying exposure "must be controlled
where the server process is launched", which in an application you open by
double-clicking is a sentence with no action behind it.

## Decision

**The listener binds `0.0.0.0` when the user turns network access on, and
`127.0.0.1` otherwise.** Off by default; nothing changes on a machine whose
owner does not go and change it.

The mode lives in `remote-access.json` beside the tunnel hostnames, because both
answer "who may reach this server" and `ServerSettings` is the contract's and
closed. `crate::remote_access::Exposure` is the type; `bind_address()` is the
whole of what it decides.

**Both controls stay.** Binding wide answers the LAN and says nothing about a
`trycloudflare.com` hostname, which still arrives as an unknown origin. Upstream
keeps both for the same reason, and a user with a tunnel is not helped by a
switch about interfaces.

Three consequences worth naming, because each is a place this could have been
got wrong:

- **The origin check has to admit this machine's own LAN address.** A phone
  loads the page from `http://192.168.10.45:4773` and opens a socket carrying
  that as its `Origin`. Nothing had heard of it, so without this the switch
  would bind the port, serve the page, and refuse the socket — ticket 73's
  original bug, moved one address along. `RemoteAccess::allows` admits _this
  machine's own address_ and not the subnet.
- **`0.0.0.0` is an address to bind and not one to reach.** The window is
  pointed at `Server::http_url`, which was `local_addr` — so the first time the
  switch was turned on, the window opened at `http://0.0.0.0:4773/#token=…` and
  the webview answered `ERR_ADDRESS_INVALID`. Every URL handed to a client on
  this machine now names loopback explicitly and takes only the port from the
  listener.
- **Changing it restarts the application.** A listener cannot be moved out from
  under its open sockets. Upstream restarts its backend here and the
  confirmation dialog in `ConnectionsSettings` already says laplus will. The
  hostname list has no such problem — `crate::auth` reads it per request — so
  that half applies immediately, and the asymmetry is deliberate rather than an
  oversight.

## What makes this safe enough, and what it costs

The honest version: **turning this on puts a process that runs `claude` as you,
with your terminals and your filesystem behind it, on your network.** That is
the actual exposure, and no amount of pairing makes it nothing.

What makes it defensible is that reaching the port is no longer the same as
being let in. Since ADR-0019 an absent credential is refused, a pairing code is
twelve characters, single-use and expires in five minutes, and a session is a
row that can be revoked. A stranger on the same coffee-shop Wi-Fi who finds the
port gets a pairing screen.

What it costs, plainly:

- **The credential check is now load-bearing in a way it was not.** Under
  loopback a bug in it was a bug behind a locked door. It is now the door.
- **HTTP, not HTTPS.** Anyone on the path sees the traffic and the bearer token.
  A tunnel with TLS is still the better answer for anything but a home network,
  and this does not replace it.
- **`auth.clients` is still out of scope**, so a session handed to a device on
  the network cannot be revoked from the UI. Ticket 73 left that "nice, not
  load-bearing"; with a LAN switch it is closer to load-bearing, and it should
  be reconsidered rather than inherited.
- **Windows will ask about the firewall** the first time, and the answer to that
  prompt decides whether any of this works. Declining leaves a switch that reads
  as on and a machine nothing can reach — which is a state the UI cannot
  currently see or explain.

## Alternatives

**Leave the bind alone and put a Settings field on `remote-access.json`.**
Smaller, keeps the second lock, and was offered. It does not do what was asked:
a tunnel is still required for a phone on the same sofa as the PC.

**Fake `window.desktopBridge` so the existing bridge path lights up.** The
shortest diff and the worst idea. `desktopBridge` is consulted in two dozen
files as "am I on the desktop", and a partial one answers yes to all of them —
including the boot-credential lookup, which would stop falling back to the URL
fragment and leave the window unable to open a socket at all. The shell exposes
named commands instead (ADR-0021), and the network-access seam is an injected
object rather than a global.

**Bind a named interface rather than `0.0.0.0`.** More precise, and it needs a
UI for choosing between the six adapters a Windows machine with Hyper-V, WSL and
a VPN reports. Upstream binds the wildcard; so does this.
