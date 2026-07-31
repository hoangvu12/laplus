# 09 — A Codex conversation survives a restart

**What to build:** Closing the window is not the end of the work. A developer
restarts laplus, opens a Codex conversation and carries on — and the agent
remembers what came before.

**Continuity costs one string.** A Codex conversation's continuity is its thread
id, stored where the agent session id already lives. A new process resumes from
that id alone; the rollout under `CODEX_HOME` is the agent's memory. Nothing else
is stored, and the capture proves it: a new process resumed an earlier thread
from its id and the agent quoted the earlier prompt back.

**Any failure to resume is treated as recoverable.** The driver starts a fresh
thread and publishes an activity saying the previous context could not be
resumed. A developer whose Codex history has been pruned or removed keeps a
working conversation, and — this is the half that matters — is _told_ the agent no
longer remembers, so they do not spend an hour arguing with an agent that has
quietly forgotten it.

Upstream matches a list of error phrases instead. That list does not match the
message the current codex emits — resuming a thread with no rollout answers
`"no rollout found for thread id …"`, and upstream's list has `"not found"`,
`"missing thread"`, `"no such thread"`, `"unknown thread"` and
`"does not exist"`. None of them fire. Treating **any** resume error as
recoverable is both simpler and not a list that goes stale, and nothing is hidden
by being generous because the fallback publishes the activity either way.

This ticket generalises the **agent session id** glossary entry from "`claude`'s
handle, given back as `--resume`" to "the driver's own handle" — Codex's is a
thread id.

Two captures become fixtures: `captures/05-resume.jsonl`, a new process resuming
successfully, and `captures/06-resume-missing.jsonl`, resuming an id that has no
rollout.

**Blocked by:** 04.

**Status:** ready-for-agent

- [ ] A Codex thread id is stored where the agent session id lives, and survives
      a restart.
- [ ] After a restart, sending a message to a Codex conversation resumes the
      existing thread rather than starting a new one, and the agent has the
      earlier context.
- [ ] A resume that fails for **any** reason starts a fresh thread rather than
      leaving a dead conversation.
- [ ] That fallback publishes an activity telling the developer the previous
      context could not be resumed.
- [ ] The specific message the current codex emits for a missing rollout is
      covered, and no list of error phrases is introduced.
- [ ] The **agent session id** entry in `server/CONTEXT.md` reads as the driver's
      own handle rather than as one CLI's flag.
- [ ] `05-resume` and `06-resume-missing` are committed as fixtures with expected
      folds, and both are replayed through the socket.
