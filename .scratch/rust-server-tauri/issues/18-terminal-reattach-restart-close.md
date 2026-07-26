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
