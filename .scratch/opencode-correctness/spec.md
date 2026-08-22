Status: ready-for-agent

# OpenCode correctness: text parts, stop reliability, process lifetime

Evidence: the research pass of 2026-08-22 (this conversation), upstream issues
sst/opencode #29894, #28958, #26635, #12860, #3815, #1151, and leak family
#20695/#14091/#22198/#29204. Local evidence: rows in `thread_messages` whose
text glues several narration blocks together, and long-lived `opencode serve`
processes on this machine (~1 GB each). Related decisions: ADR-0038
(superseded by ADR-0045), ADR-0041, ADR-0043, ADR-0056, ADR-0036.

Three defects, one effort, because they share one cause: the OpenCode driver
summarises its provider instead of reporting it. It folds every text block of a
turn into one message, it believes a stop worked when a status endpoint says
so, and it keeps a server per conversation alive forever because nothing asks
whether the conversation still needs it.

## Problem Statement

**Text renders in the wrong place.** When OpenCode narrates, calls tools, then
narrates again, the second narration is appended onto the first instead of
appearing below the tool calls. A whole turn's commentary arrives as one ever-
growing wall of text with tool activity underneath it, and reading the
transcript in order is impossible. Claude and Codex do not do this.

**Stop sometimes does not stop.** Hitting interrupt can leave the provider
running: the next message I send appears to be swallowed by the turn I just
stopped, and old output keeps streaming into the conversation. Repeated clicks
do nothing visible. This has survived several fix attempts because each fix
hardened our side of the exchange while the provider's own answer ("yes I
stopped", via a status endpoint) is the thing that lies.

**Dead servers pile up.** After using laplus for a while there are many
`opencode serve` processes holding hundreds of megabytes each, growing over
days, one per conversation I have touched, none of them doing work.

## Solution

The transcript mirrors what OpenCode actually said: one assistant message per
provider text part, positioned where the part happened, so text between tools
reads between the tools. Stop becomes an outcome we verify rather than a
request we send: after asking OpenCode to abort, laplus proves quiescence from
its messages, escalates to killing and resuming its _own_ server when the proof
never comes, and never pretends a click landed that didn't — nor tears down a
whole conversation because a status field disagreed. Conversation-owned servers
are reaped after sitting idle between turns and come back transparently on the
next prompt, exactly as the shared text-generation server already does.

## User Stories

### Text parts

1. As a developer reading a transcript, I want each narration block to appear
   where the agent said it, so that text written after a tool call reads below
   that tool call.
2. As a developer following a long turn, I want commentary split into separate
   readable bubbles at tool boundaries, so that I am not faced with one wall of
   concatenated text.
3. As a developer copying an answer, I want the copy button to copy one
   coherent statement, so that I do not paste five stitched-together fragments.
4. As a developer who interrupted a turn, I want the partial reply to keep
   whatever text had arrived before the interrupt and no text invented after
   it, so that what I read is what was said.
5. As a developer whose event stream dropped mid-turn, I want recovery to fill
   each text block into its proper place between the tools, so that a
   recovered transcript looks like a live one.
6. As a developer watching reasoning, I want reasoning to stay in the work log
   as it is today, so that thinking does not start appearing as chat bubbles.
7. As a developer reopening laplus, I want saved transcripts to show the same
   ordering I saw live, so that restart does not rewrite history.
8. As a developer expanding a settled turn's fold, I want every intermediate
   message preserved inside the fold, so that collapsing never loses words.

### Stop reliability

9. As a developer who pressed stop, I want to see a distinct "stopping" state,
   so that I know my click landed and am not tempted to click again.
10. As a developer who pressed stop, I want the turn to settle as interrupted
    only once the provider has actually gone quiet, so that stale output cannot
    keep flowing into the conversation afterwards.
11. As a developer who pressed stop and immediately typed a follow-up, I want
    my message held until the stopped turn has provably ended, so that my words
    are never fed into a turn I asked to die.
12. As a developer on a locally-owned server whose provider ignored the abort,
    I want laplus to kill and resume its own server rather than let the runaway
    continue, so that stopping always means something.
13. As a developer pointing at an external server I own, I want an unkillable
    runaway reported loudly and supervised until it settles, so that I learn
    the truth instead of watching a zombie work.
14. As a developer whose reconcile finds the provider still busy, I want the
    conversation session itself to survive, so that one bad stop does not end
    the whole conversation process.
15. As a developer who pressed stop twice or saw a duplicated idle event, I
    want settlement to happen once, so that no ghost turns or double rows
    appear.
16. As a developer whose provider emits a spurious idle before real work
    begins, I want it ignored as today, so that a turn does not settle in its
    first second.
17. As a developer diagnosing a stuck stop, I want diagnostics naming instance,
    session, phase (abort sent / verifying / escalated / settled) and the last
    observed message count, so that I can tell which link failed without
    attaching a debugger.
18. As a developer whose subagent children were running under a stopped turn,
    I want their rows ended as today, so that the delegation tree does not
    outlive the stop.

### Process lifetime

19. As a developer with many conversations, I want a conversation I have not
    used for a while to give up its server process, so that memory tracks what
    I am working on rather than everything I ever opened.
20. As a developer returning to a reaped conversation, I want the next message
    to just work, resuming the same agent context, so that reaping is
    invisible.
21. As a developer mid-turn, I want the idle reaper to never fire, so that a
    slow turn is never killed underneath me.
22. As a developer with a permission or question pending, I want the server
    kept while it waits, so that answering still reaches the agent that asked.
23. As a developer running an external server, I want laplus to never kill it,
    so that ownership stays mine (ADR-0036).
24. As a developer watching RAM, I want opencode's own per-directory instances
    (LSP servers, watchers) disposed when idle via its documented environment
    knob, so that even the live server shrinks between conversations' work.
25. As a developer reading logs after a week, I want every reap and resume
    logged with instance and session identifiers, so that process churn is
    explicable.

## Implementation Decisions

### Text parts

- **One assistant message per OpenCode text part**, keyed by the provider part
  id so identity is stable across replay, reconcile, and restart. The id is
  derived from the part id, not minted fresh per delta batch.
- The turn-level accumulation buffer goes away; deltas extend the message of
  their own part. A part that produces no text produces no message.
- Messages take their transcript position at first delta, which is what places
  post-tool text below the tool rows. No client change is required: several
  assistant messages per turn are already first-class, and the settled-turn
  fold already keeps only the terminal message visible.
- Settlement closes every open text message of the turn, not one aggregate;
  an interrupt closes each with whatever it held.
- The interrupt-reconcile merge extends the matching part-keyed message from
  REST history instead of appending to a single accumulated string; parts
  absent locally are inserted in provider order.
- Reasoning parts remain work-log entries, unchanged.
- Codex and Claude drivers are untouched: Claude already rotates per content
  block; Codex's single-message-per-turn shape is its own behaviour, not this
  spec's.

### Stop reliability

- **Quiescence is proven from messages, not believed from status.** After the
  abort request lands, the driver samples the session's message list across a
  bounded verification window and settles only when no new assistant output
  appears. Status remains a hint that can shorten the wait, never the sole
  evidence. This is the specific defence against the upstream fake-idle class.
- **Escalation ladder in owned mode:** abort → bounded verification → if the
  provider is provably still producing, terminate the owned server tree, settle
  the turn as interrupted, and let the next turn resume by durable session id
  (ADR-0041 rules apply). Killing our own child is honest where a second abort
  would be another guess.
- **External mode escalates as far as it can and says so:** no kill (the server
  is operator-owned, ADR-0036); a visible failure row names the provider as
  ignoring the stop, and supervision continues under ADR-0056 semantics until
  the provider settles.
- **A failed reconcile never tears down the session loop again.** The current
  break-and-fail path is replaced by the visible row plus continued recovery;
  ending the conversation remains the exclusive job of explicit stop-session.
- **A distinct stopping phase is published** between "abort accepted" and
  "proven quiet", rendered by the existing activity vocabulary; the composer
  keeps working and queued prompts stay queued (ADR-0045) until settlement
  completes.
- Duplicate aborts, duplicate idles, and abort racing error/exit settle
  idempotently, extending the existing settle-once guard; the ignore-idle-until-
  busy guard is unchanged.
- Diagnostics carry instance id, session id, phase, and last message count;
  never prompt or answer text.

### Process lifetime

- **Conversation-owned servers reap after an idle window between turns** — no
  active turn, nothing pending, no outstanding approval or question — mirroring
  the shared text-generation server's pattern (ADR-0043). The window is a code
  constant, not a setting.
- The reap decision is a pure function of idle time and session conditions, so
  policy is testable without spawning anything.
- Resumption after reap uses the existing durable cursor path; a failed resume
  fails visibly like any other resume refusal.
- Spawned owned servers receive OpenCode's idle-instance disposal knob in their
  environment, so LSP/watcher instances inside the process also shrink; the
  configured value is shorter than laplus's own conversation-idle window.
- External servers are never reaped or killed by this machinery.
- Every reap and resume logs instance and session ids.

## Testing Decisions

A good test here drives the wire, not the internals: it asserts on the
transcript rows, published activities, and process outcomes a client could
observe, never on how the driver bookkeeps internally.

- **Scripted HTTP/SSE peer (existing seam).** One peer covers most claims:
  - text → tool → text interleave yields three transcript positions in order,
    with two assistant messages either side of the tool row, live and after
    reload;
  - abort answered then output continuing (fake-idle): turn settles
    interrupted, post-abort parts are not folded into any later turn, and the
    owned server is escalated per ladder;
  - abort answered and genuinely quiet: settles interrupted within the
    verification window, no escalation;
  - abort unanswered/still busy in external mode: visible failure row, session
    loop survives, recovery continues;
  - spurious idle before busy ignored; duplicate idle settles once; queued
    prompt delivered only after settlement;
  - reconcile inserts missing parts in order into their own messages.
- **Fake-driver session harness (existing seam)** owns the escalation-policy
  cases that do not need HTTP: verification-window expiry, escalate-only-when-
  owned, reconcile-failure-no-teardown, stopping-phase publication.
- **Pure-function tests (ADR-0043 pattern)** for the conversation-idle decision:
  reaps when idle past the window; refuses with an active turn; refuses with
  pending approvals/questions; refuses in external mode.
- **One integration test** that a reaped conversation's next prompt resumes by
  session id against a restarted peer, and that spawn passes the idle-disposal
  environment knob.
- Prior art: the existing OpenCode protocol/socket test files and the
  text-generation idle tests.
- Per AGENTS.md, the user-visible halves (bubble placement, stopping state)
  get a ui-driver walkthrough before this is called done.

## Out of Scope

- Fixing opencode's own leaks; they are upstream's (#20695 et al.) and we
  defend by reaping and by the disposal knob.
- Changing Claude or Codex rendering, steering, or queueing semantics.
- Any new settings surface: windows and knobs are constants in this round.
- Database growth and compaction (the store's size is a separate concern).
- The stream-loss reconnect/watchdog state machine itself — that is
  `.scratch/opencode-turn-recovery/`; this spec only depends on its settle-once
  and recovery-while-busy semantics.
- Title generation and other background text generation lifetimes (already
  governed by ADR-0043).

## Further Notes

- Upstream issue numbers above were verified against research on 2026-08-22;
  the scripted peer must reproduce each misbehaviour locally rather than
  trusting upstream fix claims — installed version here is 1.18.21 and the
  minimum supported version is unchanged.
- Landing the stop-ladder should update ADR-0056 (quiescence proof supersedes
  status-trust) and add one ADR for conversation-owned idle reaping, so the
  domain docs stay the source of truth.
