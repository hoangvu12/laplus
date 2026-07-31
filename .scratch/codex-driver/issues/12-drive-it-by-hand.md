# 12 — Drive it by hand, and write down what that finds

**What to build:** The done bar. Not a test — a session in the window, driven by
a person, against a real authenticated `codex`.

This project's own guidance is that **a green suite is not evidence the
application works**, and the reason it says so is that a whole afternoon's
findings once came from driving the window for a minute, none of which a passing
suite had caught. Every ticket before this one is held to what a client can
observe. This one is held to what a developer actually experiences.

The run, in order:

1. Open laplus and pick Codex in the composer.
2. Send a prompt. Watch it stream, and watch the reasoning.
3. Let it do something that needs permission. Answer the question.
4. Interrupt a turn mid-sentence, then send a correction.
5. Restart the server. Carry on with the same conversation.

**What that finds gets written down** — in this ticket's file, under a
`## Comments` heading, and as new tickets for anything that needs one. A run that
finds nothing is itself worth recording, because it is the claim being made.

**One number is worth measuring while the window is open.** Codex starts its own
MCP server per thread, so one app-server per conversation means an MCP child per
conversation: five open Codex conversations is five app-servers and five of
those. Rejecting the shared-process shape was a judgement about complexity rather
than a measurement, and this is the cheapest moment to turn it into one. If the
cost proves real, that is a ticket rather than a reopened decision.

**Blocked by:** 08, 10, 11.

**Status:** ready-for-agent

- [ ] The suite is green and every Codex capture is committed as a fixture.
- [ ] The five steps above are performed by hand in the window against a real
      authenticated `codex`.
- [ ] What the run found — including "nothing" — is written into this file under
      `## Comments`.
- [ ] Anything the run found that needs fixing is filed as its own ticket rather
      than fixed silently here.
- [ ] Five concurrent Codex conversations are opened and the resulting
      app-server and MCP child count and cost is measured and recorded.
- [ ] If that cost is a problem, it is filed as a ticket naming the
      shared-process shape as the alternative — the ADR from ticket 03 is
      amended rather than deleted.
