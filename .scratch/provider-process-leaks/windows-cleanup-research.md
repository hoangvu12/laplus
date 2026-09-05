# Windows provider process ownership: external evidence

Status: ready-for-agent

Researched: 2026-09-05. Scope: primary documentation for an implementation proposal; this note does not establish which running processes belong to laplus or implement a fix.

## What the platform guarantees

- Windows does **not** terminate a process's children when that process exits. Process termination closes its kernel handles. Therefore, killing only a CLI launcher is insufficient, and a cleanup mechanism whose only trigger is application code cannot cover abrupt parent termination. [Microsoft: Terminating a Process](https://learn.microsoft.com/en-us/windows/win32/procthread/terminating-a-process)
- A Job Object groups processes. Ordinary `CreateProcess` descendants join their parent's job by default; breakaway flags can disable this inheritance, and WMI `Win32_Process.Create` is an explicit exception. `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` terminates associated processes when the **last job handle** closes, including child jobs in a nested hierarchy. Jobs also expose accounting for their processes. These guarantees support one owned job per provider process tree. They do not promise to contain services launched indirectly outside that tree. [Microsoft: Job Objects](https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects)
- Make the job handle non-inheritable: `CreateJobObjectW(NULL, NULL)` creates an unnamed job with a non-inheritable handle. A child retaining an inherited or duplicated job handle could delay the last-handle trigger. Keep ownership explicit and bounded. [Microsoft: CreateJobObjectW](https://learn.microsoft.com/en-us/windows/win32/api/jobapi2/nf-jobapi2-createjobobjectw)
- Nested jobs are supported from Windows 8. Assignment still has conditions: an already assigned process needs a compatible nested hierarchy, and the target cannot have UI limits; job members must run in the same Windows session. Check every assignment result and surface the Windows error. Do not silently claim containment after an assignment failure. [Microsoft: AssignProcessToJobObject](https://learn.microsoft.com/en-us/windows/win32/api/jobapi2/nf-jobapi2-assignprocesstojobobject)

## Spawn ordering matters

Starting a running child and then assigning its job allows it to spawn descendants first. Creating it suspended, assigning the job, then resuming prevents that execution race, but still leaves a crash window between creation and assignment. Microsoft's preferred Windows 10 mechanism is `PROC_THREAD_ATTRIBUTE_JOB_LIST`: assignment occurs during process creation, before its initial thread runs. Prefer that where the Rust spawn integration permits it. The suspended approach is a fallback with a documented remaining gap. [Microsoft engineering: creating a process in a job](https://devblogs.microsoft.com/oldnewthing/20230209-00/?p=107812) (search index supplied article text; direct page retrieval returned HTTP 403).

`CREATE_SUSPENDED` means the initial thread waits for `ResumeThread`; simply adding this creation flag to Tokio and failing to retain/resume that thread would hang startup. [Microsoft: Process Creation Flags](https://learn.microsoft.com/en-us/windows/win32/procthread/process-creation-flags)

## Rust and protocol shutdown

Tokio's `Child` continues executing after drop by default. `kill_on_drop(true)` changes direct-child behavior; `start_kill()` initiates termination without waiting, whereas `kill().await` includes waiting. `wait()` closes stdin only if it remains in `Child`; if code used `stdin.take()`, the separate writer owner must release it. Keep `kill_on_drop` as a direct-child backstop, with the job handling descendants. [Tokio: Child](https://docs.rs/tokio/latest/tokio/process/struct.Child.html)

Official OpenAI documentation distinguishes stopping a turn from unloading a thread: `turn/interrupt` cancels a turn; `thread/unsubscribe` removes a subscription and currently leaves the last-subscriber thread loaded until 30 minutes without activity. It is therefore not an immediate process cleanup primitive. `thread/backgroundTerminals/clean` exists but is experimental. The fetched page did not document a whole-app-server shutdown request or a guarantee that closing stdin always finishes cleanup. Validate EOF behavior against the **installed CLI version** rather than assuming it. [Official OpenAI documentation: App Server](https://learn.chatgpt.com/docs/app-server)

## Recommended design and validation

Idle eviction can preserve the conversation: Claude documents `--resume` by session ID/name and persistence unless `--no-session-persistence` is enabled; Codex documents `thread/resume` using the previously recorded thread ID. Persist that identifier before eviction and restore on the next turn. This preserves stored conversation state, not live shell processes, pending approvals, or arbitrary in-memory tool state; evict only genuinely idle sessions. [Claude CLI reference](https://code.claude.com/docs/en/cli-reference), [Official OpenAI documentation: App Server](https://learn.chatgpt.com/docs/app-server)

The following is an engineering proposal derived from the sources above, not a claim about current repository behavior:

1. Give each provider launch one owner containing the child, stdin writer, reader tasks, and non-inheritable kill-on-close job. Include startup/handshake failures and cancellation during startup in its ownership rules.
2. Attach the job during creation if possible; otherwise document the suspended-spawn limitation. On containment failure, terminate and wait for the new child and return a startup error.
3. At session disposal, stop accepting work, release input, and give protocol shutdown a bounded opportunity. Then terminate the job, wait for the root child, and finish or cancel stream tasks. Do not keep the job alive merely because an event-reader task retains shared session state.
4. Preserve a job-drop fallback for cancellation and abrupt server exit. Explicit shutdown should not depend on Tokio runtime teardown executing asynchronous cleanup.
5. Test a fake provider that spawns a grandchild and exits its launcher. Assert root and descendant handles signal on normal disposal, failed handshake, cancellation, and forced termination of a disposable host process. Test both native `.exe` and `.cmd` wrapper launches. Leave unrelated CLI instances alive.
6. Record provider/session ID, PID, creation time, job assignment success, stop reason, and completion. Compare live owned processes against live sessions; many Task Manager entries alone cannot distinguish valid idle sessions, independent apps, and abandoned children.

No claim here establishes that upgrading Claude or Codex alone fixes laplus-owned leaks. OS containment is a separate responsibility from provider-specific graceful shutdown and an idle-session eviction policy.
