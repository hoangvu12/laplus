# 26 — The UI warns about a version skew that is not one

**What to build:** a first launch with nothing wrong on the screen.

The UI shows "Client and server versions differ — Client 0.0.28 is connected to
server 0.1.0. Relaunch the server with the copied command to sync them." above
the composer, on every launch, in a fresh profile. Nothing is wrong; there is one
process and it is this one.

**Status:** needs-triage

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
