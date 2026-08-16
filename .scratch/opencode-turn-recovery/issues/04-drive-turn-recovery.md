# 04 — Drive OpenCode turn recovery in the application

**What to build:** Complete the reliability project by breaking the event wire
in a running rebuilt application and observing honest recovery end to end.

**Blocked by:** 03 — Recover unfinished OpenCode turns after restart.

**Status:** ready-for-agent

- [ ] Run all focused protocol, session, socket, contract, client, and UI tests.
- [ ] Drive disconnect-before-idle and verify reconnecting copy, missing-suffix
      recovery, no duplicate output, and one completion.
- [ ] Drive a still-busy reconnect, pending approval/question, and Stop.
- [ ] Restart during a turn and verify completed and missing-session outcomes.
- [ ] Inspect diagnostics for useful lifecycle evidence and absence of secrets or
      user content.
- [ ] Record the walkthrough under `## Comments` and stop all test processes.
