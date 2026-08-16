Status: ready-for-agent

# OpenCode turn recovery after event-stream loss

Evidence: `.scratch/opencode-upstream-audit/research.md` and ADR-0056.

## Problem Statement

Laplus normally learns that an OpenCode turn ended from the live SSE stream.
If that stream closes, loses the final idle event, or crosses a temporary
network failure, Laplus has no way to reconcile the provider's real state and
messages. A completed answer may remain stuck as working, or a recoverable turn
may fail despite OpenCode retaining its complete session.

## Solution

Supervise the event stream as a recoverable connection. On retryable loss,
publish a visible reconnecting state, query the existing OpenCode session and
messages, merge unseen output idempotently, and either settle from the observed
state or resubscribe with bounded backoff while OpenCode remains busy. The same
reconciliation runs for unfinished turns discovered after Laplus restarts.

## User Stories

1. I see `Reconnecting to OpenCode…` instead of an unexplained endless spinner.
2. A completed answer survives a lost final event and appears only once.
3. A long-running turn continues recovering until it finishes or I stop it.
4. Pending approvals and questions return once after reconnection.
5. Restarting Laplus reconciles unfinished OpenCode turns automatically.
6. If OpenCode lost the session, I keep partial work and receive a clear failure.
7. Recovery never repeats my prompt or its side effects.

## Decisions

- EOF and retryable transport errors enter turn recovery; authentication,
  incompatible protocol, malformed durable identity, and structured missing
  session errors are terminal and visible.
- Recovery never creates a new OpenCode session and never resends a prompt.
- Query session status and messages before deciding whether to fail or
  resubscribe. Idle reconciles output and settles; busy/retry reconciles output
  and resubscribes; provider error fails with its structured reason.
- Continue recovery without an arbitrary wall-clock deadline while the provider
  reports busy. Use capped exponential backoff with jitter and keep Stop active.
- Maintain a monotonic activity timestamp and use a conservative no-event
  watchdog to initiate reconciliation rather than blindly aborting.
- Merge only unseen text suffixes and deduplicate message parts, tools, requests,
  title updates, warnings, and settlement across replay and live events.
- Preserve and restore pending permission/question identities exactly once.
- On restart, unfinished OpenCode turns with valid durable cursors enter the
  same recovery path before accepting a new prompt.
- Settlement and stop are idempotent across idle, error, EOF, reconnect replay,
  abort, project close, and owned-server exit races.
- Owned and external servers share recovery semantics. Diagnostics contain
  instance/session identifiers, lifecycle phase, last-event kind/time, and
  redacted error class—not prompts, answers, credentials, or tool arguments.

## Testing

- A scripted HTTP/SSE peer covers EOF before idle with completed REST history,
  EOF while busy then reconnect, replayed deltas/tools, duplicate idle, and
  malformed or terminal failures.
- Tests cover approval/question pending across reconnect, stop during backoff,
  abort racing reconciliation, provider death, external proxy disconnect, and
  no duplicate settlement.
- Restart tests persist an unfinished turn and prove completed, busy, missing,
  and authentication-error recovery outcomes.
- The completion gate drives a real application with an injected disconnect and
  verifies status copy, recovered output, Stop, reload, and truthful settlement.

## Out of Scope

- Resending prompts or reconstructing a missing OpenCode conversation.
- A fixed timeout that declares a provider-reported busy turn failed.
- General recovery for Claude, Codex, or ACP providers.
- Changing OpenCode's own retry or model behavior.
