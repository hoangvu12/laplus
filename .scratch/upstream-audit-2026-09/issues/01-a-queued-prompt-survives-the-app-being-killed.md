# 01 — A queued prompt survives the app being killed

Status: ready-for-human
Closed by: `Thread::restored` in `server/crates/laplus-server/src/threads/fold.rs`

**What was built:** A prompt queued behind a running turn comes back from a hard
kill offering Retry, instead of waiting forever on a session that no longer
exists.

## Why

Upstream's [`#7719`](https://github.com/pingdotgg/t3code/pull/7719) repairs
sessions left `starting`/`running` after a restart. Laplus cannot reach that
state — it stores no live session, and `Thread::restored` already moved a
`running` turn to `interrupted` — but the same question asked one step further
on found a case that was open.

`Threads::pending_retryable` is called from two places, and both of them are
code that runs. `session.rs` calls it when a session _ends_ in failure;
`orchestration.rs` calls it when a dispatch is refused. A process that is killed
runs neither. So an OpenCode prompt queued behind the turn that was in flight
came back with `retryable: false`, behind a turn now marked `interrupted`,
waiting on a session that does not exist: nothing left running could deliver it,
`claim_pending_retry` refuses a pending turn that is not `retryable`, and there
was no way to send it and no way to be rid of it.

Queued prompts are OpenCode-only here (`orchestration.rs`: `let queued =
thread.provider.driver == "opencode" && active_turn.is_some()`), so the blast
radius is one driver.

## What changed

`Thread::restored` marks a carried pending turn `retryable`, beside the two
repairs it already made and by the same argument: the restart says what the
ending would have said. The prompt itself is untouched, so Retry sends what the
developer wrote.

## Verification

`cargo test -p laplus-server --lib --no-fail-fast -- threads::` — 65 passed.

Two cases, both in `threads.rs`:

- `a_prompt_queued_when_the_app_closed_comes_back_offering_retry` — written
  first and failing on the assertion, not on a panic elsewhere.
- `a_restart_does_not_invent_a_queued_prompt` — a delivered prompt clears the
  pending turn, so there is nothing for a restart to mark and it must not invent
  one.

## Comments

Not driven in the window. The path needs an OpenCode session, a prompt queued
behind a running turn and the process killed rather than stopped; the unit tests
pin the decision, and a probe would pin the button.
