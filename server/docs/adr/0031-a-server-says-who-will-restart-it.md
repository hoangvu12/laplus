# ADR-0031 — A server says who will restart it

Date: 2026-07-30
Status: Accepted
Supersedes, in part: [ADR-0020](0020-this-fork-publishes-an-installer-and-does-not-yet-update-itself.md)

## Context

ADR-0020 deferred self-update rather than refusing it, and said exactly what would
lift the deferral: _"unblocked the moment a release exists to update from."_

Two feeds now exist. `hoangvu12/laplus` has tags `v0.1.0` and `v0.1.1` and a
GitHub Release carrying the NSIS installer, which is the Tauri updater's feed;
`bundle.createUpdaterArtifacts` is on and `TAURI_SIGNING_PRIVATE_KEY` gates the
release job. And `laplus` is published on npm at `0.1.1-rc.15`, which is the feed
the CLI path would use. The precondition is met twice over.

### ADR-0020 also describes a shape the contract does not have

It says `capabilities.serverSelfUpdate` "stays false". The field is not a boolean.
`packages/contracts/src/environment.ts:33` types it as one of three literals —
`boot-service`, `respawn`, `desktop-managed` — held under an optional key, where
**absent** means "must be relaunched manually". So the true state today is absent,
and "false" is not a value it can hold.

### What the word actually decides

Not "can this server update itself" but **who restarts it**, because the server
cannot restart the process that is dying. Upstream resolves it at
`apps/server/src/cloud/selfUpdate.ts:92`:

```
desktop app supervising it            → "desktop-managed"
marked systemd user unit              → "boot-service"
published npm CLI on linux/darwin     → "respawn"
anything else                         → null
```

Under `desktop-managed` the server does not self-update at all — the capability's
own doc calls it _"one of the RPC self-update methods above, **or** `desktop-managed`"_
— and the client offers an application update instead of calling
`server.updateServer`. Which makes `server.updateServer` a **headless** method.

The three map onto laplus's three shapes without adaptation: the Tauri window is
`desktop-managed`, `npx laplus` is `respawn` against a feed that is already an npm
package, and ADR-0028's systemd unit is `boot-service`.

The distinction between the last two is not cosmetic. Respawning while a
supervisor is watching produces two servers — the detached replacement plus
whatever systemd restarts — which is why upstream refuses to infer `boot-service`
from `INVOCATION_ID` alone and demands a marker written into the unit it wrote.

## Decision

**laplus reports a self-update path, and which one is a fact about how it was
launched.**

- **`desktop-managed` when the shell spawned the server.** A branch on how the
  process was started; no method, no feed logic, and correct for the shape laplus
  is usually run in.
- **`respawn` for `npx laplus`.** Install the exact version, verify, spawn a
  detached replacement, hand off, exit.
- **`boot-service` for the systemd unit.** ADR-0028's unit gains a marker
  identifying itself as the supervisor before the server will trust it. Last,
  because it is the only one that has to change a file already installed on a
  machine.
- **Absent otherwise**, which is a first-class answer rather than a hole: upstream
  returns it for dev checkouts, foreground Windows runs, and any unmarked systemd
  unit.
- **Install and verify before anything restarts.** Upstream's property, and worth
  restating as ours: _"a failed install leaves the running server untouched."_

Three tickets, in that order, under the server-admin parity effort.

### What survives of ADR-0020

Everything except the deferral. This fork still publishes a Windows installer on a
tag and nothing else; no nightly cron; `tauri.conf.json` is still the version and
the tag is checked against it; the Rust suite still runs in the release job.
ADR-0020's reasoning about the two signatures — that an updater needs minisign and
only a paid certificate quiets SmartScreen — is unchanged and is why shipping this
costs no certificate.

## Consequences

- **Advertising the capability is a promise.** A client that reads a path offers an
  update; ADR-0020 already noted that the first tag was a commitment, and this
  makes the commitment reachable from inside the application.

- **This is the one feature whose bug leaves no working server.** On the aarch64
  box that means a server reached through a tunnel that is now down, with no
  window to fall back to. Verify-before-restart is the mitigation and it is not
  optional.

- **The two feeds disagree on version.** `v0.1.1` on GitHub Releases,
  `0.1.1-rc.15` on npm. "What version is available?" has two answers today, and
  the `respawn` ticket hits that first. Not decided here.

- **The capability moves with the code that backs it**, per **Capability** in
  `CONTEXT.md`: reporting `respawn` before the respawn path exists offers an
  update that cannot happen.

- **Do not port by directory.** Upstream's implementation lives in
  `apps/server/src/cloud/selfUpdate.ts` — the directory whose _surface_ `94da6be`
  removed from this fork. The file itself is npm, systemd and spawn, with nothing
  cloud, relay or Clerk in it, but the neighbourhood is one this repository
  deliberately left behind.
