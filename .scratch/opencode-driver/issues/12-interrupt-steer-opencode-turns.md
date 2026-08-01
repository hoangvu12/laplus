# 12 — Interrupt and steer active OpenCode turns

**What to build:** A developer can abort unwanted OpenCode work or immediately
steer a busy turn. Interrupt preserves already published output, stop releases
the session, and steering retains the active turn id without changing Claude or
Codex queued-follow-up semantics.

**Blocked by:** 09 — Run the first owned OpenCode text turn.

**Status:** ready-for-agent

- [ ] Interrupt calls the OpenCode abort operation and settles the turn as
      interrupted while retaining partial transcript content
- [ ] Late abort-related events and duplicate idle signals do not overwrite the
      interrupted result or settle twice
- [ ] Stop aborts active work, closes the OpenCode scope and releases owned
      resources
- [ ] A prompt sent while OpenCode is busy is delivered immediately to the same
      upstream session under the active Laplus turn id
- [ ] A prompt sent after settlement begins a new turn normally
- [ ] Claude and Codex follow-ups remain queued as before
- [ ] Socket tests cover timing and correlation using a controllable peer
