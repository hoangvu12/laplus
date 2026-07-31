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

**Status:** ready-for-agent

- [ ] The session loop compiles and runs against a trait rather than against one
      concrete agent.
- [ ] The trait's surface is the I/O verbs and nothing else. Baselines,
      checkpoints, session epochs, settling and session-event publishing are
      outside it and shared.
- [ ] `claude` is an implementation of the trait, and the only one.
- [ ] The whole existing suite passes with no test changed, no test added, and no
      test removed.
- [ ] The `Driver` glossary entry in `server/CONTEXT.md` stops naming a single
      module as "the one this server has" and describes the trait.
