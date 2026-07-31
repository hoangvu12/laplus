# 06 — The developer is asked before Codex escapes its sandbox

**What to build:** Nothing writes to the developer's working tree without their
say-so. When Codex asks to do something its sandbox would not allow, the approval
panel opens — with the tool call already on screen above it, so the question has
context.

**The panel offers only the decisions the request allows.** This is the finding
that matters most here, because the contract makes it look like a free 1:1
mapping and it is not. `accept`, `acceptForSession`, `decline` and `cancel` are
Codex's own words with matching meanings — but _which of them apply is a property
of each request_, not of the contract. The sandbox-escaping write in the capture
offered only this:

```json
"availableDecisions": ["accept",
                       {"acceptWithExecpolicyAmendment": {"execpolicy_amendment": [...]}},
                       "cancel"]
```

No `decline`. No `acceptForSession`. A driver that always offers four will offer
decisions the server did not, and the developer will press a button whose answer
the agent refuses.

The two structured decisions Codex adds — an execpolicy amendment and a network
policy amendment — are **never sent** by this server. They are recognised in what
a request offers and not offered onward.

`decline` continues the turn, so the agent can try another way. `cancel`
interrupts it, so cancel means what it says.

A tool call is published **before** its approval request. That is the order the
work log wants and, as the capture confirms, the order the wire delivers — the
`item/started` arrives before the request.

`captures/03-write-approval.jsonl` becomes a fixture with an expected fold and is
replayed through the socket. Like the `claude` captures that contain a request,
it is replayed with a stop where the request is: the agent waits there, and
everything after that line in the recording happened _because of_ the answer.

**Blocked by:** 05.

**Status:** ready-for-agent

- [ ] A sandbox-escaping action opens an approval panel rather than proceeding.
- [ ] The panel offers exactly the decisions the request carried — no more, and
      not a fixed set of four.
- [ ] A request offering an execpolicy or network policy amendment is handled
      without offering that amendment onward, and without failing to render the
      request.
- [ ] The tool call is on screen before the approval request that belongs to it.
- [ ] Declining, where declining is offered, lets the turn carry on.
- [ ] Cancelling stops the turn.
- [ ] An approval left unanswered when the driver ends is closed rather than left
      open, so a restart does not leave a composer the developer cannot type
      into.
- [ ] `03-write-approval` is committed as a fixture with an expected fold, and is
      replayed with a stop where the request is.
