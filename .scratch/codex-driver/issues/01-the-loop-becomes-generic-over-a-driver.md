# 01 — The session loop becomes generic over a driver

**What to build:** Nothing a developer can see. The session loop stops knowing
which agent it is running: the I/O verbs move behind a **driver** trait, and
`claude` becomes an implementation of it rather than the only thing there is.

The trait covers the I/O verbs only — open a session, take the next event, send
a prompt, interrupt, answer an approval, retune, stop. Everything the loop does
_around_ those is written once and shared: baselines, checkpoints, session
epochs, settling, and publishing session events. A second driver must reuse that
logic rather than copy it, because checkpoints, epochs and settling drifting
apart between two agents is a class of bug nothing would catch.

This is a prefactor and it comes first. Make the change easy, then make the easy
change.

**The existing suite is the whole proof.** No new test asserts the trait exists —
a test of this project's own abstraction would be worth less than the suite
already passing unchanged, which is the actual claim being made. If a test has to
move, that is a signal the cut is in the wrong place.

The seam is where ADR-0001 already put it: the decoder is mirrored and shared,
the encoder belongs to the driver. `Folded` stays the shared vocabulary. Each
driver brings its own protocol module, its own accumulated state for the two
index-carrying variants, and its own encoder.

**Blocked by:** None — can start immediately.

**Status:** done

- [x] The session loop compiles and runs against a trait rather than against one
      concrete agent.
- [x] The trait's surface is the I/O verbs and nothing else. Baselines,
      checkpoints, session epochs, settling and session-event publishing are
      outside it and shared.
- [x] `claude` is an implementation of the trait, and the only one.
- [x] The whole existing suite passes with no test changed, no test added, and no
      test removed.
- [x] The `Driver` glossary entry in `server/CONTEXT.md` stops naming a single
      module as "the one this server has" and describes the trait.

**Where it landed.** `crate::session` is the loop, written once over
`session::Driver`; `crate::turn` is the `claude` implementation of it and keeps
its 34 unit tests, byte for byte, in the file they were already in. The cut was
chosen so that no test had to move: what those tests exercise — `decide`,
`Ending`, the turn's summary sentences, the sentence a dead agent earns — is the
driver's own encoder, and ADR-0001 already said an encoder belongs to a driver.

**A driver answers in `Decided`, not in `Folded`.** The glossary entry this spec
wrote is what settled it: a driver "answers with the changes a conversation is
owed". So the fold and the encoder are both behind the trait, `Folded` stays the
shared vocabulary between protocol modules rather than a type the loop handles,
and the two index-carrying variants never have to be resolved across the seam.

**The trait has nine verbs where this ticket named seven.** The two extra are
`measure` — asking how full the context window is, which the loop drives off a
flag the driver sets — and `close_input`, the "no more turns" a shutdown uses
without giving the child up. Both are I/O and nothing else could do them.
`measure` is deliberately a verb of its own rather than something the driver does
while folding: `Driver::next` is one arm of a `select!` and must be cancel-safe,
so a write it awaited could be dropped along with the event that occasioned it.

**Two loose ends, both named in the code and both later tickets'.**
`Start::settings` is still a `ClaudeSettings`, which is ticket 02's registry.
And `Permission`, `Drift` and `TokenUsage` cross this seam while still living in
`crate::protocol`, which is the `claude` wire format's module — they are the
shared vocabulary in practice (`worklog` already reads a `Permission` to draw the
approval panel), but `Permission` is a deserialized `can_use_tool` body and will
not survive Codex's per-request decisions unchanged. Splitting `protocol` into a
shared vocabulary and a module per driver belongs to the ticket that writes the
second protocol.

**The suite.** 1157 pass. Two fail on this Linux box before this change as well
as after — `watcher::tests::a_file_written_outside_…` and
`a_call_that_names_no_size_does_not_resize_the_terminal` — checked against a
worktree at `HEAD`, and both are environmental rather than this change's. The
window was not driven, and this is the one kind of change where that is the right
call: nothing a developer can see moved, which is the ticket's own first line.
