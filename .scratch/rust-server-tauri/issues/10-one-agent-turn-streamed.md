# 10 — One complete agent turn, streamed

**What to build:** The heart of the product. A developer types a prompt into the
real UI, watches Claude Code's reply appear token by token, and ends with a
complete, correct message in the transcript. The agent runs in the project's
directory as a long-lived subprocess driven over newline-delimited JSON.

This is the milestone the whole project has been working toward, and it is kept
whole deliberately: "agent output streams in the real UI, driven by Rust" is worth
having as one demoable thing.

**The reconciliation rule is load-bearing.** Assistant text arrives twice — once
incrementally as content-block deltas, and again as a complete buffered message.
Deltas drive live rendering; the buffered message is authoritative and replaces the
accumulation when it lands. Rendering deltas alone risks silently truncated output;
waiting only for the buffered message makes streaming pointless. From the
prototype's reducer, trimmed to the decision:

```rust
// live rendering: append deltas as they arrive
StreamEvent::ContentBlockDelta { delta, .. } => {
    if let Delta::TextDelta { text } = delta {
        self.live_text.push_str(&text);
    }
}

// reconcile: the buffered message wins
Event::Assistant(env) => {
    let text = flatten(&env.message);          // authoritative
    let from_deltas = !self.live_text.is_empty() && self.live_text == text;
    self.transcript.push(Turn { role: env.message.role, text, from_deltas });
    self.live_text.clear();
}
```

The flag recording whether the two agreed is a cheap, continuous check on that
assumption and should be observable.

Tests use a scripted fake agent executable replaying canned captures, injected
through the agent-executable-path configuration that already exists for real use —
no test-only seam is added. No test calls the real API.

**Blocked by:** 09 (Provider configuration and agent binary resolution), 05
(Project registry), 04 (First streaming subscription).

**Status:** ready-for-agent

- [ ] A prompt sent from the real UI reaches the agent and is acknowledged
      immediately
- [ ] The reply renders incrementally as it is produced, not in one jump at the end
- [ ] The final transcript text equals the buffered message, even when deltas were
      shed
- [ ] Whether deltas agreed with the buffered message is recorded and observable
- [ ] The session's model and permission mode are shown in the UI
- [ ] A completed turn reports its duration and cost
- [ ] The agent runs with the project directory as its working directory
- [ ] The subprocess is spawned once and stays alive across the turn, rather than
      per-request
- [ ] The subprocess is terminated and reaped when the session ends
- [ ] A scripted fake agent executable replays captured sessions deterministically,
      offline, at no cost
- [ ] Tests drive a full turn through the socket boundary and assert the streamed
      event sequence and the final transcript
