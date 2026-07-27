# 18 — Terminal: reattach, clear, restart, close

**What to build:** Terminals survive the developer's navigation. Moving away from a
terminal and coming back reattaches to the still-running session with its history
intact — a long-running process is never lost by clicking elsewhere. A broken
shell can be cleared or restarted, and closing a terminal actually reaps its
process.

**Blocked by:** 17 (Terminal: open, write, resize).

**Status:** done

- [x] Navigating away and back reattaches to the same running session
- [x] Scrollback from before detaching is still present after reattaching
- [x] A process still running while detached continues, and its output is
      available on reattach
- [x] A terminal can be cleared
- [x] A terminal can be restarted, replacing the shell in the same pane
- [x] Closing a terminal terminates and reaps its process and child processes
- [x] Closing the app or a project reaps all of its terminals
- [x] No orphaned processes, file handles or threads remain after terminals are
      closed
- [x] Tests cover detach, reattach and close-and-reap through the socket boundary

## Comments

### What ticket 17 already left here

`terminal.attach` exists — it is the only way output reaches a client at all, so
it could not wait. What it does _not_ do yet is this ticket's list:

- **`restartIfNotRunning`** is read off the payload and ignored. Attaching to a
  terminal whose shell has exited gives you the terminal as it stands.
- **`terminal.clear`, `terminal.restart` and `terminal.close`** are unimplemented,
  so `terminal.open` on an exited terminal returns it rather than quietly
  starting a second shell. Restarting is a thing the developer asks for by name.
- **Scrollback does not survive a server restart.** It is in memory only; upstream
  persists it under its logs directory. "Navigating away and back" works without
  it, "closing the app and coming back" does not.
- **`hasRunningSubprocess` is always `false`.** This ticket's "a long-running
  process is never lost by clicking elsewhere" is the acceptance that would want
  it to be true.

The reaping machinery is already there and used by shutdown —
`Terminals::shutdown` kills each shell and waits for its reaper, which closes the
pty and joins the reader and writer threads. `terminal.close` is that per
terminal, plus a `remove` on the metadata feed.

Read **ADR-0005** before touching the attach path: the rule about questions and
scrollback is what makes a reattached terminal work at all.

### What was built

Three more methods, and one flag on a method that already existed:

| Tag                                       | Answer                                               |
| ----------------------------------------- | ---------------------------------------------------- |
| `terminal.clear`                          | void                                                 |
| `terminal.restart`                        | `TerminalSessionSnapshot`, off the read loop         |
| `terminal.close`                          | void, off the read loop                              |
| `terminal.attach`'s `restartIfNotRunning` | the stream, of a terminal that now has a shell in it |

Nothing about detaching and reattaching needed building — that was the finding.
There is no `terminal.detach` on this wire: navigating away cancels the
`terminal.attach` subscription and touches nothing else, and a terminal already
outlived every attachment to it by construction. So the first three acceptance
lines are _tests_ rather than code, and they are worth more than they look: they
pin behaviour that no line of code asserts and that a plausible change — reaping
a terminal when its last subscriber leaves — would silently break.

### The one thing the ordering forced

**A `restarted` event has to be published before the new shell says a word**, and
that is not free. The client _replaces_ its buffer from the snapshot the event
carries (`applyTerminalAttachStreamEvent`), so a byte that arrived first is a
byte thrown away — and the first thing a shell says, it says immediately.

So `Session::adopt` now takes a closure that runs with the state lock held, the
new shell's handles already in it, and **the reader thread not yet started**.
That window is the only place an event can be published that is guaranteed to
precede every byte of the shell it announces. The same lock numbers the event and
stamps the snapshot, so the two carry the same sequence — which is what
`Terminals::attach` compares against when it drops what a description already
covered, and what
`a_restarted_terminal_gets_a_new_shell_in_the_same_pane` asserts directly.

### Where the locks went, and why they went there

One rule: **the registry lock is what makes a terminal's identity stable while
somebody is changing what is in it.** Two shapes fall out of it.

- **Restart holds it throughout**, including across `open_locked` on the branch
  where the terminal turns out not to exist. The terminal has to still be there
  afterwards, so the window between killing one shell and adopting the next is
  exactly what a second caller must not see — and letting go between "is it
  there?" and "then restart it" would let an `open` land in between and make the
  restart hand back somebody else's shell, un-restarted. It is the same lock
  `Terminals::open` already held across a spawn, and it is why `open_session`
  was split into a locked half.
- **Close takes the terminal out of the registry first and reaps with the lock
  released**, because it has removed the identity rather than kept it: nothing
  can reach _that_ session once it cannot be found. Reaping is the slow half and
  it must not stall every other terminal.

The cost of the first is real and is the price of the rule: while one terminal is
restarting, no other can be opened, restarted or closed. It is bounded by a kill,
two thread joins and a spawn.

The attach path re-takes **both halves** of its decision under that lock — that
the terminal it found is still the one registered, and that it is still not
running. Identity alone is not enough: two `restartIfNotRunning` attaches racing
on one exited terminal would both pass an identity check, and the second would
kill the shell the first had just started.

What is deliberately _not_ serialised is a close racing an attach that carries a
`cwd`. The attach finds nothing, opens a fresh terminal under the same id, and
the old one is reaped on its own — two well-formed terminals in sequence rather
than a leak. Upstream takes a per-thread lock and would order them; the reused
client already serialises terminal lifecycle calls per thread, so the ordering
this would buy is one the traffic does not need.

### The declared divergences

- **`deleteHistory` is not carried at all.** What this server keeps of a terminal
  is _in_ that terminal, so closing one deletes its history whichever way the
  flag was set. Upstream keeps a second copy under its logs directory and has to
  be told whether to remove that too. Not carried rather than carried and
  ignored, so that nothing later reads it as a decision somebody made.
- **A restart that cannot start a shell leaves the terminal in the `error` state
  rather than failing the call**, exactly as `terminal.open` does and for the same
  reason: `TerminalError` has no class for "no shell could be started".
- **`terminal.restart` accepts a missing size instead of refusing it**, where the
  contract makes `cols`/`rows` required on a restart and optional on an open. The
  two calls start the same shell the same way, and being pickier about a number
  the pane corrects a frame later would only be pickier, not safer. What a
  missing size means is emphatically _not_ the default, though — see below.
- **An attach restarting a terminal publishes `restarted`, where upstream
  publishes `started`.** `TerminalStartedEvent` is not in the attach stream's
  union, so a client attached to the terminal being revived could not decode
  upstream's event; `restarted` is in the union and carries the snapshot such a
  client needs.
- **A blank `terminalId` on a close is refused rather than read as an absent
  one.** Absence means "every terminal on this thread", so leniency here would
  reap terminals the client never named — the one field on this wire where being
  generous about an empty string destroys something.

### One bug this ticket found in ticket 17's code

**A call that said nothing about the size was resizing the terminal.**
`Open::read` defaulted a missing `cols`/`rows` to 120x30, and `terminal.open` on
a terminal that already existed then applied that as a resize. The contract makes
the size optional on both `terminal.open` and `terminal.attach`, and the pane
sending one of those is not always the pane that opened the terminal — so every
time a second client mounted a pane, somebody else's 200-column terminal shrank.
Upstream reads the same field as `input.cols ?? session.cols` and does not.

`Open` now carries `Option<u16>` and `Open::size(current)` resolves it: the
contract's defaults for a terminal that does not exist yet, the terminal's own
for one that does. `a_call_that_names_no_size_does_not_resize_the_terminal`
asserts it the only honest way there is — by asking the shell how big it thinks
it is — and fails with `Columns: 120` when the resolution is put back.

Restart would have inherited the same bug and now shares the fix.

### Carried forward from ticket 17, deliberately

Both were named in this ticket's opening comments and neither is in its
acceptance list. They are recorded here rather than done, and either is a small
ticket of its own.

- **Scrollback is still in memory only.** Navigating away and back keeps
  everything, because the terminal never went anywhere. Closing the app does not,
  because this ticket also requires that closing the app _reap_ every terminal —
  so what a restored scrollback would describe is a shell this server has already
  killed. Upstream restores it; nothing here asks for it.
- **`hasRunningSubprocess` is still always `false`.** A process running in a
  detached terminal is not lost by that: it keeps running and everything it
  printed is there on reattachment, which is what the acceptance line says and
  what `a_process_that_outlives_the_pane_keeps_running_and_its_output_is_kept`
  asserts against a real four-second command. What is missing is the _busy dot_
  on the tab, which upstream computes by polling the process tree with
  `powershell`/`pgrep` once per terminal per interval.

### What the tests actually assert

`socket_terminal_lifecycle.rs`, ten tests, against a real `cmd.exe`/`/bin/sh`
like ticket 17's. Three of them are worth calling out, because the obvious
version of each proves nothing.

- **"A process still running while detached continues"** runs a command that
  waits three seconds _and then_ prints its marker, and asserts the marker is not
  on screen before detaching. Without that assertion the test would pass against
  a server that lost every detached process, because the work would already have
  finished.
- **"Closing a terminal reaps its child processes"** cannot be asserted from the
  terminal, because the terminal is what is being closed — and it cannot be
  asserted from a gauge either, because every gauge here counts what the registry
  holds and a close empties the registry whether or not it killed anything. So
  the child appends a line to a file once a second, the test waits for the file
  to grow _twice_ before closing (a child that never started would make the
  assertion true for the wrong reason), and the assertion is that the file then
  stops growing. Confirmed to fail when the reaping is removed.
- **"Closing a terminal reaps its shell"** asserts the `exited` event arrives
  _before_ the `closed` one. That event is published by the reaper, after the
  process has gone and the threads reading and writing its pty have been joined,
  so its ordering is the whole promise of the call — and it is the only evidence
  available that the shell was reaped rather than merely forgotten.

A fourth is worth a line: **"a restarted terminal starts with an empty screen"
is asserted on the snapshot the `restarted` event carries, not on the one the
call answers with.** The first is taken under the lock before the reader thread
exists and cannot contain a byte of the new shell; the second is taken afterwards
and by then the shell has usually said something, so the same assertion there
would fail whenever the machine was quick.

The last of the ten repeats ticket 17's "stopping the server reaps every
terminal" in the plural and detached: three terminals across two threads, none
attached, one running a child process. A server that only reaped what had a
subscriber would leave a shell per abandoned pane.
