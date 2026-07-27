# 17 — Terminal: open, write, resize

**What to build:** A developer opens a terminal rooted in their project directory
and uses it as a normal shell alongside the agent. Interactive programs render
correctly, and resizing the pane rewraps output the way a real terminal does.

**Blocked by:** 05 (Project registry), 04 (First streaming subscription).

**Status:** done

- [x] A terminal opens with the project directory as its working directory
- [x] Typed input reaches the shell and output streams back
- [x] Interactive full-screen programs render correctly
- [x] Colour output renders correctly
- [x] Resizing the pane resizes the underlying terminal and output rewraps
- [x] High-volume output does not stall or disconnect the socket
- [x] The shell's exit is reported in the UI
- [x] Tests drive open, write and resize through the socket boundary and assert
      the streamed output

## Comments

### What was built

Five methods, in `crate::terminal`:

| Tag                         | Answer                                               |
| --------------------------- | ---------------------------------------------------- |
| `terminal.open`             | `TerminalSessionSnapshot`, off the read loop         |
| `terminal.attach`           | a stream: scrollback, then everything the shell says |
| `terminal.write`            | void                                                 |
| `terminal.resize`           | void                                                 |
| `subscribeTerminalMetadata` | a stream: the terminal list                          |

`terminal.attach` is here rather than in ticket 18 because it is the only way
output reaches a client at all — the reused UI's pane reads its bytes from that
subscription and nothing else. What ticket 18 adds on top is `restartIfNotRunning`,
clear, restart, close and reaping, not the mechanism.

`subscribeTerminalMetadata` is here because it is in the UI's boot sequence and
because `fixtures/socket-wire/04-streaming-subscription.ndjson` captures it
whole. `socket_conformance.rs` now compares that frame **byte for byte** rather
than only comparing the envelope, which is the strictest conformance assertion in
the suite.

`subscribeTerminalEvents` is deliberately not implemented: it is a real method in
the contract with no caller anywhere in the reused UI.

### Two things the spike found that the plan did not have

Both were measured against a real `cmd.exe`, and both changed the design.

1. **ConPTY opens by asking `ESC [ 6 n` and blocks until it is answered.** The
   emulator answers, which is why this works at all in the app — and it means a
   test that only reads watches a shell that never prints a prompt. The harness
   is therefore an emulator (`harness::terminal::Pane`), which is honest rather
   than a workaround: the UI is one too.
2. **The reader never sees EOF until the pty is closed.** A shell exiting leaves
   ConPTY holding the output pipe open, so the reaper closes the console first
   and joins the reader second — which is also what puts the shell's last line of
   output in front of the exit that ended it.

The first of those collided with the rule that scrollback must not contain
questions, and the resolution is written up as **ADR-0005**.

### The declared divergences

- **`hasRunningSubprocess` is always `false`, and a terminal's label is always
  its own.** Upstream polls the process tree with `powershell`/`pgrep` per
  terminal per interval so that a tab running `vim` is titled `vim`. That is a
  poll for a caption; none of this ticket's acceptance rests on it, and an
  invented answer would be worse than the honest one. Ticket 18 is the natural
  home if it is wanted.
- **`exitSignal` is always null.** Windows has no signals and
  `portable_pty::ExitStatus` does not distinguish one, so there is nothing true
  to put there.
- **The session's own environment names the shell.** Upstream reads `SHELL` and
  `ComSpec` off the server process; lightcode reads them off the session first.
  They are the platform's conventional names for the command interpreter and the
  client already sends an environment for the shell it is opening, so this is an
  extension of the convention rather than an invention — and no capability is
  gained by it, since anyone who can open a terminal can already run any program
  by typing its name. It is what lets the suite drive one shell everywhere rather
  than whichever PowerShell the machine prefers.
- **A shell that will not start leaves a terminal in the `error` state rather
  than failing the call.** The contract's `TerminalError` union has no class for
  "no shell could be started", so a failure would tell the developer the call
  broke instead of what went wrong. The terminal exists, says `error`, and
  publishes a message naming every shell that was tried.
- **`terminal.open` on a terminal that has already exited returns it as it
  stands.** Upstream silently starts a second shell. Restarting is something the
  developer asks for by name, and the name is `terminal.restart` — ticket 18.
- **An oversized `terminal.write` is refused rather than truncated.** The
  contract caps one call at 65,536 characters and lightcode enforces it, because
  the queue in front of the shell is bounded in _slots_: sixty-four slots of
  unbounded data is not a bound. Truncating would silently drop keystrokes,
  which is the worse of the two.

### Two things here that ticket 18 also lists

Both are overlaps rather than creep, and both are named in 18's comments as
already done.

- **Reaping on shutdown.** 18 owns "closing the app or a project reaps all of its
  terminals". What is here is the narrower half of it: the ticket that starts
  processes is the ticket that must not leak them, and without it every run of
  this suite would leave a `cmd.exe` behind. `terminal.close` — reaping one
  terminal because the developer asked — is untouched.
- **Scrollback.** 18 owns "scrollback from before detaching is still present
  after reattaching". It is here because this ticket's "high-volume output does
  not stall or disconnect the socket" needs it: the way a stalled subscriber is
  survived is that the server stops trying to catch up and re-describes the
  world instead, and a re-description with nothing to describe would blank the
  pane under load. What is _not_ here is 18's version of it — scrollback is in
  memory only, so it survives navigating away and does not survive a restart.

### One change outside this module

`EventSource::superseding`, in `crate::subscriptions`. Every other subscription
on this wire delivers _replacements_, so a client seeing an update its snapshot
already contained lands on the same state — the contract says as much. A
terminal's output is appended, so an overlap is text on the screen twice and
nothing later corrects it. The overlap is not theoretical: `resynchronise` drains
and then describes, the producer is another thread, and that path is taken every
time a subscriber falls behind. The filter is fifteen lines and the terminal
attachment is the only caller.

### What the tests actually assert

`socket_terminal.rs` drives a real `cmd.exe`/`/bin/sh`. This does not break the
spec's rule about determinism, which is about the Anthropic API: it is offline,
free, and every assertion is on a marker the test asked for rather than on
timing or on the shell's own decoration.

Two are worth calling out because the obvious version of them would be worthless:

- **Resize** is asserted by asking the shell how big it thinks it is (`mode con`,
  `stty size`) and reading the answer off the same stream. Rewrapping is what the
  _program_ does with the size it is given, so asking the program is the only
  honest test that does not need an emulator.
- **Colour** is asserted as a control sequence arriving _as_ a control sequence:
  the shell is asked to print in red, and the red is in the stream while the
  escape is not in the visible text. Deliberately not a byte-for-byte comparison
  against what the program wrote — ConPTY is itself an emulator and re-spells
  what it re-renders, so byte-exactness from the _program_ was never this
  server's to promise. What is its to promise is that it forwards rather than
  reads, and the two ways it could fail — a pipe instead of a pty, which emits
  no colour at all, and a server that mangled what did arrive — are both caught.
- **"Interactive full-screen programs render correctly"** has no assertion of its
  own, and should be read as the conjunction of the two above: a program only
  redraws a screen if it believes it is on a tty of a known size (the resize
  test) and only reaches the emulator if its control bytes are forwarded (the
  colour test). Rendering itself is `xterm.js`'s, which the spec puts out of
  scope for automated tests.
