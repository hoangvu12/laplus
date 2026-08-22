Status: ready-for-agent

# 04 — Stop escalation ladder; reconcile failure no longer ends the conversation

**What to build:** When the quiescence proof of ticket 03 never arrives, laplus
escalates instead of guessing again. On an **owned** server, a provider that is
provably still producing after the verification window gets its server tree
terminated, the turn settles as interrupted, and the next turn resumes by
durable session id. On an **external** server — which laplus must never kill —
the runaway is reported loudly as the provider ignoring the stop, and
supervision continues under recovery semantics until it settles.

A failed interrupt reconciliation stops tearing down the whole session loop:
the visible row plus continued supervision replaces today's break-and-fail.
Ending a conversation remains the exclusive job of explicit stop-session.

Landing this updates ADR-0056 (quiescence proof supersedes status trust) and
adds one ADR for the ladder.

**Blocked by:** 03 — Stop is proven, not believed.

**Status:** ready-for-agent

- [ ] Owned mode, scripted peer faking endless busy: after the window the owned
      child is terminated (process-double observable), the turn settles
      interrupted exactly once, and a follow-up prompt resumes by session id
      against the restarted peer.
- [ ] External mode, same fake: a failure row names the provider as ignoring
      the stop; no kill is attempted; supervision continues while busy
      (ADR-0056 semantics).
- [ ] A reconcile error leaves the session loop alive: the conversation accepts
      answers and further turns afterwards; only the failed stop is reported.
- [ ] Subagent rows under the stopped turn are ended as today (delegation tree
      does not outlive the stop).
- [ ] The escalation cannot fire when there is nothing to stop, and cannot fire
      twice for one turn.
- [ ] Diagnostics carry instance/session ids and the phase reached in the
      ladder.
- [ ] Policy paths run on the fake-driver harness; wire behaviour on the
      scripted peer with an owned-mode process double where the suite supports
      one.
