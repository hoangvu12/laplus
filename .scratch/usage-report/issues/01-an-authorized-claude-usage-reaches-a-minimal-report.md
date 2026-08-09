# 01 — An authorized Claude usage reaches a minimal report

**What to build:** A developer with read access can open a responsive Usage
report and see the processed-token total from one valid Claude Code assistant
record on that environment. This is the narrow end-to-end tracer bullet through
the versioned contract, authorized deferred server read, client query, sidebar
navigation, and rendered web route; it establishes the seams later tickets
deepen without attempting the complete upstream dashboard.

**Blocked by:** None — can start immediately.

**Status:** ready-for-human

- [x] The shared contract declares the upstream-compatible Usage summary vocabulary, contract version, typed read failure, and `server.getUsageSummary` RPC.
- [x] A real server reads one valid Claude assistant usage record from a temporary configured/default provider home and returns its day, provider, model, token categories, record count, and session count in a summary bucket.
- [x] The transcript scan is deferred work and does not hold the WebSocket read loop while touching disk.
- [x] The RPC dispatch authorization seam receives the authenticated grant and requires `orchestration:read` for this method without changing authorization behavior for unrelated methods.
- [x] A real grant carrying `orchestration:read` receives the summary, while a real grant without it receives the contract's scope-required refusal and no transcript-derived data.
- [x] Raw prompt, response, and tool content from the fixture do not occur anywhere in the response payload.
- [x] A Usage entry opens a root web route in browsers and in the desktop shell's shared web bundle.
- [x] The minimal route renders the selected inclusive range and the fixture's processed-token total at desktop and narrow widths.
- [x] Back has a useful direct-route fallback, and sidebar navigation closes the mobile sidebar consistently with existing footer navigation.
- [x] Contract tests, a real WebSocket integration test, focused route tests, targeted checks, a fresh web build, and a UI-driver walkthrough prove the tracer bullet.
- [x] The contract-parity ledger is re-derived for the newly implemented method without copying a method count into other prose.
