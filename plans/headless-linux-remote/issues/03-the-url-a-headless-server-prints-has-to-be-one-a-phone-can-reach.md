# 03 — The URL a headless server prints has to be one a phone can reach

**What to build:** teach `laplus-server`'s startup output to name the address
other machines reach it at, instead of loopback.

**Status:** ready-for-human

**Depends on:** 01 (a URL is only worth printing once there is a page at it)

## Why

`laplus-server` prints two lines at startup, both built from
`Server::reachable_addr`, which runs everything through:

```rust
fn reachable_from(bound: SocketAddr) -> SocketAddr {
    if bound.ip().is_unspecified() {
        return SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, bound.port()));
    }
    bound
}
```

That rewrite is correct and must stay. ADR-0022 records why it exists: the first
time the network switch was turned on, the shell opened its window at
`http://0.0.0.0:4773/` and the webview answered `ERR_ADDRESS_INVALID`. Every URL
handed to a client _on this machine_ names loopback explicitly.

On a headless box that is precisely backwards. The server binds `0.0.0.0`, and
prints:

```
laplus: open http://127.0.0.1:4773/#token=ABCD2345WXYZ
```

which is useless on the phone it was printed for. The credential in it is
correct; the host is not.

## The answer already exists

`crate::endpoints` solves this and is currently called from one place —
`laplus-shell/src/main.rs:242-247`, feeding the Settings panel. The plain binary
never touches it.

```rust
pub fn lan_address() -> Option<std::net::Ipv4Addr> {
    let socket = std::net::UdpSocket::bind(("0.0.0.0", 0)).ok()?;
    socket.connect(("203.0.113.1", 80)).ok()?;
    ...
}
```

A routing-table lookup, not a conversation — `connect` on a UDP socket sends
nothing, and TEST-NET-3 is reserved by RFC 5737 so it cannot be mistaken for
reaching a real host. `advertised_host` wraps it and returns `None` unless
exposure is network-accessible, which is the right guard: a LAN URL printed by a
loopback-bound server would refuse every connection made to it.

So this ticket is **wiring, not arithmetic**. Note that the module comment on
`reachable_addr` already points here — "The addresses other machines use are
`crate::endpoints`'s, which is a different question with a different answer."

## What to build

In `laplus-server/src/main.rs`, after binding:

1. Ask `endpoints::advertised_host` for the LAN address.
2. When there is one, print the pairing URL against it as the **primary** line,
   and keep the loopback line beneath it — a developer running this on their own
   machine still wants the local one, and both are true.
3. When there is not one — loopback-bound, or a box with no route off itself —
   print what is printed today and say which of the two it is. "No network
   address: this server is bound to loopback" and "no route off this machine"
   are different problems with different fixes, and the user is at a terminal
   where a sentence is cheap.
4. Print the exposure mode explicitly. On a machine with no window, the only way
   to find out whether the switch is on is to be told.

Keep `Server::window_url` as it is. It is the _window's_ URL and the shell is
right to want loopback; this is a second question asked at the same moment.

## Read the reference

Upstream answers the same question in
`pingdotgg/t3code:apps/server/src/startupAccess.ts`:
`resolveHeadlessConnectionHost` sees a wildcard bind, walks
`os.networkInterfaces()` and takes the first non-internal IPv4.
`resolveHeadlessConnectionString` builds `http://host:port` from it, and
`t3 serve` prints a connection string, a pairing token, a pairing URL and a QR
code.

laplus should print the first three. **Do not port the interface walk** —
`crate::endpoints`' own comment rejects it deliberately, because it needs a rule
for choosing between the six adapters a Windows machine with Hyper-V, WSL and a
VPN reports, and the routing table already holds that answer.

A QR code is genuinely useful for a phone and is genuinely a dependency. Leave
it out of this ticket; note it as a follow-up if typing twelve characters proves
annoying in practice.

## Acceptance criteria

- Bound to `0.0.0.0` on a machine with a route off itself, the primary printed
  URL names the LAN address and carries the boot credential in its fragment.
- Bound to loopback, output is what it is today.
- Bound to `0.0.0.0` on a machine with no route off itself, the loopback URL is
  printed with a sentence saying no network address was found.
- The exposure mode is stated on every start.
- `reachable_from` and its tests are untouched.
- A phone on the same network opens the printed URL, lands on the pairing
  screen, and pairs. A drive, not a test.

## Out of scope

- QR codes.
- IPv6. `lan_address` answers `Ipv4Addr` today and widening it is a separate
  question about the whole endpoints surface.
- Printing anything the shell prints. The shell has a Settings panel for this
  and it already works.

## What landed

Wiring, as the ticket predicted — but the wiring turned out to be a decision
table worth its own module. `crate::startup` takes what was bound, where the
mode came from, and the addresses, and answers with the lines to print;
`laplus-server/src/main.rs` holds nothing but the reading and the `println!`.
That split is the only reason any of this is tested: a binary's `main` is the
one part of this crate no test runs.

```
laplus: network access is on, from --network — this server is on your network
laplus: open http://192.168.1.42:4773/#token=ABCD2345WXYZ
laplus: or open http://192.168.1.42:4773/ and pair with ABCD2345WXYZ
laplus: on this machine, http://127.0.0.1:4773/#token=ABCD2345WXYZ
```

The LAN URL is primary and the loopback one stays beneath it. `reachable_from`,
`reachable_addr` and `window_url` are untouched, along with their tests;
`Server` grew `url_for(host)` and `pairing_url_for(host)` beside them, which are
the same arithmetic against a host that is not this machine.

**The third line is the ticket's "print the first three".** Upstream's
`t3 serve` prints a connection string, a pairing token and a pairing URL, and
the reason holds harder here than there: the URL above it is being typed by hand
into a phone, and a bare address followed by twelve characters into the pairing
screen is a great deal less to get right than the same thing with `/#token=` in
the middle of it. A QR code would beat both and is still the follow-up this
ticket said it was.

**Not finding a LAN address is told apart from not looking for one.** Bound to
loopback there is nothing to look for and the exposure line has already said so
— so the output is what it was, rather than a complaint greeting every
`cargo run`. Bound wide with no route off the machine gets a sentence naming
that cause, on stderr, because the operator asked for something they did not
get. `startup::Line` has two variants for exactly this: the announcement is not
all ordinary output, and an operator who redirects stdout should still see the
half that went wrong.

`endpoints::advertised_host` is called for the first time outside the shell and
is unchanged. Its exposure guard is what makes the two states above
distinguishable at all — it answers `None` for both, and `Exposure` says which.

### Driven, not only tested

Against the machine's own LAN address, with the real bundle: `GET /` served the
3192-byte page, the printed credential exchanged at `POST /oauth/token` for a
thirty-day bearer, that bearer was accepted at `/api/auth/session`, and the
descriptor reported `policy: remote-reachable` — which is the `--network`
override carrying the auth policy with it, end to end. The boot grant was
confirmed reusable by spending it twice.

### What is left

**The last acceptance criterion is unmet and cannot be met here.** "A phone on
the same network opens the printed URL, lands on the pairing screen, and pairs.
A drive, not a test." Everything above was driven from `curl` on the machine
running the server. Nobody has held a handset.
