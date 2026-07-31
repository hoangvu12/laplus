# 10 — A model or access mode changed between turns applies to the next turn

**What to build:** The picker tells the truth on Codex. A developer who changes
the model or the access mode between turns gets that change on the next turn, and
a turn already in flight keeps the rules it started under.

This is **retune** — telling an agent already serving a conversation to change
what it _is_ — and it is the third of the things this server says to a running
agent, distinct from an interrupt (which ends a turn) and a permission decision
(which answers a question). Neither of those changes what the agent is.

The pairing rule is the one the `claude` driver already follows and the loop from
ticket 01 already owns: a mode belongs to one turn, so it travels with the prompt
rather than as a signal. Two turns queued behind a running one, with the picker
moved between them, must each be answered under the mode they were requested
under.

Ticket 07's table is what a runtime mode becomes. The reviewer is sent explicitly
here too, for the same reason it is sent on resume: omitting it leaves whatever
the thread last used.

**Blocked by:** 07.

**Status:** ready-for-agent

- [ ] Changing the model between turns applies to the next Codex turn, without
      replacing the session or losing the conversation.
- [ ] Changing the access mode between turns applies to the next Codex turn, as
      the approval policy and sandbox ticket 07's table names.
- [ ] A turn already running when the picker moves finishes under the mode and
      model it started with.
- [ ] Two turns queued behind a running one, with the picker moved between them,
      are each served under the mode they were requested under.
- [ ] Every session event published for one turn reports the same mode and model.
- [ ] A retune that does not land is reported to the developer rather than
      silently dropped.
