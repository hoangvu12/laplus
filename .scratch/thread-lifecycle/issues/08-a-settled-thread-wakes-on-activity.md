# 08 — A settled thread wakes itself on real activity

**What to build:** a conversation the developer settled comes back on its own the
moment there is real work in it again — so settling can never hide something that
needs them.

Without this, ticket 07 ships a known hole. A settled thread whose agent later
asks for permission would sit outside the inbox while blocked on a decision only
the developer can make. That is the exact failure the settle invariants refuse to
create at settle time, and it must not be reachable a minute later either.

**Real activity resets any override.** Not only does a settled thread wake — a
thread the developer pinned _active_ returns to neutral, so it can settle again
once the burst of work goes stale. Both directions go through the same event, with
the server-decided neutral reason. The contract lets a client send only the user
reason, so this reset cannot be forged by a client.

Three trigger points, each guarded so nothing is emitted unless an override is
actually set:

| When                                                  | What wakes            |
| ----------------------------------------------------- | --------------------- |
| a turn is requested                                   | any override is reset |
| the session becomes starting or running               | any override is reset |
| an approval or a question is appended to the work log | any override is reset |

The second and third are guarded narrowly on purpose. A session status arriving
_after_ the fact — ready, stopped, error — must not fight the developer's explicit
settle, so only a session coming alive counts. And only a request that blocks on
the developer wakes a thread; ordinary work-log rows do not, or a settled
conversation would wake on every tool call.

Two of the three are internal paths this server already owns — the session-set
change and the activity-append change — so this is a guarded emission beside an
event that already fires, not a new mechanism.

**Note:** the turn-requested site is also touched by ticket 09, which clears a
snooze there. The two tickets are independent — neither gates the other — but they
edit the same place, so whichever lands second should expect to merge.

**Blocked by:** 07 — Settling and unsettling (there is no override to reset until
settle exists).

**Status:** ready-for-agent

- [ ] Requesting a turn on a settled thread returns it to neutral, and the thread
      reappears in the inbox.
- [ ] Requesting a turn on a thread pinned active returns it to neutral.
- [ ] Requesting a turn on a thread with no override emits no reset.
- [ ] A session becoming starting or running on a settled thread returns it to
      neutral.
- [ ] A session status of ready, stopped or error does **not** reset an override.
- [ ] An approval request appended to the work log of a settled thread returns it
      to neutral.
- [ ] A question appended to the work log of a settled thread returns it to
      neutral.
- [ ] An ordinary work-log row — a tool call, a thinking row — does **not** reset
      an override.
- [ ] Every reset carries the server-decided neutral reason, never the user one.
- [ ] A reset is published on the thread's own feed and reaches the project list.
- [ ] Where a reset accompanies another event, both are published, and the
      dispatch answers with the last of their sequences — the same shape a turn
      request already uses when it commits several events.
- [ ] The reset survives a restart.
