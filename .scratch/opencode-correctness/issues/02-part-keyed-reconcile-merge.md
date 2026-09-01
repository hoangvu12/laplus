Status: ready-for-agent

# 02 — Part-keyed merge for interrupt & stream-loss reconcile

**What to build:** When laplus reconciles an interrupted turn — or otherwise
merges provider history it missed — missing text blocks are inserted into
their **own** messages in provider order, extending the matching part-keyed
message rather than appending onto one accumulated string. A recovered
transcript looks like a live one: text lands between the tools where it was
said, and what was already on screen is never duplicated or rewritten.

This builds on ticket 01's per-part model: today's reconcile compares REST
history against a single accumulated string; here it addresses parts instead.

**Blocked by:** 01 — One assistant message per OpenCode text part.

**Status:** ready-for-agent

- [x] Against the scripted peer: reconcile after a lost suffix inserts the
      missing block as a new message positioned after the tool row, not glued
      to the pre-tool message.
- [x] Text already shown is left byte-identical; a divergent snapshot cannot
      retract on-screen text (existing rule preserved, now per part).
- [x] Reconcile is idempotent: running it twice against the same history
      produces one copy of each block.
- [x] Parts absent locally are inserted in provider order between existing
      rows; ordinals keep reload consistent with live.
- [x] An interrupted turn whose partial last block gains a REST suffix closes
      with the extended text under the same part identity.
- [ ] Focused tests pass: the interrupt-reconcile scenarios in the OpenCode
      socket/protocol suites, extended for multi-part histories.

## Comments

2026-08-22 review: the part-keyed merge seam is implemented, but the tracker
does not yet carry enough test evidence to check these acceptance criteria.
Keep this ticket `ready-for-agent`; the remaining work is the explicit
multi-part lost-suffix, provider-order, divergence, and idempotence coverage.

2026-09-01 implementation: the merge seam needed no change; what it needed was
evidence, and it now has it on the wire. A new scripted peer,
`ExternalOpenCode::narrating_past_a_lost_suffix`, streams the interleaved
narration only as far as the second block's first delta and then goes silent,
while its `session.messages` holds the rest of the turn the stream never
delivered — the cut-off block completed, two further blocks spoken after the
tool call, and a first block that reads differently in history than what the
developer was shown. No idle ever arrives, so the bounded interrupt
reconciliation is the only thing that can account for anything beyond the
delta.

`opencode_reconcile_lands_a_lost_suffix_in_its_own_rows_below_the_tool` closes
the first, second, fourth and fifth boxes. The settled transcript reads
"Reading the tree first. " / "The tree holds eleven files." / "Then I looked
again." / "Nothing else to add.": the first row is byte-identical to what
streamed even though history disagrees with it, the cut-off row closes under
the very message id it streamed with, and the two recovered rows appear below
the `call-parts-1` tool activity in the order history lists them. A full reload
of the same database reads the same rows in the same order.

`opencode_reconcile_leaves_one_copy_of_each_block_however_often_it_reads`
closes the third. The driver addresses the one unchanging history repeatedly
while the quiet window runs and takes from it once; a stream that comes back
afterwards and replays the recovered blocks adds no second copy.

The assertions were checked against three reverted mutations of
`src/opencode.rs` — dropping the merge loop, letting a divergent snapshot
replace on-screen text, and re-keying the merge under a different part id.
Each turns the new tests red, so none of the four rows is an accident of the
script.

Ran: `cargo test -p laplus-server --test socket_opencode_turn --
--test-threads=1` (54 passed, 4 failed, 1 ignored),
`cargo test -p laplus-server --test opencode_protocol` (10 passed), and
`cargo check -p laplus-server`. Every interrupt-reconcile scenario passes:
`interrupting_opencode_aborts_and_keeps_partial_output_despite_duplicate_idle`,
`missing_opencode_idle_reconciles_and_late_idle_cannot_settle_queued_work`,
`interrupting_opencode_keeps_each_partial_text_part_exactly_as_it_arrived`,
`an_external_runaway_is_reported_once_and_remains_supervised_without_a_kill`,
`an_owned_runaway_is_killed_once_and_the_follow_up_resumes_its_session`, and
the two added here. Nothing was extended in the protocol suite: it covers the
`session.messages` route itself, and multi-part history is a driver behaviour
observable only at the socket.

The last box stays unchecked, because one interrupt-reconcile scenario in the
same suite is red and it is not this ticket's.
`failed_interrupt_reconciliation_is_reported_once_and_later_turns_still_run`
wedges waiting for a settlement that cannot come: against a peer whose
`session.messages` answers 500 for ever, `reconcile_interrupt` reports once and
returns `Pending` from then on, which is ADR-0056's "inspection failures remain
supervised" read literally — the turn never settles. That reproduces on this
branch's HEAD with this ticket's changes reverted, so it is ticket 04's
criterion ("a reconcile error leaves the session loop alive") rather than the
part-keyed merge, and whether a permanently unreadable history should
eventually settle a stopped turn is a policy question for that ticket.

Three other failures in the same run are the machine rather than the code, and
also reproduce on HEAD: `opencode_prompt_resolves_stored_attachments_and_omits_
missing_references` and `stopped_queued_opencode_work_survives_restart_and_
retries_once_in_order` both fail instantly on a loopback connect with
`AddrNotAvailable` (10049), and `project_closure_reaps_its_threads_live_owned_
opencode_server` passes when run on its own. Orphaned `--exact
opencode_peer_child` processes from an earlier interrupted run were holding the
test binary and competing with the suite; reaping those by pid is what made the
serialized run reproducible.
