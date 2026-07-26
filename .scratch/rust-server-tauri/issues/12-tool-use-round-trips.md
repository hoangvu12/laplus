# 12 — Tool-use round-trips

**What to build:** The developer can follow what the agent is actually doing. When
the agent invokes a tool, the transcript shows which tool and what it was given;
when the tool returns, the result appears. The developer can tell at a glance
whether a step succeeded or failed.

Thinking is distinguished from writing in the UI, so a pause while the agent
reasons does not read as a hang.

**Blocked by:** 10 (One complete agent turn, streamed).

**Status:** ready-for-agent

- [ ] A tool invocation appears in the transcript naming the tool and its input
- [ ] The corresponding result appears and is visually associated with its
      invocation
- [ ] A failed tool call is distinguishable from a successful one
- [ ] Several tool calls within one turn are rendered in order and correctly paired
- [ ] Thinking is shown as distinct from assistant output
- [ ] Large tool inputs and outputs are truncated for display without losing the
      underlying record
- [ ] A turn mixing text and tool use renders both in the order they occurred
- [ ] Scripted fake-agent captures cover single tool use, multiple tool use, and
      tool failure
- [ ] Tests assert the event sequence for each case through the socket boundary
