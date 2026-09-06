# 02 — A provider process is supervised from the moment it is created

Status: ready-for-human
Closed by: `supervise_this_process` in `server/crates/laplus-server/src/process.rs`

**What was built:** laplus joins its own supervision job at startup, so every
process it starts is a member from creation rather than from whenever the
assignment gets round to running.

## Why

[`../../provider-process-leaks/implementation.md`](../../provider-process-leaks/implementation.md)
left this open in as many words: _"Windows job assignment still occurs
immediately after spawn, as in ADR-0060. This change does not close the
pre-assignment descendant/crash race."_ The window has two ways to lose a
process:

- the child starts its own child inside it — a dev server from Claude's Bash
  tool, the `codex.exe` under `codex.cmd` — and that grandchild is created
  outside the job, where nothing holds a handle to it;
- laplus dies inside it, and the whole tree is outside.

`server/crates/laplus-server/src/process.rs` had a test comment naming the same
hole: _"Keep the root blocked until both jobs are attached, so the test measures
disposal rather than the separately documented spawn race."_

## Why not `PROC_THREAD_ATTRIBUTE_JOB_LIST`

That is Microsoft's direct answer — assignment during process creation, before
the initial thread runs. Reaching it from `std::process::Command` needs
`CommandExt::raw_attribute`, which is **unstable**; confirmed against the
toolchain this tree builds with:

```
rustc 1.97.0 (2d8144b78 2026-07-07)
error[E0599]: no method named `raw_attribute` found for struct `Command`
```

Reaching it any other way means writing `CreateProcessW` by hand at every spawn
site and giving up tokio's pipes. `CREATE_SUSPENDED` is not a fallback either:
`std` hands back no thread handle, so nothing could resume the child.

## What changed

The inherited half of the same guarantee needs neither. A process created by a
process in a job joins that job as it is created, so a laplus that is itself a
member has no window left. `SUPERVISION`'s initialiser now calls
`assign_current_process`, and both binaries call `supervise_this_process()` as
their first statement.

Measured on this machine before committing to it, in a throwaway binary against
the same `win32job` version:

| Step                                                           | Result                                                         |
| -------------------------------------------------------------- | -------------------------------------------------------------- |
| `assign_current_process`                                       | OK                                                             |
| a `cmd.exe` + `ping` tree spawned afterwards, nothing assigned | 3 job members: self, child, grandchild                         |
| re-assigning that child to the same job                        | OK — the per-spawn call stays as a fallback without error spam |
| assigning it to a nested job                                   | OK                                                             |
| dropping the nested job                                        | reaps its members, parent still alive                          |

Self-assignment failure is reported and not fatal, for the reason the creation
failure already was: a CI container, a debugger or a job with the breakaway
limit set would otherwise stop laplus starting, and the per-child assignment
below it still does what every version before this did.

## What is still open

A grandchild created between `spawn` and **`SessionJob`**'s assignment joins
this process-wide job but not that conversation's, so it is reaped when laplus
exits rather than when the conversation is disposed of. Strictly smaller than
the leak above — the tree is contained either way — and still there. Closing it
needs the same `CreateProcessW` work this ticket declined.

Linux is unchanged and still cooperative-only.

## Verification

`cargo test -p laplus-server --lib --no-fail-fast -- process::supervision` — 5
passed.

The new case,
`a_tree_spawned_after_this_process_joined_is_supervised_without_being_bound`,
asserts on a tree **nothing bound**: this process, its child and its grandchild
are all job members with no assignment having run. It was written first and
failed to compile against the missing function, then passed against it.

`cargo build -p laplus-server` and `cargo build -p laplus-shell` both succeed.
