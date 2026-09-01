Status: ready-for-human

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

**Status:** ready-for-human

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

2026-09-01 addendum on the suite's own noise. A second full `--test-threads=1`
invocation of `socket_opencode_turn`, on identical code, read 52 passed and 5
failed rather than 53 and 4: the same four, plus
`retuning_opencode_reapplies_the_same_permission_rules_with_patch` timing out
at `socket_opencode_turn.rs:1564` waiting on its external peer. It passes in
2.7s in its own invocation and passed in the other full run, so it belongs with
the load-dependent flakes rather than with this change. Recorded because two
runs of the same code disagreeing about which tests fail is the thing a future
reader most needs to know before trusting a single number from this file.

2026-09-01 code review of the rung, and the leak it opened. Four findings, one
of them a regression this ticket introduced.

**The rung settled a turn and left the runaway able to speak into the next
one.** It kills nothing — deliberately — so the event subscription stays open on
a session whose provider is still producing, and `settle` clears the driver's
per-part transcript state. Every filter on the way in reads the part's _role_
and _kind_; none of them asked which turn the part's message belonged to. So a
part of the abandoned turn's message, arriving once a follow-up prompt had
started the next turn, was minted as a fresh assistant row **inside that later
turn**. Before this branch that path wedged on `Pending` and nothing leaked;
the rung opened it. It is story 10's defect ("stale output cannot keep flowing
into the conversation afterwards") and the upstream fake-idle family, reached
through a different door than the quiescence proof guards.

Fixed by retiring provider messages at settlement. Every settlement — not just
this rung — retires the provider messages its turn heard from; a message first
heard from while no turn is driving is retired where it is heard, because
nothing arriving before the developer has asked for the next turn can belong to
it; and every later event naming a retired message is dropped at the single
point where events are dispatched, so parts, deltas, tool rows and token counts
all obey it at once. The set is a bounded ring of sixty-four ids, oldest
evicted, so a conversation of any length pays a constant price. Written as a
property of settlement rather than of abandonment because a late part after an
ordinary interrupt has exactly the same shape. What it cannot close, and says
so: a provider that mints a _wholly new_ assistant message after the settlement
and after the next prompt has gone out is indistinguishable on the wire from
the next turn answering.

`an_abandoned_runaway_speaks_into_no_later_turn` is the proof, on the scripted
peer. The peer accepts the abort and ignores it, answers 500 to every history
sample so no proof can ever arrive, is external so nothing may be killed, and
then writes two more parts of the abandoned turn's message — one part the
developer already read, one they never saw — from inside the _second_ prompt's
handler. That placement is what makes it deterministic rather than a race:
laplus awaits the prompt response before it reads a single event, so those
parts are read with the later turn already in flight. With the guard reverted
the test reads
`["hello from OpenCode", "hello from OpenCode, and still going", "a sentence
from the turn you stopped"]` against an expected `["hello from OpenCode"]` —
watched go red, then green again with the guard restored.

**`abandon_verification` recorded and decided in one call**, unlike the
`observe`/`should_escalate` split beside it, and was called before the
report-once check so the first error mutated as a side effect of a bool that
was then discarded. Split into `observe_unreadable` (record) and
`should_abandon` (decide); the call site records every unreadable sample and
asks for the verdict only where it has one to act on, so the timing is
unchanged. `observe`'s doc now says that it closes the unreadable window as
well as advancing the quiet one.

**`unreadable_since` stays a `Duration`**, against the suggestion to make it an
`Option<Instant>`. The invariant is real — it is only meaningful against
`started_at` — but every rung of this ladder is driven by an `elapsed` the
caller supplies precisely so the policy is testable without sleeping, and
`quiet_since` beside it is the same shape for the same reason. An `Instant`
here could only be compared against a real clock, which would make the terminal
rung the one rung a test has to wait out. Written down on the field so the
question is not reopened.

**ADR-0058's in-place amendment became ADR-0059.** The amendment disclosed the
no-kill choice but not that settling by abandonment also _releases queued
prompts_ — the session loop takes the next prompt exactly when `driving.turn`
is empty — which is the thing story 11 asked to be protected from. That is a
policy change with a user-visible consequence, and this repository's precedent
for one is ADR-0031's "Supersedes, in part" rather than ADR-0007's in-place
narrowing of a single sentence. So ADR-0059 now carries both rules — the rung
and the retirement — states the queued-prompt cost plainly, and names the one
case that stays open. ADR-0058 keeps a short superseded-in-part note; ADR-0056's
amendment stays in place (it narrows one sentence, which is exactly ADR-0007's
shape) and points at 0059.

**Status is `ready-for-human` rather than `done`.** The spec's testing
decisions end with "the user-visible halves (bubble placement, stopping state)
get a ui-driver walkthrough before this is called done", and no walkthrough
exists: the user decided to skip ui-driver work this session. That walkthrough
is the only thing standing between this ticket and `done`, and calling it done
without one would repeat the overclaim the 2026-09-01 correction above was
about.

Ran. `cargo check -p laplus-server`: clean. `cargo clippy -p laplus-server`: the
two remaining `too_many_arguments` warnings are `threads::queue_prompt` and
`usage::source`, neither touched here; `message_sent` no longer warns.
`cargo doc -p laplus-server --no-deps`: no `private_intra_doc_links` warning for
`Anchor` or `placed_before` (the crate's pre-existing family of them elsewhere
is unchanged). `cargo test -p laplus-server --lib`: 901 passed.
`cargo test -p laplus-server --test opencode_protocol`: 10 passed.
`cargo test -p laplus-server --test socket_opencode_turn --no-fail-fast --
--test-threads=1`: 54 passed, 5 failed, 1 ignored. Two of the five —
`opencode_prompt_resolves_stored_attachments_and_omits_missing_references` and
`stopped_queued_opencode_work_survives_restart_and_retries_once_in_order` — die
in a quarter of a second at `harness/mod.rs:902` on `AddrNotAvailable` (10049)
before any driver code runs, and do the same in their own invocation; the other
three — `an_owned_opencode_turn_crosses_the_socket_and_reaps_its_server`,
`stopping_busy_owned_opencode_aborts_and_reaps_its_server` and
`owned_opencode_uses_the_injected_generic_mcp_platform` — each pass in their own
invocation and are the owned-server process-double interference the comments
above already describe. The ordering sweep is green: `socket_streaming` (18),
`socket_continuity` (9), `socket_turn` (29), `socket_interrupt` (9),
`socket_settling` (8), `socket_revert` (9), `protocol_golden` (7).
