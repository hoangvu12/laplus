# 18 — Terminal: reattach, clear, restart, close

**What to build:** Terminals survive the developer's navigation. Moving away from a
terminal and coming back reattaches to the still-running session with its history
intact — a long-running process is never lost by clicking elsewhere. A broken
shell can be cleared or restarted, and closing a terminal actually reaps its
process.

**Blocked by:** 17 (Terminal: open, write, resize).

**Status:** ready-for-agent

- [ ] Navigating away and back reattaches to the same running session
- [ ] Scrollback from before detaching is still present after reattaching
- [ ] A process still running while detached continues, and its output is
      available on reattach
- [ ] A terminal can be cleared
- [ ] A terminal can be restarted, replacing the shell in the same pane
- [ ] Closing a terminal terminates and reaps its process and child processes
- [ ] Closing the app or a project reaps all of its terminals
- [ ] No orphaned processes, file handles or threads remain after terminals are
      closed
- [ ] Tests cover detach, reattach and close-and-reap through the socket boundary

## Comments

### What ticket 17 already left here

`terminal.attach` exists — it is the only way output reaches a client at all, so
it could not wait. What it does *not* do yet is this ticket's list:

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
