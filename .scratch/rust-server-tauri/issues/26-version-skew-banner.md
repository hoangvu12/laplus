# 26 — The UI warns about a version skew that is not one

**What to build:** a first launch with nothing wrong on the screen.

The UI shows "Client and server versions differ — Client 0.0.28 is connected to
server 0.1.0. Relaunch the server with the copied command to sync them." above
the composer, on every launch, in a fresh profile. Nothing is wrong; there is one
process and it is this one.

**Status:** done

**Found by:** ticket 23, the first time the real UI was run against this server in
a window. It has presumably been true since ticket 03 and nobody could see it.

## Why it happens

`t3code/apps/web/src/versionSkew.ts` compares its own `APP_VERSION` — the web
package's version, `0.0.28`, baked into the bundle at build time — against
`environment.serverVersion` from `server.getConfig`, by **string equality**. This
server reports `env!("CARGO_PKG_VERSION")`, which is `0.1.0` and will never be
`0.0.28` on purpose.

The banner is dismissible and the dismissal is stored per version pair, so it is
one click rather than a permanent fixture. It is still the first thing a new user
sees, and its advice — relaunch the server to sync them — cannot be followed.

## What the options look like

Not decided here; this needs a call rather than a patch.

- **Report the vendored UI's version as `serverVersion`.** Silences it exactly,
  and makes the field lie about which server this is. The field is also on
  `/.well-known/t3/environment`, where its readers are the UI's connection
  catalogue — so the lie is contained, but it is a lie.
- **Take the version from the bundle at build time.** The shell's build script
  already reads `t3code/apps/web/dist`; `package.json` beside it has the number.
  Honest about what it means ("the version this UI expects") but couples the
  server's advertised version to vendored code, and the plain server binary has
  no bundle to read it from.
- **Leave it and accept the click.** Cheapest, and defensible while lightcode is
  a fork in progress: the versions genuinely do differ.

Whichever is chosen, this is the kind of thing that will keep surfacing — the
hard fork means every field the client interprets rather than displays is a place
upstream's assumptions leak through.

## Comments

### 2026-07-27 — triage. Option 2, with a fallback for the bare server

**Take the version from the bundle at build time.** The shell's build script
already reads `t3code/apps/web/dist`; read the number from the `package.json`
beside it and report that as `serverVersion`. Where there is no bundle to read —
the plain `lightcode-server` binary — fall back to `env!("CARGO_PKG_VERSION")`,
which is the honest answer for a server that is not claiming to match any
particular UI.

Chosen over the alternatives because option 1 puts a lie in a field, and option 3
leaves the first screen a new user sees carrying advice they cannot act on. The
coupling that option 2 costs is real but already paid: the shell cannot build
without the vendored checkout, so a build script that reads a second file from
`dist/` adds no new dependency.

What this ticket should be honest about when it lands: the banner is silenced
because the two numbers are made equal, not because a real skew is now detected.
Version skew between this server and its embedded UI is not a thing that can
happen — they ship in one binary — so the check is vestigial here rather than
satisfied. Say so where the value is set, or the next person will read it as a
working comparison.

### 2026-07-27 — agent. Done, and looked at in the window

Option 2 as triaged. The version travels **with the bundle**: the shell's build
script reads `version` from `t3code/apps/web/package.json` and emits it beside
the asset table, `ui::Assets` carries it, and `Server::bind_with` applies it.
That placement is the load-bearing bit — the config and the bundle meet in
exactly one function, so a shell that shipped a UI could not report somebody
else's number even by forgetting to.

`ServerConfig::serving_ui_version` is where the honesty note asked for above
lives, and it says the thing directly: this removes a check rather than passing
one. `docs/adr/0011` records the decision at the size the next fork-leak ticket
will want.

**Verified against the real UI**, since that is where the bug was and no test in
this repo can see a banner. Two `probe-boot.mjs` runs, each in a fresh headless
Chrome profile:

| Build | `serverVersion` | Composer |
|---|---|---|
| The instance already running (pre-change) | `0.1.0` | "Client and server versions differ — Client 0.0.28 is connected to server 0.1.0. Relaunch the server with the copied command to sync them." |
| This change, on `LIGHTCODE_PORT=4774` with its own `LOCALAPPDATA` | `0.0.28` | nothing above it |

That second launch is also how a change to the shell can be looked at without
closing the lightcode already open — a running `lightcode.exe` holds a lock on
its own file, so `cargo build -p lightcode-shell` cannot even relink while one is
up. `probe-boot.mjs` now takes the URL as an argument for it.

Tests: the seam gets a unit test on each side (`ui.rs` for the bundle carrying a
version, `config.rs` for what a server reports with and without one) and a wire
test on both answers the client reads (`http_ui.rs` — `/.well-known/t3/environment`
and `server.getConfig`, since a disagreement between those two would raise the
banner just as well). The one worth knowing about is in the shell:
`the_version_reported_is_the_one_the_ui_compares_against` reads the shipped
JavaScript and requires the `APP_VERSION` beside it to be the number this server
will report — because Vite prefers an `APP_VERSION` environment variable over
`package.json`, and a `dist/` built that way would put the banner back with
nothing else noticing.
