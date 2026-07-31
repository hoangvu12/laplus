# 05 — A command the agent ran reaches the work log

**What to build:** A developer watching a Codex turn sees each command the agent
ran appear in the work log with its command line and its exit status, so they can
see what was done to their machine.

Codex's `commandExecution` carries the command, the working directory, a process
id, a status and an exit code. The work log's existing vocabulary is what those
become — this ticket adds no new activity kind that `claude` does not already
produce.

**One decision this ticket owes the work log.** A Codex `agentMessage` carries a
**phase**: `commentary` before tool use, `final_answer` after. Whether commentary
is published as a message or as an activity is not something the protocol
decides, and it is a decision a reader of the transcript will feel. Make it, and
write down why.

`captures/02-command-execution.jsonl` becomes a fixture with an expected fold and
is replayed through the socket. It is also the capture that establishes something
worth carrying into ticket 06: under `approvalPolicy: untrusted` with a read-only
sandbox, `ls` ran with **no approval at all**. What triggers a request is the
sandbox escape, not the policy name.

**Blocked by:** 04.

**Status:** ready-for-human

- [x] A command the agent runs appears in the work log with its command line.
- [x] Its exit status is recorded when the command finishes, paired to the row
      that announced it.
- [x] The `commentary` / `final_answer` phase distinction is handled by a
      deliberate choice, and the choice is written down where the next reader
      finds it.
- [x] A command that fails is distinguishable from one that succeeded.
- [x] `02-command-execution` is committed as a fixture with an expected fold.
- [x] The same capture is replayed through the socket, and the assertions are on
      the work log the UI receives.
- [x] The capture's own finding holds: a command that does not escape the sandbox
      runs without an approval request even under `untrusted`.
