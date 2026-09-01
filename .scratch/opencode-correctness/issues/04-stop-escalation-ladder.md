Status: done

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

**Status:** done

- [x] Owned mode, scripted peer faking endless busy: after the window the owned
      child is terminated (process-double observable), the turn settles
      interrupted exactly once, and a follow-up prompt resumes by session id
      against the restarted peer.
- [x] External mode, same fake: a failure row names the provider as ignoring
      the stop; no kill is attempted; supervision continues while busy
      (ADR-0056 semantics).
- [x] A reconcile error leaves the session loop alive: the conversation accepts
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
unchecked and the status returns to `needs-triage` rather than being
silently left as proven.

2026-09-01 implementation of the missing rung. The policy chosen: **an unbroken
run of unreadable history snapshots lasting `STOP_ESCALATION_WINDOW` abandons
verification and settles the stopped turn as interrupted.** The reported
failure stands — it is still emitted exactly once, on the first unreadable
sample — and the conversation stays open, so the criterion's later turn runs.

Why this shape rather than another. The ladder already owns a bounded window
and a rung above it, so the terminal rung reuses both rather than adding a
timer: `StopVerification::abandon_verification` is the sibling of
`should_escalate`, keyed off the same `started_at` and the same constant, and
`observe` clears the unreadable window because reaching it at all means the
history answered. That makes the rule an _unbroken_ window like the quiet one
above it, so a single transient 500 only delays the proof rather than ending a
turn. Nothing is killed on this rung in either ownership mode: an unreadable
history is not the proof of a runaway that ADR-0058's owned reap is built on,
and ADR-0036 forbids it for an external server regardless. Ending the whole
conversation stays the exclusive job of explicit stop-session.

Interrupted is the honest verdict rather than a convenient one. Story 10's
worry — stale output flowing in after settlement — is still answered, because
settlement takes `driving.turn` and `emit_text` mints nothing without it, so
whatever the provider does next cannot reach the conversation as assistant
text. What is _not_ claimed is quiescence: the diagnostic says `phase
abandoned`, not `phase settled`, and the failure row above it says why.
Settle-once and the nothing-to-stop guard are unchanged and still come from
`settle` itself (the `settled` flag and `driving.turn.take()`); reconciliation
is only scheduled while a turn carries a stop request.

Changed: `StopVerification` gains `unreadable_since` plus
`abandon_verification`/`reported_count`, `observe` clears the unreadable
window, and `reconcile_interrupt`'s error arm falls through to a settle instead
of returning `Pending` forever (`server/crates/laplus-server/src/opencode.rs`).
ADR-0056 and ADR-0058 are amended in place with dated notes — 0056's
supervision sentence is narrowed to the conversation, and 0058 gains the rung.

Ran. `cargo check -p laplus-server`: clean. `cargo test -p laplus-server --lib
-- stop_verification`: 3 passed, including the new
`stop_verification_abandons_only_an_unbroken_unreadable_window`. `cargo test -p
laplus-server --test socket_opencode_turn -- --exact
failed_interrupt_reconciliation_is_reported_once_and_later_turns_still_run`: the
60s wedge, now passing in ~13s. `cargo test -p laplus-server --test
socket_opencode_turn -- --test-threads=1 interrupting_opencode`: both interrupt
integration tests passed. The whole binary, `--no-fail-fast --test-threads=1`:
53 passed, 4 failed, 1 ignored, and all four are the two known categories.
`opencode_prompt_resolves_stored_attachments_and_omits_missing_references` and
`stopped_queued_opencode_work_survives_restart_and_retries_once_in_order` die in
half a second at `harness/mod.rs:902` on `AddrNotAvailable` (10049) before any
driver code runs — this machine's loopback, not this change.
`an_owned_opencode_turn_crosses_the_socket_and_reaps_its_server` and
`project_closure_reaps_its_threads_live_owned_opencode_server` are the
owned-server reaping flake under load: both pass in their own invocation, the
first only when it is the sole test in the process.

Not proven: that the two loopback failures predate this branch, since they never
reach the code it touches and this machine cannot bind the address either way.
