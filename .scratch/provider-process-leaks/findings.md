# Claude and Codex process accumulation

Status: ready-for-agent
Date: 2026-09-05
Scope: initial research and read-only diagnosis. Implemented fixes and subsequent
verification are recorded in [implementation.md](implementation.md).

## Finding

The current installation retains many provider processes while laplus remains
open. This is a different lifecycle case from the abrupt-exit protection added
in ADR-0060. The strongest explanation is the explicit exclusion of Claude and
Codex from the conversation idle reaper. Process age alone does not establish
whether any particular conversation is idle, so do not terminate processes on
the basis of this inventory alone.

## Live evidence

Ran the read-only inventory in `audit-processes.ps1` on this machine. It records
PID, creation time, working set and ancestry without emitting command lines or
conversation content. Parent creation times are checked to reject reused PIDs.

```powershell
& ./.scratch/provider-process-leaks/audit-processes.ps1
```

All inventoried processes traced to installed `laplus.exe`, PID 3424:

| Process                  | Count | Summed working set (MiB) | Oldest (hours) |
| ------------------------ | ----: | -----------------------: | -------------: |
| claude.exe               |    20 |                   4716.2 |           83.8 |
| codex.exe                |     9 |                   1107.1 |           84.0 |
| codex-code-mode-host.exe |     9 |                    265.2 |           84.0 |

These are point-in-time measurements. Summed working set can double-count shared
pages and differs from Task Manager's default memory column; it is not a private
memory total. A separate command-line classification, without printing arguments,
identified the Claude processes as stream-json and Codex as app-server. Counts
include currently active sessions, including this investigation.

The installed executable reports version 0.1.13, modified September 2 at 00:28
local time. The running laplus began September 2 at 06:04. Repository HEAD is
`e9237a6`; release 0.1.13 includes commit `325063a`, the process-wide Job Object
change. This is consistent with the fix being installed; binary source identity
and actual Job Object membership were not independently verified.

## Source evidence

- [`session.rs:632`](../../server/crates/laplus-server/src/session.rs#L632):
  `reaps_when_idle` returns false for both Claude and Codex. Only a locally owned
  OpenCode server can enter the existing 90-second idle cleanup policy.
- [`session.rs:1085`](../../server/crates/laplus-server/src/session.rs#L1085):
  the timer is armed only for eligible providers with no active turn, queued
  prompt or outstanding request. This is the appropriate existing policy seam.
- [`process.rs:123`](../../server/crates/laplus-server/src/process.rs#L123):
  `SUPERVISION` owns one process-wide job. Its lifetime is laplus's lifetime,
  so it cannot release an abandoned conversation while laplus stays open.
- [`agent.rs:486`](../../server/crates/laplus-server/src/agent.rs#L486) and
  [`codex.rs:1636`](../../server/crates/laplus-server/src/codex.rs#L1636):
  graceful root exit returns from `stop` before tree termination. Both tree
  helpers in `process.rs` also return when the root has already exited. Any
  surviving descendant has no per-session kernel cleanup here. This is a
  source-level weakness, not a reproduced explanation for the live root count.
- [`turn.rs:198`](../../server/crates/laplus-server/src/turn.rs#L198) and
  [`codex.rs:213`](../../server/crates/laplus-server/src/codex.rs#L213): both
  drivers already consume durable resume cursors. Claude resumes by session ID;
  Codex resumes its provider thread. The existing idle ending detaches the live
  session without deleting the conversation (`session.rs:1176` onward).

## Recommended implementation

1. Generalize idle eligibility to locally owned, resumable provider sessions.
   Start with the existing 90-second policy, then tune from measured startup
   cost. Require a valid continuation cursor before automatic eviction. Keep
   external OpenCode endpoints excluded. Preserve holds for active turns,
   queued prompts, permissions/questions and any provider background work that
   remains active after turn settlement; investigate that last condition before
   shipping blanket process-tree eviction.
2. Resume the existing provider session on the next prompt. Preserve the
   conversation, model/options, provider identity and continuation cursor.
   Keep the existing wind-down/epoch protection so a prompt arriving during
   cleanup attaches to the replacement session without being lost.
3. Give each provider session an owned Windows Job Object, alongside the
   process-wide crash backstop as appropriate. Close/terminate that session's
   job after a bounded graceful shutdown, including when the root already
   exited or startup failed. This releases remaining descendants without
   relying on a still-live root PID or killing another conversation's processes.
   Audit handle inheritance and assignment failures. Close the spawn/assignment
   race with a supported safe wrapper or process-creation job attribute; do not
   merely add CREATE_SUSPENDED without a reliable resume mechanism.
4. Add diagnostics mapping provider PID and creation time to session ownership,
   lifecycle state, idle deadline and job-assignment result. Avoid storing
   prompts, raw command lines or credentials. A PID alone is not identity.

Do not treat a name-based `taskkill /IM` sweep as the fix. It cannot distinguish
active sessions or independently launched agent instances. An orderly laplus
restart after active work finishes is a temporary reset, not an idle policy.

## Verification required before claiming a fix

The audit above is observational, not a deterministic reproduction of idle
eviction. No shutdown experiment was performed on the user's live sessions.
Implementation should first add failing integration tests at the real session
loop, following `tests/opencode_conversation_idle.rs`:

- Complete one Claude/Codex turn, cross an injected idle deadline, and assert
  that the provider process and its helper descendant actually exit.
- Send a follow-up and assert the same provider session/thread ID is resumed,
  with its history retained.
- Verify active turns, queued prompts, pending approvals/questions and relevant
  background work prevent cleanup.
- Send a prompt during wind-down and verify exactly one delivery and no stale
  ending applied to the replacement session.
- Have a fake provider spawn a helper, then exit gracefully itself; assert
  that the helper exits while another provider session stays alive.
- Kill an isolated laplus test process forcibly and verify descendants exit;
  exercise startup/initialization failure and job-assignment failure separately.

Run the smallest relevant Rust test binaries with `--no-fail-fast`. Then drive
an isolated app through two real resumed conversations using the UI driver,
and compare process inventory before, after idle cleanup and after shutdown.
Stop test servers when finished. No application tests were run for this
research-only change.

External primary-source research: [Windows cleanup research](windows-cleanup-research.md).
