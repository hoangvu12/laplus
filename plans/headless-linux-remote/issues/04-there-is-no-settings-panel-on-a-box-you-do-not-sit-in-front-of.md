# 04 — There is no Settings panel on a box you do not sit in front of

**What to build:** a way to turn network access on from the command line, and
the documentation for running laplus as a server.

**Status:** ready-for-human

**Depends on:** nothing. Can land alongside 01.

## Why

`RemoteAccess` reads and writes `remote-access.json` and decides the bind
address, and all of that works headless already. What does not is the only
supported way to _change_ it. The switch in Settings calls
`set_network_exposure`, a `#[tauri::command]` registered in
`laplus-shell/src/main.rs` and reachable only from the webview:

```rust
.invoke_handler(tauri::generate_handler![
    network_access_state,
    set_network_exposure,
])
```

A headless server has no webview, so today the only route is to hand-write JSON
into the preferences directory — and to know where that is, which on Linux is
`$XDG_DATA_HOME/laplus` or `$HOME/.laplus` depending on what is set
(`config.rs:535`). That is a bad first five minutes.

Upstream's equivalent is a flag: `hostFlag` in
`pingdotgg/t3code:apps/server/src/cli/config.ts`, backed by a
`T3CODE_HOST` environment variable, resolved in that order, and its whole auth
policy is settled from `isRemoteReachableHost(config.host)`. laplus already has
the predicate — `remote_access::is_remote_reachable_host` is a port of it — and
already settles the policy from the address actually bound rather than a
hostname string.

## What to build

1. **A `--network` flag on `laplus-server`**, parsed in `crate::launch` next to
   `--port` and `--ui`, with `LAPLUS_NETWORK` behind it in the same
   argument-beats-environment-beats-default order. It sets `Exposure` for this
   run.
2. **Decide, and write down, whether the flag persists.** The recommendation is
   that it does **not**: `remote-access.json` is what the Settings switch owns,
   and a flag that rewrote it would mean one `--network` run silently changed
   what the desktop application does on next launch. A flag that overrides for
   the process it was given to is the smaller claim and the one a service unit
   wants. Record it either way — this is the kind of thing that gets
   re-litigated.
3. **Say what was decided at startup.** Ticket 03 prints the exposure mode; if
   it came from a flag rather than the file, say so, because the two disagreeing
   is otherwise invisible.
4. **Documentation.** The page already exists: ticket 05 created
   `server/docs/running-headless.md` to hand over the build prerequisites, and
   said in its opening paragraph that this ticket writes the rest. What is there
   already — the C compiler for `rusqlite`'s bundled SQLite, the `claude` CLI
   and a shell as runtime requirements, where state lives on Linux, and what
   the machine calls itself — is the part 05 could answer. What is still missing
   is this ticket's: which binary to run, `--ui`, `--port`, `--network`, how to
   pair a phone, and the security posture below.

## What the documentation has to say about the posture

Not softened. ADR-0022 already has the wording and it applies more here, not
less:

> turning this on puts a process that runs `claude` as you, with your terminals
> and your filesystem behind it, on your network.

Three things specific to a headless box:

- **HTTP, not HTTPS.** Anyone on the path sees the traffic and the bearer
  token. On anything but a home network, put Tailscale or a tunnel with TLS in
  front. This is the recommended deployment, not a footnote.
- **`auth.clients` is not implemented**, so a session handed to a device cannot
  be revoked from the UI. Pairing links can be revoked; sessions already issued
  cannot. On a machine you sit in front of, ticket 73 called that "nice, not
  load-bearing". On a server, say plainly that it is a gap.
- **Nobody is watching.** Every failure here is a log line on a box with no
  window. Say where the log is.

## Acceptance criteria

- `laplus-server --network` binds `0.0.0.0`; without it, loopback.
- `LAPLUS_NETWORK` is read when the flag is absent, and the flag wins when both
  are present.
- A malformed value is a refusal with a sentence, matching `launch::port_from`.
- Whether the flag persists to `remote-access.json` is decided, implemented, and
  written down in the module or an ADR.
- Startup states the exposure mode and where it came from.
- `server/docs/running-headless.md` covers the flags and the pairing walkthrough
  alongside the prerequisites ticket 05 left there, and includes all three
  posture points.

## Out of scope

- A systemd unit. Worth having eventually; not what blocks anyone today.
- Restart-on-change semantics. ADR-0022 notes that a listener cannot be moved
  out from under its open sockets and the desktop restarts for it. A headless
  server is restarted by whatever supervises it, which is the operator's
  business.
- Any change to the Settings switch or the Tauri command. Both keep working
  unchanged for the desktop application.

## What landed

`--network` on `laplus-server`, with `LAPLUS_NETWORK` behind it in the same
argument-beats-environment order `--port` and `--ui` use. It sets `Exposure` for
this run and writes nothing.

**It does not persist, and `docs/adr/0023` is the record.** The short version is
that `laplus-server` and `laplus-shell` read the same `remote-access.json` out
of the same preferences directory, so a flag that wrote it would mean one server
run over ssh silently moving the switch a user later sees in Settings. A
process-scoped override is the smaller claim and the one a unit file wants.

The flag can also turn exposure **off** — `--network=false` over a file that
says otherwise. That is beyond what this ticket asked for and is argued in the
ADR: once the flag exists, an operator who wants one run on loopback otherwise
has to edit and restore the file, which is the manual step the flag removes.
`true`/`1`/`on`/`yes` and their opposites are all accepted, because the three
authors of this value — a person, a `systemd` unit, a `docker run -e` — each
have their own habit. Anything else is a refusal with a sentence.

`--network` is the first flag in `crate::launch` that means something bare, so
`flags_from` grew a second list. It accepts `--network=false` but **not**
`--network false`, which would make `--network --port 4773` eat its neighbour
and start a server on the default port.

`Server::bind` takes an `Option<Exposure>` the command line insisted on and
applies it through `ServerConfig::with_remote_access`, so `auth.policy` moves
with the bind address rather than being left describing a reachability the
config no longer has. The shell passes `None` and its flag set still refuses
`--network`: it restarts itself when the switch moves, so a one-run override
behind a panel that rewrites the file would be undone by the first use of the
panel.

`StartupFailure::Listen` now carries the address rather than the port. It used
to render `cannot listen on 127.0.0.1:<port>`, which was true until the exposure
switch existed and would have sent somebody who passed `--network` looking for a
conflict on an address the process never asked for.

`RemoteAccess` gained one field, `stored` — whether a `remote-access.json`
actually decided the mode. Only a `mode` this server understood counts; a
missing file, an unparseable one, and a mode nobody knows are all the default.
It exists for one line of output, and it earns that: "from remote-access.json"
printed on a fresh box sends the operator looking for a file that is not there.

### Startup says which of the four decided it

```
laplus: network access is on, from --network — this server is on your network
laplus: network access is off by default — this server answers this machine only
```

`--network`, `LAPLUS_NETWORK`, `remote-access.json`, or the default. Read back
out of the _running server's_ configuration rather than from the parsed
arguments, so the line cannot describe a posture the listener does not have.
`crate::startup` decides it; ticket 03 is the rest of that module.

### The documentation

`server/docs/running-headless.md` grew from the build prerequisites ticket 05
left there into the full page: which binary, all three flags and their
environment variables, what startup prints and how to read each state, the
pairing walkthrough, where the files live, and the posture.

All three posture points are in it, unsoftened. The third — "nobody is
watching" — turned out to have a sharper answer than expected: **`laplus-server`
writes no log file at all.** The `logs/` directory that
`observability.logsDirectoryPath` advertises to the UI is written only by the
desktop shell, and only when the shell fails to start. So the page says the log
is wherever the operator redirects it, and that it has to be **both** streams.

It also carries the finding from the previous session that cost the most time:
`claude` lives in `~/.local/bin`, which `~/.profile` adds to `PATH` for **login
shells only**. `ssh box` finds it; `ssh box 'which claude'`, `systemd` and cron
do not. `crate::provider` walks `PATH`, so a service unit reports no provider on
a machine where `claude` is installed, authenticated and working by hand, with
nothing about the failure pointing at `PATH`.

### Driven, not only tested

Against the real `apps/web/dist`, all four sources announce themselves
correctly, `--network=false` pulls a `network-accessible` file back onto
loopback for one run, `--network=flase` is refused with a sentence and exit 1,
and a `remote-access.json` that will not parse complains and falls back. 865
tests pass on Windows.

### What is left

Nothing in this ticket. The `vite.config.ts` pre-commit question is still
undecided and still not ours to change.
