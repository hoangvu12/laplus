# 13 — Permission prompts

**What to build:** When the agent asks permission to do something, the developer
is asked, and their answer decides what happens. Approving lets the agent proceed.
Rejecting returns control to the agent cleanly so the session continues — a
declined action must never kill the conversation.

This is the developer's control surface over what runs against their code, so both
paths need to be solid, not just the happy one.

**Blocked by:** 12 (Tool-use round-trips).

**Status:** ready-for-agent

- [ ] A permission request from the agent surfaces in the UI describing what is
      being asked
- [ ] Approving allows the action to proceed and the turn to continue
- [ ] Rejecting returns control to the agent and the session remains usable
- [ ] The conversation can continue with further turns after a rejection
- [ ] A permission request left unanswered does not deadlock the session or leak
      the subprocess
- [ ] The permission mode in effect is visible, so the developer knows how much
      latitude the agent has
- [ ] Scripted fake-agent captures cover approval, rejection, and an unanswered
      request
- [ ] Tests drive all three paths through the socket boundary
