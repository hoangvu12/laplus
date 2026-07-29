# 04 — There is no Settings panel on a box you do not sit in front of

**What to build:** a way to turn network access on from the command line, and
the documentation for running laplus as a server.

**Status:** ready-for-agent

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
