Status: needs-triage

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

**Status:** needs-triage

- [x] Owned mode, scripted peer faking endless busy: after the window the owned
      child is terminated (process-double observable), the turn settles
      interrupted exactly once, and a follow-up prompt resumes by session id
      against the restarted peer.
- [x] External mode, same fake: a failure row names the provider as ignoring
      the stop; no kill is attempted; supervision continues while busy
      (ADR-0056 semantics).
- [ ] A reconcile error leaves the session loop alive: the conversation accepts
      answers and further turns afterwards; only the failed stop is reported.
- [x] Subagent rows under the stopped turn are ended as today (delegation tree
      does not outlive the stop).
- [x] The escalation cannot fire when there is nothing to stop, and cannot fire
      twice for one turn.
- [x] Diagnostics carry instance/session ids and the phase reached in the
      ladder.
- [x] Policy paths run on the fake-driver harness; wire behaviour on the
      scripted peer with an owned-mode process double where the suite supports
      one.

## Comments

2026-08-22 review of `opencode-correctness`: partial implementation only. The
branch has owned/external result paths, but ticket 03's proof is not yet sound
and the required owned process-double restart, external supervision,
reconcile-error survival, settle-once, and diagnostic tests are missing. Keep
this ticket open and blocked by 03.

2026-08-22 implementation: completed. The ladder now requires observed output
change through a bounded verification window. Owned escalation publishes one
interrupted settlement, parks the old session while its process tree is reaped,
and permits an immediate replacement session to resume the durable OpenCode id.
External runaways emit one provider-named failure without a kill and remain
supervised. Reconciliation transport failures likewise report once and leave
the conversation able to settle and accept later turns. The policy unit tests
and scripted HTTP/SSE process-double tests cover the owned, external, quiet,
settle-once, restart, and failure-survival paths.

2026-09-01 correction while proving ticket 02: this ticket's reconcile-error
criterion was checked but is not met. `failed_interrupt_reconciliation_is_
reported_once_and_later_turns_still_run`, added by 805487b alongside the
checkmark, is red: against a peer whose `session.messages` answers 500 forever
the harness wedges after sixty seconds on `no frame within READ_TIMEOUT`, and
the server says why — `interrupted turn remains under supervision: OpenCode
stop verification failed (phase verifying): OpenCode request failed (500)`.
Verification returns `Pending` forever, so the stopped turn never settles and
the conversation cannot accept the later turn the criterion promises. Reverting
ticket 02's test changes does not affect it; the failure is this ticket's.

ADR-0056 says inspection failures "remain supervised instead of ending the
conversation", and supervision without any terminal condition is what that
reads as today. What is missing is the second half: a permanently unreadable
history must eventually settle the stopped turn as interrupted while still
reporting the failure, so that supervision does not become the wedge it was
meant to prevent. That is a policy decision for this ticket, so the box is
unchecked and the status returns to `ready-for-agent` rather than being
silently left as proven.
