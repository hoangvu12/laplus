# 16 — Concurrent sessions across projects

**What to build:** A developer runs more than one conversation at a time — across
different projects — and they stay independent. Output from one never appears in
another, each runs in its own project directory, and stopping one leaves the others
running.

**Blocked by:** 10 (One complete agent turn, streamed).

**Status:** ready-for-agent

- [ ] Two conversations in different projects can stream simultaneously
- [ ] Output, transcripts and session state never cross between conversations
- [ ] Each agent subprocess runs in its own project's working directory
- [ ] Ending or interrupting one session does not disturb the others
- [ ] Two conversations within the same project remain independent
- [ ] Subprocesses are tracked per session so none is orphaned when one ends
- [ ] Server state is per-session rather than global, with no shared mutable
      bleed-through
- [ ] Tests drive two concurrent sessions through the socket boundary and assert
      their isolation
