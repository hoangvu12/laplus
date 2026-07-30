# ADR-0029 — A credential can be minted beside the server, rather than read out of it

Date: 2026-07-31
Status: Accepted

## Context

ADR-0028 gave the server a way to run in the background, and in doing so took
away the terminal it had been printing its startup credential to. The answer
that shipped with it was "read `~/.laplus/logs/service.log`", which is a bad
answer twice over: pairing a device becomes a log-reading exercise, and the one
credential in that file is reusable for twenty-four hours and rotates only when
the server restarts.

Upstream had a better one already, and it is worth being precise about what it
is. `t3`'s background service has **exactly the same problem** — its unit runs
`serve` with `StandardOutput=append:`, so its startup output, QR code included,
goes into a log file. Nobody meets that problem, because
`t3 auth pairing create` exists (`apps/server/src/cli/auth.ts`). The log is for
diagnosing a server that will not start. To get in, you mint.

## Decision

### `auth pairing create|list|revoke`, against the database

Three verbs mirroring upstream's, with its flags: `--ttl`, `--label`,
`--base-url`, `--json`.

**They open the running server's SQLite file and never speak to the server.**
This is the whole mechanism and it is available only because
`Database::issue_pairing_link` already says of a pairing code that "the row is
the code's whole existence: there is no in-memory half". A second process can
therefore write a credential the running server will honour, without discovering
its address, without a bearer token to authenticate the request with, and
without the server having to expose an endpoint for it.

The alternative — an authenticated HTTP call to the running server — needs the
address (which the CLI does not know), a credential (which is what is being
asked for), and a new route. It is strictly more moving parts to reach the same
row.

`list` never prints credentials, as upstream's does not. What an operator does
with that output is decide what to revoke, which needs an id and a label; a list
that printed secrets would put every live code into a scrollback every time
anyone looked.

### Minted codes are single-use and short, unlike the boot grant

Five minutes and one use, matching the code Settings mints and deliberately not
matching the boot grant in the log, which is reusable for twenty-four hours so
that a page reload cannot lock the operator out of their own window
(ADR-0022 and `crate::pairing`). A code minted to be carried to one device has
no business working twice.

### `--base-url`, because the machine cannot see how it is reached

The LAN address is the default and is right for the case this feature exists
for. It is not right for a tunnel, a tailnet name or a reverse proxy, none of
which appear in this machine's routing table — so the operator can say, exactly
as upstream's flag lets them.

With neither, the code is printed **without a URL** rather than with a
`127.0.0.1` one. A loopback pairing URL is not a degraded answer; it is a link
that works nowhere except the machine that cannot use it.

### A QR code, which is where this stops copying and starts improving

`crate::qr` renders the pairing URL as half-block characters. It is printed by
`auth pairing create` and by the startup announcement of any server that has a
LAN address.

The startup half closes a follow-up `crate::startup` had left in a comment since
ticket 03 — "a QR code would beat both and is a dependency" — and the comment
was right about which is better. Pairing a phone with a headless box means
moving twelve characters from an SSH window to a device that cannot paste from
it. Every alternative is a person transcribing a credential.

**Upstream prints one from `serve` but not from `auth pairing create`.** Ours
prints it from both, because the code most likely to be carried to a phone is
the one somebody explicitly minted for a phone.

`Line::Drawn` is a third variant rather than another `Said` for one concrete
reason: `main.rs` prefixes every sentence with `laplus: `, and a prefix on the
first row of a QR code lands inside its quiet zone and stops a camera finding
the edge.

The dependency is `qrcode`, default features off — pure Rust, no build script
and no C, so ADR-0026's musl targets are untouched. Encoding is Reed-Solomon,
mask selection and version fitting; upstream hand-rolls only the _renderer_ and
uses a library for the encoder too.

### `Ttl` gained a lifetime

`--ttl` is a value an operator types, and `Ttl` held a `&'static str`. It now
borrows, so a runtime string can be one. The alternative was leaking or an owned
string in a type that is `Copy` and is embedded in another `Copy` type; a
lifetime parameter costs four `Ttl<'static>` annotations on the constants and
nothing else.

### The database is opened by two processes, so it waits

`busy_timeout` is five seconds, set at every open. SQLite's default is to fail a
busy write _immediately_, so without it `auth pairing create` would report
`database is locked` whenever it landed during a turn — a failure that depends
on timing, appears only under load, and would have been found by a user rather
than by this repository. The server gets the same rule and benefits from it in
the same way.

## Consequences

- **A second writer is now a supported thing.** This crate has assumed one
  process on `state.sqlite` since it was written. Nothing about the schema
  changes, but "the server owns the file" is no longer true, and the next thing
  that wants to write from outside should read this ADR before assuming the
  server is the only one there.
- **The log stops being the way in.** It still carries the startup credential
  and is still a secret, but `running-headless.md` no longer sends anyone to
  read it in order to pair. Diagnosing a server that will not start is what it
  is for.
- **Scopes are granted in full.** A minted code carries all eight
  `ENVIRONMENT_SCOPES`, like the boot grant. Nothing gates on scopes
  (`crate::pairing` argues why), so a narrower default would be a claim this
  server does not enforce — and upstream mints with its standard client set for
  the same reason. A `--scopes` flag can be added when there is enforcement to
  make it mean something.
- **Two more verbs the npm launcher forwards without knowing about.**
  ADR-0028's shape holds: `laplus-server` grew a command line, `apps/cli` grew
  help text. That is now twice this has cost the launcher nothing, which is the
  evidence for the decision rather than the argument for it.
- **The startup output got taller.** A QR code is about twenty rows. It is
  printed only when there is a LAN address — a loopback server prints none —
  so the `cargo run` case a developer sees fifty times a day is unchanged.
