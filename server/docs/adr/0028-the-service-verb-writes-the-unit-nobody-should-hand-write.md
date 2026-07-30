# ADR-0028 — The `service` verb writes the unit nobody should hand-write

Date: 2026-07-30
Status: Accepted

## Context

`docs/running-headless.md` carried this under _Known gaps_ for as long as the
page existed:

> **No systemd unit ships with this.** The two things such a unit has to get
> right are the explicit `PATH` above and capturing both streams.

That is a gap stated precisely enough to be closed, and left open because the
obvious way to close it — a `.service` file in the repository and a paragraph
saying where to copy it — closes it badly. A pasted unit has no version, cannot
be repaired, and is wrong the moment any path in it moves. Meanwhile the actual
answer to "how do I keep this running on my VPS" was `nohup`.

Upstream had already answered the same question, and how it answered was worth
reading before copying: `t3 service install` in
`apps/server/src/cli/service.ts`, over `apps/server/src/cloud/bootService.ts`.

## Decision

### A verb on the server, not on the launcher

`laplus-server service install|status|uninstall`. The npm launcher forwards it
without knowing it exists, which is the shape `apps/cli/src/invocation.ts`
already committed to: "this is a launcher, not a second command line in front of
the first one."

This is where the resemblance to upstream stops being structural. `t3`'s CLI and
`t3`'s server are one Node program, so `t3 auth pairing create` reaches into the
running server's own auth code in-process and a subcommand costs one
`Command.make`. Ours is a language boundary — the launcher is JavaScript,
everything real is Rust — so a verb that lived in the launcher would be a
reimplementation, and a verb in Rust is free to the launcher. `laplus-server`
grows the command line; `laplus` grows a line of help text.

`launch::invoked` is therefore a verb peeled off the front and nothing more.
Anything without a leading `service` parses exactly as it did, so every
invocation that worked before this ADR still does.

### The flags travel twice, on purpose

`service install --network --port 5000` records `--network --port 5000` in the
unit's `ExecStart` **verbatim**, and separately parses them so a typo is refused
before systemd is touched.

Rebuilding the flags from the parsed `Requested` would be the bug: `Requested`
is fully settled, so it carries this run's _defaults_ — a port nobody asked for,
and an exposure that may have come from `remote-access.json` rather than the
command line. A unit built from that would hard-code today's answer to a
question ADR-0023 deliberately leaves to the file at every start.

`--ui` is the exception and is stripped, because `npx laplus` appends its own
bundle path to every invocation and that path is the one thing in the whole
command that must not survive into the unit.

### The unit points at a copy

`npx laplus` runs from `~/.npm/_npx/<hash>`, which npm may empty at any time. A
unit naming it is a service that works until it silently does not, at a moment
nothing connects to the install. So `service::stage` copies the binary and the
bundle into `~/.laplus/service/` and the unit names the copies. A binary already
somewhere stable — a release build, a global install — is left where it is, so a
developer's rebuild is picked up by the next restart.

This is the same decision upstream made and a much smaller one. `t3` is Node, so
pinning a runtime means a real `npm install --prefix` of an exact version over
the network, native modules and all: `pinnedRuntime.ts` exists for it, and
`bootService.ts` is 433 lines. `laplus-server` is one statically linked musl
binary and a directory of files, so it is two `fs::copy` calls and no network.
ADR-0026 chose static linking to fix a `GLIBC_2.39` failure; this is the second
thing it bought.

### `PATH` comes from the shell that ran the install

The unit's `PATH` is the installing process's own, plus the system directories if
they were missing. Not a list of toolchain locations compiled into laplus:
`running-headless.md` records that being surprised three separate times in one
afternoon — `claude` in `~/.local/bin`, node under `~/.nvm`, cargo under
`~/.cargo` — and a fourth surprise is a certainty rather than a risk.

The operator ran `service install` from a terminal where `claude` works. That
fact is the most reliable thing available, and copying it is the whole method.

**Upstream's unit sets no `PATH` at all**, which is the bug this project has
already paid for; the file being copied from is not always the file to copy.

### Currency ignores `PATH`, and only `PATH`

`service status` reports a service as out of date by comparing `ExecStart` — the
binary, the bundle and the flags — and checking the binary is still there. It
does not compare the whole file, because `PATH` records _which shell installed
it_ and nothing about whether the install is stale. Comparing it would tell an
operator who installed from `bash` and asked from a login shell that their
service needs a repair, every time, and the repair would change nothing they
could see.

The second half of that check is not obvious and is the one that fires in
practice: a staged binary can be deleted to reclaim space, leaving a unit that is
textually perfect and names nothing.

### Lingering failing is a warning

`loginctl enable-linger` is what keeps the user manager alive after the last
session closes, which is the entire point on a box reached over SSH. Upstream
treats it as fatal and rolls the whole install back.

Here it is a warning, and the install stands. `enable-linger` can want a polkit
authorisation that `ssh host cmd` has no way to supply, and tearing down a
service that is installed and running because it will not survive a logout
discards the working nine tenths of what was asked for. The warning says what
did not happen and the one command that fixes it.

Every _other_ activation failure does roll back, for the reason upstream gives:
a unit file left behind by a failed install is a service `status` calls
installed and systemd never enabled, and a dangling `wants/` symlink logs
`Failed to load unit` at every boot.

## Consequences

- **A second way to be running.** `laplus-server` was a foreground process; it
  can now be a thing that starts itself. `service status` is the only way to ask
  which, and an operator debugging a port already in use has a new suspect.
- **The pairing credential moves from a terminal to a file.** There is no
  terminal for a service to print to, so the boot grant lands in
  `~/.laplus/logs/service.log`. That file is now a secret with a 24-hour reusable
  credential in it, rotated at every restart. `running-headless.md` said to treat
  the startup output that way when it was scrollback; it is easier to mean now
  that it is a file.
- **laplus writes a log file after all.** The page's _Nobody is watching_
  section is still true of the server, which writes nothing on its own — it is
  the unit that redirects both streams. The distinction survives because
  `service install` is the only thing that creates it.
- **Linux with systemd, and everything else says so.** The three verbs are a
  clear refusal on macOS, on Windows and on a Linux without a user manager.
  launchd would be a second renderer and a second set of things to be wrong
  about, and the machine most people run the desktop application on has a window
  to start it from.
- **This is checked on Windows and run on Linux.** The unit's text, the escaping,
  the staging and the currency rule are pure and covered by unit tests that need
  no systemd. What no test reaches is `systemctl` and `loginctl` themselves —
  the same honesty `running-headless.md` already keeps about what CI covers.
