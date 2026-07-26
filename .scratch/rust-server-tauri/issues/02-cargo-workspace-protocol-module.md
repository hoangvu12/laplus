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

**Status:** ready-for-agent

- [ ] A Cargo workspace builds clean with no warnings
- [ ] The protocol module is a library module in the workspace, still pure — no
      I/O, no printing, no terminal code
- [ ] The spike's throwaway terminal shell is deleted rather than carried forward
- [ ] Golden-file tests fold each captured session and assert the resulting state
- [ ] A test covers an unrecognised event type degrading to a drift counter rather
      than an error
- [ ] A test covers a malformed line being counted as a parse error rather than
      panicking
- [ ] Adding a new capture file requires no test code changes
