# ADR-0023 — `--network` decides one run and does not touch the file

Date: 2026-07-30
Status: Accepted

## Context

[ADR-0022](0022-a-verified-credential-is-what-lets-the-listener-leave-loopback.md)
gave laplus an exposure switch and put it in Settings, which was the
right home for it: laplus is opened by double-clicking `laplus.exe`, and the
panel is what the user has. `set_network_exposure` is a `#[tauri::command]`
registered in `laplus-shell/src/main.rs` and reachable only from the webview.

Ticket 04 of the headless-Linux effort is the case that has no webview. A
`laplus-server` on a Linux box with no window has exactly one way to turn
network access on today: hand-write `remote-access.json` into the preferences
directory — and first work out where that is, which is `$XDG_DATA_HOME/laplus`
or `$HOME/.laplus` depending on what is set. That is a bad first five minutes,
and it is the whole reason the effort's phone case does not work out of the box.

So the server gets a flag. Upstream's equivalent is `hostFlag` in
`pingdotgg/t3code:apps/server/src/cli/config.ts`, backed by a `T3CODE_HOST`
environment variable and resolved in that order.

The question this record exists for is the one the ticket flagged as
re-litigable: **does the flag write the file?**

## Decision

**`--network` sets the exposure for the process it was given to. It does not
write `remote-access.json`, and the next start reads that file as though the
flag had never been passed.**

`LAPLUS_NETWORK` sits behind it in the order every other flag in
`crate::launch` uses — argument, then environment, then the default, which here
means the file. A value neither on nor off is a refusal with a sentence rather
than a fall back to either answer.

The flag can also turn exposure **off**: `--network=false` over a
`remote-access.json` that says `network-accessible`. That is not symmetry for
its own sake. Once the flag exists, an operator who wants one run on loopback —
a debugging session, a `--port 0` sanity check, a second instance beside the one
that is meant to be reachable — otherwise has to edit and restore the file,
which is exactly the manual step the flag was added to remove. `--network` bare
means on, because that is the common case and the only thing worth typing.

### Why the flag does not persist

**A flag that rewrote the file would make one server run change what the desktop
application does on next launch.** The two binaries read the same
`remote-access.json` out of the same preferences directory. Somebody who runs
`laplus-server --network` once over ssh to try something would find the switch
in Settings flipped on the next time they opened laplus on that machine, with
nothing to connect the two events. A switch that moves on its own is worse than
a switch that is hard to reach.

**A process-scoped override is the smaller claim.** The file answers "what does
this machine do"; the flag answers "what does this invocation do". Those are
genuinely different questions, and collapsing them means the file can no longer
be read as the machine's settled position.

**It is what a service unit wants.** The unit file is the record of what that
service does. If the first start wrote the mode into a file, the unit and the
file would both be claiming to be that record, and reconciling them is work
that only exists because of the write. `ExecStart=… --network` needs no
reconciling, and an operator can read the exposure of a service off the same
line that starts it.

**And it is the safer direction to be wrong in.** A flag that fails to persist
costs an operator a repeated argument. A flag that persists when nobody wanted
it leaves a machine on the network after the run that put it there has ended.

### What makes the disagreement visible

The cost of not persisting is that the flag and the file can say different
things, and on a headless box there is no panel showing which won. So the server
states it on every start, naming the source:

```
laplus: network access is on, from --network — this server is on your network
```

`crate::startup` is where that is decided and `laplus-server/src/main.rs` reads
the mode back out of the _running server's_ configuration rather than from the
parsed arguments, so the line cannot describe a posture the listener does not
have.

## Consequences

- **The Settings switch and the Tauri command are untouched.** The shell passes
  `None` to `Server::bind` and takes no such flag: `crate::launch`'s two entry
  points already differ, and a shell that accepted `--network` would offer a
  second way to set something its own panel restarts the application to change.
- **`auth.policy` moves with the override**, because `Server::bind` applies it
  through `ServerConfig::with_remote_access` rather than onto the field. A
  server bound to `0.0.0.0` that went on reporting `loopback-browser` would hide
  the section of Settings holding the only button that mints a pairing code —
  the bug that method's own note was written for.
- **The exposure a server started with cannot be changed while it runs.** ADR-0022
  already establishes that a listener cannot be moved out from under its open
  sockets, and the shell restarts for it. A headless server is restarted by
  whatever supervises it, which is the operator's business and not this crate's.

## Alternatives

**Write the file, like a `--save` that is always on.** One place to look
afterwards, and the desktop application would agree with the last server run.
Rejected above: the agreement is the problem, not the feature.

**Persist only when an explicit `--save-network` is passed.** Honest, and it is
a second flag for a thing nobody has asked for yet. The file is editable and the
Settings switch already writes it; if a headless operator turns out to want a
persisted change often enough to notice, this is the shape it should take, and
nothing here forecloses it.

**Take a host rather than a boolean, as upstream's `--host` does.** More
expressive — it would allow binding one named interface. It also needs an answer
for what a host string that is neither loopback nor wildcard means for
`RemoteAccess`, which stores a two-valued mode, and ADR-0022 already declined to
bind named interfaces for the reason upstream's own desktop does: choosing
between the six adapters a Windows machine with Hyper-V, WSL and a VPN reports
needs a UI, and the wildcard needs none. `--network` matches the switch it
stands in for, which is what makes the two describable in one sentence.
