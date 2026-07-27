# ADR-0011 — The server reports the version of the UI it ships

Date: 2026-07-27
Status: Accepted

## Context

Upstream's client compares its own `APP_VERSION` — the number Vite compiles into
the bundle — against `environment.serverVersion` from `server.getConfig`, by
string equality (`apps/web/src/versionSkew.ts`). When they differ it raises a
banner above the composer: *Client 0.0.28 is connected to server 0.1.0. Relaunch
the server with the copied command to sync them.*

That check is written for upstream's shape, where a long-running server is talked
to by whatever browser session reaches it, and the two really can be different
builds. lightcode is not that shape. The shell is **one executable**: the bundle
is embedded by `lightcode-shell`'s build script and served by the same process
that answers the socket. There is no arrangement of the two halves that could
produce a skew, and no relaunch that could resolve one.

Reporting `env!("CARGO_PKG_VERSION")` — `0.1.0`, this workspace's version — meant
every first launch in a fresh profile opened on a warning about a problem that
did not exist, offering advice the developer could not act on. Ticket 26.

Three options were weighed (ticket 26 records them): report the vendored UI's
version, read the version out of the bundle's own `package.json` at build time,
or leave the banner and accept the click.

## Decision

**A server that ships a UI advertises that UI's version as `serverVersion`. A
server that ships no UI keeps the crate's.**

`lightcode-shell`'s build script reads `version` from
`t3code/apps/web/package.json` — the file `dist/` was built from — and emits it
beside the asset table. It travels with the bytes in `ui::Assets`, and
`Server::bind_with` applies it through `ServerConfig::serving_ui_version`, which
is where the reasoning is written down for the next reader.

The plain `lightcode-server` binary carries no bundle, so it keeps
`CARGO_PKG_VERSION`. That is the honest answer for a server pointed at by a UI
built somewhere else: there, a difference is a real one.

**What this is not.** The two numbers are made equal because they describe one
artifact, not because a skew check now passes on its merits. The check is
*vestigial* in lightcode. Nobody should read a quiet banner here as evidence that
client and server were compared and found to match.

## Consequences

- **The first screen of a fresh profile has nothing wrong on it.** Verified with
  `tools/ui-driver/probe-boot.mjs` against both builds: the old one raises the
  banner, the new one does not.
- **`serverVersion` no longer names the server.** Anything that wants to know
  which lightcode is running cannot ask this field in the shell — it will hear
  the UI's number. Nothing reads it that way today; the field's readers are the
  client's connection catalogue and this banner.
- **The advertised version is coupled to vendored code.** A `pnpm` build in
  `t3code/` can change what the shell reports. The coupling was already paid —
  the shell cannot build without that checkout — and a second file read from it
  adds no new dependency.
- **A build with `APP_VERSION` set would put the banner back.** Vite prefers that
  environment variable over `package.json`, and the build script cannot see it.
  `the_version_reported_is_the_one_the_ui_compares_against` in the shell's tests
  is the guard: it looks for `APP_VERSION` in the shipped JavaScript and requires
  at least one sighting to carry the number the server will report — every
  sighting is the same substitution, so a bundle built that way has none. It has
  to be **asked for by name** (`cargo test -p lightcode-shell`), like everything
  else in that crate: plain `cargo test` does not build the shell, for the reason
  the workspace manifest gives.
- **This will not be the last of them.** Every field the client *interprets*
  rather than displays is a place upstream's assumptions leak through a hard
  fork. This one was found by opening the window and looking, which is still the
  only way the UI half of this application gets checked.
