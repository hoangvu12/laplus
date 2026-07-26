# 02 — Cargo workspace and protocol module lifted from the spike

**What to build:** A real Cargo workspace for the server, with the spike's pure
protocol module moved into it as a proper library module and covered by
golden-file tests. The spike's throwaway terminal shell is discarded, not ported.

This is the prefactor — make the change easy, then make the easy change. The
protocol module is already pure, already correct, and already has real captures
sitting beside it from the spike. Lifting it now establishes the drift-detector
seam before anything depends on it, so every later agent ticket builds on tested
ground.

The module's parse-and-fold behaviour is the thing under test: feed captured
newline-delimited JSON, assert the folded session state. Assertions stay at the
level of observable outcome — parsed events and resulting transcript state — not
internal bookkeeping.

**Blocked by:** None — can start immediately.

**Status:** done

- [x] A Cargo workspace builds clean with no warnings
- [x] The protocol module is a library module in the workspace, still pure — no
      I/O, no printing, no terminal code
- [x] The spike's throwaway terminal shell is deleted rather than carried forward
- [x] Golden-file tests fold each captured session and assert the resulting state
- [x] A test covers an unrecognised event type degrading to a drift counter rather
      than an error
- [x] A test covers a malformed line being counted as a parse error rather than
      panicking
- [x] Adding a new capture file requires no test code changes

## Comments

**2026-07-26 — agent.** Done. `cargo build --workspace`, `cargo test --workspace`
and `cargo clippy --workspace --all-targets` are all clean, no warnings.

Layout:

- Workspace root `Cargo.toml` at the repo root; one member,
  `crates/lightcode-server`, which ticket 03's socket endpoint builds into.
  `unsafe_code = "forbid"` is set workspace-wide.
- `crates/lightcode-server/src/protocol.rs` — lifted from the spike. Still pure.
  Two additions: `Serialize` on the state types (skipping `counts`, so the
  goldens pin outcomes rather than the reducer's tallies) and `fold_line`, which
  is the "one raw line, malformed ones included" entry point the stdio pump will
  want. The reducer itself is unchanged.
- `fixtures/claude-cli/` — three captures with `.expected.json` siblings, plus a
  README on provenance and how to add one. Sibling of `fixtures/socket-wire/`
  from ticket 01: that pins the protocol the UI speaks, this pins the one the
  agent speaks.
- `spike-claude-protocol/` — `src/`, `Cargo.toml` and `Cargo.lock` deleted. The
  README stays: the spec's "Reference artifacts" section names it, and it holds
  the CLI flags and the account of what the spike did not prove. It now says
  where the code went.

How the drift detector works: the test walks `fixtures/claude-cli/*.ndjson`,
folds each through a fresh `SessionState`, and compares the serialized state to
its golden sibling. Adding a capture is dropping the file in and running
`UPDATE_GOLDEN=1 cargo test -p lightcode-server` — no test code changes. All
three failure modes were exercised by hand and confirmed to fail loudly: a
capture with no golden, a tampered golden, and a golden whose capture was
deleted.

Two tests beyond the goldens are worth naming, because they guard things a
capture set can lose quietly:

- `every_golden_file_has_a_capture` — a stale expectation left behind by a
  deleted capture is silent coverage loss.
- `the_captures_cover_the_wire_format` — asserts the capture set still reaches
  streamed turns, delta reconciliation, drift counts and parse errors between
  them, so it cannot narrow to one boring session and still pass.

Findings worth carrying forward:

- **`Delta::Unknown` deliberately does not count as drift, and that is correct.**
  Every other catch-all arm increments `unknown_events`, so the asymmetry looks
  like an oversight against the spec's "every enum has a catch-all arm that
  increments a drift counter". It is not. `Delta` recognises only `text_delta`,
  and real turns emit `thinking_delta`, `signature_delta` and `input_json_delta`
  constantly — counting them would bury an actual format change under routine
  noise. Same reasoning for `system` subtypes other than `init`. Both are
  recorded in the fixtures README so the next person does not "fix" them.
- The two recorded captures are one turn each and end cleanly. Nothing here
  covers tool-use round-trips, permission prompts, interrupts, compaction or an
  abrupt child exit — those need purpose-built scripts, and tickets 10–15 are
  where they arrive. `03-synthetic-drift.ndjson` is hand-written precisely
  because no recording contains degradation: a healthy CLI never emits it.
- Golden files embed absolute `cwd` values from the machine that recorded them
  (`C:\Users\ADMIN\...`). Left verbatim — they are evidence, and redacting would
  weaken the detector's fidelity — but a capture recorded elsewhere will differ
  there, so re-recording an existing capture means regenerating its golden.
