# ADR-0060 — A child process outlives laplus only if the kernel lets it

Date: 2026-09-01
Status: Accepted

## Context

Every path in this crate that reaps a provider process is cooperative.
`Agent::stop`, `AppServer::stop` and `OwnedServer::stop` run because
`Server::shutdown` called them; `Server::shutdown` runs because the shell raised
`RunEvent::ExitRequested` or because `asked_to_stop` heard a signal. Each of
those paths is careful, and two of them already walk the process tree —
`terminate_tree_and_wait` exists precisely because a Windows `.cmd` shim puts a
`cmd.exe` between this server and the process that owns the protocol.

None of that runs when laplus is ended from Task Manager, terminated with
`taskkill /F`, or dies on a panic or an OOM. `kill_on_drop` does not cover those
either, and covers them least of all: it needs a tokio runtime that, in exactly
those cases, has already been taken away. The result is a class of leak with no
owner — a process laplus started, that laplus can no longer see, that no
restarted laplus will ever look for.

**It is measured, not supposed.** On 2026-09-01 this machine was holding two
`codex app-server` trees and six dev servers started three days earlier: fourteen
processes, every one with a dead parent, together holding four loopback ports.

Six of those were the more interesting half. They were `bun --cwd apps/web dev`
and `vite`, and they had never been this server's children at all — `claude`
started them through its Bash tool, and `Agent::stop` killing the `claude.exe`
handle could not have reached them on any run, successful or not. The same is
true of the stdio MCP servers a developer's `claude` configuration names.

Upstream has the identical hole open as
[`pingdotgg/t3code#5241`](https://github.com/pingdotgg/t3code/issues/5241),
where the reporter measured twenty-eight orphaned `opencode serve` processes,
ages thirty-four minutes to ten days, and 8.8 GB of resident memory. Their
diagnosis names the part that matters here: an inactivity reaper cannot see an
orphan, because it only closes sessions that a live backend still owns. That is
ADR-0057's idle reap and ADR-0058's owned reap described exactly, from the
outside. Both are good, and neither is a bound, because both need a laplus that
is still running.

## Decision

**Every provider process this server starts is joined to a Windows job object at
spawn, and that job's limits terminate its members when its last handle closes.**
The handle belongs to this process. When this process ends — by any means,
including ones it cannot run code after — the kernel closes it and
`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` does the rest.

Job membership is inherited, which is the half that decides whether this is worth
doing. Assigning the `cmd.exe` that fronts `codex.cmd` covers the `node` and
`codex.exe` beneath it. Assigning `claude.exe` covers the dev servers its Bash
tool starts. That second case is the one that was actually on the machine, and it
is not reachable by any improvement to the cooperative paths, because those
processes are grandchildren this server never had a handle to.

**This is a backstop and the cooperative paths keep doing the work.** They close
stdin, let the CLI finish its output, and collect what it said on the way down; a
job object has none of that discretion. It decides only what happens when none of
them get to run.

**`Agent::stop` now terminates the tree rather than the handle.** The job answers
"what happens when laplus ends"; a conversation ending is the earlier and far
more common event, and until this change a `claude` reaped at the end of a turn
left its `bun dev` running until laplus itself exited — which on a machine left
open for a week is not a bound. `AppServer::stop` already did this; the two now
share one implementation, which is the reason `crate::process` exists.

**Creating the job is allowed to fail, once, loudly.** A server that refused to
start because the platform would not give it a job object would supervise its
children strictly worse than one that carries on exactly as every version before
this did. The failure is reported where it happens and the `OnceLock` holds
`None` thereafter, so a machine that cannot do this does not also print a line
per child for the rest of the session.

**A dependency rather than `unsafe`.** `unsafe_code = "forbid"` is a workspace
lint whose comment says the server "shells out, spawns children and drives a
socket; none of that needs `unsafe`". Four raw Win32 calls would have needed that
lint lifted for the whole workspace to buy one module. `win32job` is a safe
wrapper over exactly these calls, and it costs the lock one crate: `windows` 0.61
and `thiserror` 1 are already in it via Tauri and `portable-pty`.

## Consequences

A provider process now dies with laplus however laplus dies, and so does
everything it started. The three-day-old trees that prompted this could not
recur, and neither could #5241's twenty-eight.

**Windows only, and the asymmetry is real rather than an oversight.** There is no
portable equivalent: a Unix process group survives the death of the process that
made it, which is why `OwnedServer`'s `process_group(0)` bounds a _signal's_
reach and not a lifetime. The nearest Unix mechanisms — `PR_SET_PDEATHSIG`, a
subreaper — are Linux-only, per-child, and a different decision with different
costs. Windows is the platform laplus installs on and the platform the evidence
came from; the Linux server (ADR-0026, `docs/running-headless.md`) keeps the
cooperative paths it has today and nothing more. This is a gap, it is written
down here, and it is not closed.

**There is a race this does not close.** A child is assigned in the statement
after `spawn`, so a process that spawns its own children in the microseconds
before that would leave them outside the job. Closing it needs `CREATE_SUSPENDED`
and a `ResumeThread`, which is `unsafe` again for a window no observed leak has
ever come through — every process in the evidence above was started by an agent
doing real work, seconds or minutes in.

**A process already in a job laplus does not own may refuse assignment.** Nested
jobs work on Windows 8 and later, so this is not the common case, but a
container or a debugger can still decline it. The assignment failure is reported
and the child runs on under the cooperative paths alone.

**What this does not do is reclaim what earlier runs leaked.** A restarted laplus
still cannot see an orphan from a previous run — it has no record that the
process was ever its. Nothing before this decision produces new ones, so the set
is closed rather than growing, and a startup reaper over persisted PIDs remains
open work rather than something this ADR claims.

**Terminals and the Cloudflare connector are deliberately not in the job.**
`Server::shutdown` names a shell outliving the server as the second thing with
this property, and it is; the connector is a third. Both have supervision
decisions of their own — ADR-0048 for the connector, which is put in its own
process group on purpose so a terminal's `^C` cannot reach it — and folding them
into this job would change those semantics as a side effect of fixing providers.
They are the obvious next users of `bound_to_this_server` and neither is covered
by this ADR.
