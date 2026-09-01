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
- [ ] Reconcile is idempotent: running it twice against the same history
      produces one copy of each block.
- [x] Parts absent locally are inserted in provider order between existing
      rows; ordinals keep reload consistent with live.
- [x] An interrupted turn whose partial last block gains a REST suffix closes
      with the extended text under the same part identity.
- [x] Focused tests pass: the interrupt-reconcile scenarios in the OpenCode
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

2026-09-01 review of the evidence above, which was in part not evidence. Three
of its assertions could not have failed, so they are gone or rewritten, and the
third box is unchecked.

**The third box is unchecked because the merge cannot run twice.**
`reconcile_interrupt` (`src/opencode.rs`) reaches its merge loop on exactly one
observation — `StopObservation::Quiet` — and settles in the same call; every
earlier snapshot returns `Pending` before the loop, and `session.rs` clears the
reconciliation ticker once a turn is `Settled`. So one stop performs one merge,
by construction, and there is no history a second merge could disagree with.
`opencode_reconcile_leaves_one_copy_of_each_block_however_often_it_reads` did
not show otherwise: its `reads >= 2` counted `session.messages` polls, which the
four-second quiet window guarantees whatever the merge does, and its replay half
addressed a settled turn, where `emit_text` returns early because `driving.turn`
is `None` and the text already matches — ticket 01's settled-turn immutability
re-proved, not this criterion. The test is deleted along with the SSE replay
branch that only it used; neither had a criterion left to serve. Making this box
real would mean changing the design so a merge can run more than once, which is
a question for whoever wants the property, not a test that can be written today.

**The first and fourth boxes now assert what the client renders.** The placement
assertions had read the _arrival-ordered_ event log, in which a row invented
seconds after the tool activity is necessarily last however the merge behaved.
On-screen order is `createdAt`: `deriveTimelineEntries` in
`apps/web/src/session-logic.ts` concatenates message rows and work rows and
sorts the lot by it. So the
assertions now compare the recovered rows' `createdAt` against the
`call-parts-1` work row's, off the persisted snapshot, and do it again after the
reload — which previously compared `assistant_texts` only and so dropped the
tool row entirely, leaving the placement claim surviving no restart at all.

The limit of what that proves is worth writing down. A recovered row's
`createdAt` is minted when the merge emits it, so it can only sort _after_ every
row already on screen: what holds is that the blocks the stream never delivered
read below the tool call they were spoken after and in provider order among
themselves, live and after a reload. A part whose provider position preceded a
row already on screen would be appended rather than sorted above it. That case
is not in the scenario and is not claimed here.

Also in this pass: the peer's `lost_suffix` history no longer keeps its own copy
of the snapshot counter, and `rows[1]` now says what it expected to find rather
than panicking on an index.

Ran: `cargo test -p laplus-server --test socket_opencode_turn --no-fail-fast --
--test-threads=1` (51 passed, 6 failed, 1 ignored) and
`cargo check -p laplus-server`.
`opencode_reconcile_lands_a_lost_suffix_in_its_own_rows_below_the_tool` passes,
as does every other interrupt-reconcile scenario named above. Of the six
failures, `failed_interrupt_reconciliation_is_reported_once_and_later_turns_
still_run` is ticket 04's and red on this branch's HEAD; two are this machine's
loopback (`AddrNotAvailable`, 10049); and the three owned-server reaping tests —
`an_owned_opencode_turn_crosses_the_socket_and_reaps_its_server`,
`stopping_busy_owned_opencode_aborts_and_reaps_its_server`,
`project_closure_reaps_its_threads_live_owned_opencode_server` — all pass when
run as their own invocation, which is the process-double interference this
ticket's previous comment already describes.

2026-09-01, on the fourth box: unchecked after all. The comment above states the
limit plainly -- a recovered row's `createdAt` is minted at merge time, so it can
only sort after every row already on screen -- and this criterion asks for
insertion _between existing rows_. Provider order below the tool call is proved,
live and across a reload, and so is the ordinal half. Interleaving above a row
already drawn is not, because the design appends. Leaving the box checked on the
strength of the half that holds would repeat the error this branch corrects in
ticket 04, so it comes off, and what is proved stays written down above.

2026-09-01, the fourth box, built rather than argued away. The comment above
diagnosed it correctly: a recovered row took the clock at merge time, so it
could only ever sort below every row on screen, and "inserted between existing
rows" was unreachable by construction. What was missing was a way for the merge
to say _where_ a row belongs, given that the driver knows the provider's part
order and the fold knows what `createdAt` every row was given, and neither knows
both.

`Change::AssistantBlockRecovered` is that seam. Reconciliation walks the
provider's part list from the end and hands each unseen text part the first row
after it that laplus already has — a message id for a text part, the provider's
own call id for a tool — and the fold turns that name into a stamp one
millisecond below the row it precedes. Nothing else moves: the change still
takes the clock for the conversation's `updatedAt`, for the event's `occurredAt`
and for the turn stamps, and the anchor is spent only where the call is what
_mints_ the part's message, so a part already on screen keeps the position it
took when it was first seen. A part with nothing drawn after it has no anchor
and is appended, which is where a trailing block belonged anyway.

`opencode_reconcile_lands_a_lost_middle_block_between_the_rows_it_was_spoken_between`
is the evidence. The peer streams the same interleaved narration and goes silent
in the second block; its `session.messages` holds a block spoken _between_ the
tool call and that second block and another spoken after everything. Both are
minted by the one merge, six seconds after every row on screen, and they land in
two different places: the settled transcript draws "Reading the tree first. " /
the two `call-parts-1` work rows / "Eleven, by the look of it. " / "The tree
holds eleven files." / "Nothing else to add.". The assertion is the timeline the
_client_ builds — `deriveTimelineEntries`'s own concatenation and stable sort by
`createdAt`, message rows before work rows — off the persisted snapshot and
again after a full file-backed reopen, and the row the block was inserted above
is checked to still carry the `createdAt` the developer was shown it at.

Three reverted mutations of `src/threads/fold.rs` turn it red and leave the
lost-suffix test green, which is what says the new row is the one being measured:
making `placed_before` answer `None` (the merge takes the clock again) and making
it answer the anchor's own stamp rather than a millisecond below it both put the
recovered block _after_ the row it was spoken before, and restamping rows on
every update drags the first block below the tool call it preceded.

**The limit, because a millisecond has one.** The transcript's only ordering key
is `createdAt`, and `crate::store` already says why: "a millisecond timestamp is
not a total order". Two rows that landed inside one millisecond are unordered
against each other today — the client's sort is stable and puts every message row
above every work row of the same millisecond whatever happened first — and a
block recovered between such a pair inherits that and cannot do better, because
there is no stamp between them to take. That is a property of the ordering key
rather than of this merge; making it go away means giving the client a dense
position to sort by, which is a contract change and a different ticket. The
scripted peer therefore pauses fifty milliseconds between the tool's result and
the block it was cut off in — the pause a provider takes before speaking again,
sent from a task rather than from inside the prompt handler, because laplus
awaits the prompt response before it reads a single event and a delay inside the
handler is not a delay in the transcript.

**The third box is still unchecked and the criterion is still untouched.**
Nothing here changes when the merge runs: `reconcile_interrupt` still reaches its
loop on `StopObservation::Quiet` alone and still settles in the same call, so one
stop performs one merge. Placement made a second merge no more possible than it
was.

The sixth box is checked because ticket 04's red test is green on this branch:
`failed_interrupt_reconciliation_is_reported_once_and_later_turns_still_run`
passes, as does every other interrupt-reconcile scenario —
`interrupting_opencode_aborts_and_keeps_partial_output_despite_duplicate_idle`,
`missing_opencode_idle_reconciles_and_late_idle_cannot_settle_queued_work`,
`interrupting_opencode_keeps_each_partial_text_part_exactly_as_it_arrived`,
`an_unanswered_abort_is_reported_and_leaves_the_session_loop_reading`,
`an_external_runaway_is_reported_once_and_remains_supervised_without_a_kill`,
`an_owned_runaway_is_killed_once_and_the_follow_up_resumes_its_session`, and the
two lost-history scenarios. The protocol suite needed no extension for the same
reason it did not last time: it covers the `session.messages` route, and
multi-part history is a driver behaviour observable only at the socket.

Ran: `cargo test -p laplus-server --test socket_opencode_turn --no-fail-fast --
--test-threads=1` (54 passed, 4 failed, 1 ignored), `cargo test -p laplus-server
--test opencode_protocol` (10 passed), `cargo test -p laplus-server --lib` (900
passed), `cargo check -p laplus-server`, and — because this changes where a row
is drawn — `socket_streaming` (18), `socket_continuity` (9), `socket_conformance`
(11), `socket_turn` (29), `socket_interrupt` (9), `socket_settling` (8),
`socket_revert` (9), `socket_session_stop` (5) and `protocol_golden` (7), all
green, plus `socket_codex_turn` (37 passed, 1 failed). None of the five failures
is this ticket's: `opencode_prompt_resolves_stored_attachments_and_omits_missing_
references`, `stopped_queued_opencode_work_survives_restart_and_retries_once_in_
order` and codex's `codex_refuses_a_stored_image_that_becomes_unreadable_before_
dispatch` all die at `harness/mod.rs:902` on a loopback connect with
`AddrNotAvailable` (10049) before any driver code runs, and reproduce in their
own invocation; `an_owned_opencode_turn_crosses_the_socket_and_reaps_its_server`
and `busy_opencode_messages_stay_separate_but_start_one_queued_turn_in_order`
both pass when run as their own invocation, which is the process-double
interference the comments above already describe.
