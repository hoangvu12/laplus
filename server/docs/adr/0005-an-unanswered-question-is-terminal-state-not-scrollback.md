# ADR-0005 — An unanswered question is terminal state, not scrollback

Date: 2026-07-27
Status: Accepted

## Context

A terminal is two halves of one machine: a pty on the server and an emulator in
the UI. lightcode is only the wire between them and reads nothing that crosses
it — except for the **scrollback**, the copy it keeps so that a client which
reconnects, or falls far enough behind that a snapshot beats catching up, has
something to be sent.

That copy is replayed _into a live emulator_. So anything in it that asks the
emulator a question would be asked again on every reattach, and answered — to a
shell that is not waiting for an answer, which means the reply lands at the
prompt as typing the developer did not do. Upstream strips those sequences from
its history for exactly this reason (`sanitizeTerminalHistoryChunk`), and
`crate::terminal::visible` strips the same set: `ESC [ … n`, `ESC [ … R`,
`ESC [ … c`, and the `OSC 10/11/12` colour queries.

Ticket 17's spike then turned up the fact that makes this more than tidiness.
**ConPTY opens by sending `ESC [ 6 n` and the shell blocks until something
answers.** Measured, not inferred: a shell driven with no reply hung
indefinitely and ran normally the instant a cursor report was written back.

The first thing a shell ever says is therefore a question, and it says it during
`terminal.open` — before anything has attached. Strip it from scrollback and
drop it from a stream nobody is subscribed to yet, and the result is a terminal
that is running, reports `running`, has a live process id, and will never print
a prompt as long as it lives. The reused UI can produce that ordering: it issues
`terminal.open` and `terminal.attach` from two different places on the same
mount and neither can be sure it went first.

Three answers were available.

| Option                                        | Cost                                                                                                             |
| --------------------------------------------- | ---------------------------------------------------------------------------------------------------------------- |
| Keep queries in scrollback                    | Every reattach injects a reply into the shell's input                                                            |
| Have the server answer the query itself       | Two answers once a real emulator attaches, and the position would be a lie about a grid the server does not keep |
| Remember the question and re-ask it on attach | One more piece of per-terminal state, and two attached clients both answer                                       |

## Decision

**A query the shell has not had answered is part of the terminal's state, and is
re-sent to whoever attaches.** It is cleared by the next write to the pty,
because that write is the answer.

Scrollback stays clean — it is still the wrong place for a question — and the
question still reaches the one thing that can answer it, whichever of
`terminal.open` and `terminal.attach` arrived first. It is sent _after_ the
snapshot rather than inside it, which is what keeps the two properties separate:
the snapshot is replayed on every re-description, the question is asked once.

The server does not become an emulator. It recognises a question by its shape
and forwards it; it never composes a reply, so it never has to hold a claim
about a screen it does not have.

## Consequences

- The rule generalises past the opening handshake for free. A full-screen
  program that asks the window for its size while the pane is detached is asking
  the same way, and is answered when the pane comes back.
- **Two clients attached to one terminal will both answer.** The reply the loser
  sends arrives as input at the prompt. Bounded — the state is cleared by the
  first write, so a third client attaching later sees nothing — and it needs two
  windows open on the same terminal to happen at all. Left as it is rather than
  arbitrated, because arbitration would mean the server deciding which emulator
  is the real one, and there is no true answer to that.
- The remembered questions are capped (`MAX_OUTSTANDING_BYTES`). Nothing else
  bounds them: a program asking in a loop with nothing attached would otherwise
  accumulate every question it ever put.
- `crate::terminal::visible` returns what it removed rather than discarding it,
  which is the only reason the module reads the stream at all. If a later ticket
  ever makes scrollback unnecessary, this is the piece that does not go with it.
