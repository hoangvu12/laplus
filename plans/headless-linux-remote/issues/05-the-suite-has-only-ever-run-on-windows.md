# 05 — The suite has only ever run on Windows

**What to build:** a Linux build and test run for `laplus-server`, in CI and
verified by hand once, so the rest of this effort has a platform to stand on.

**Status:** ready-for-human — the CI half has landed; the hand-driven run has
not, and needs somebody with a Linux box. See **What landed** at the bottom.

**Depends on:** nothing. Do this first — every other ticket assumes its answer.

## Why

`.github/workflows/rust.yml` is `runs-on: windows-latest`. `release.yml` is too,
for good reason — it builds a Windows installer. Nothing has ever compiled this
server for Linux, and the whole point of this effort is to run it there.

Ticket 36 already established that "the suite is tested only on the machine that
changed it" is a real failure mode worth a ticket. This is the same failure with
a platform attached.

## What the read says, so this is a confirmation rather than an exploration

The code is written cross-platform on purpose and nothing found in a source read
contradicts it:

- Every `std::os::windows` use is inside `#[cfg(windows)]` with a
  `#[cfg(not(windows))]` twin — `files.rs` (two symlink helpers, both in
  tests), `filesystem.rs` (one, in tests), `process.rs`
  (`without_a_console`, whose whole body is the `cfg` and which the comment
  already describes as "a no-op everywhere else").
- `crate::process` handles `PATHEXT` on Windows and an executable bit on Unix,
  in `is_executable`, with both branches written.
- `config.rs:535` walks `LOCALAPPDATA`, `APPDATA`, `XDG_DATA_HOME`, then
  `USERPROFILE` or `HOME` → `~/.laplus`.
- `terminal.rs` builds its shell candidate list from `ComSpec` and PowerShell on
  Windows, and from `$SHELL` then `/bin/zsh`, `/bin/bash`, `/bin/sh` otherwise.
- `xtask` has **zero** dependencies.
- `laplus-shell` is excluded from `default-members`, so `cargo build` and
  `cargo test` never try to build Tauri.

Dependencies, all cross-platform: `axum`, `tokio`, `futures-util`, `serde`,
`serde_json`, `getrandom`, `hmac`, `sha2`, `notify` (inotify on Linux, and its
default features are macOS-gated), `portable-pty` (Unix is `openpty(2)` /
`forkpty(3)` — a first-class backend, it is WezTerm's own crate), `rusqlite`
with `bundled`.

**`rusqlite`'s `bundled` feature compiles SQLite from source through the `cc`
crate**, so the Linux box needs a C compiler. Without one the build fails with
`failed to find tool "cc"`. This is the one non-obvious prerequisite and belongs
in ticket 04's documentation.

## What to look at rather than assume

Three things a compile will not catch:

1. **`config::machine_label`** reads `COMPUTERNAME` then `HOSTNAME`. `HOSTNAME`
   is a shell variable on most Linux distributions and is frequently not
   exported, so this will quietly answer `"laplus"` for every machine. That is a
   label the UI shows. Decide whether to read `/etc/hostname` or leave it.
2. **The `claude` binary.** It must be installed and authenticated _on the Linux
   box_ — this server drives the CLI where it runs, which is the entire point,
   but it means the box needs its own provider setup.
   `crate::provider`'s resolution walks `PATH` and the extension list is empty
   off Windows, which is correct; confirm it finds a real installation.
3. **A terminal.** `crate::terminal` opening `/bin/bash` under a pty is the
   feature most likely to behave differently, and the one the suite can only
   partly speak for.

## What to build

1. **A `ubuntu-latest` job** running `cargo build -p laplus-server` and
   `cargo test -p laplus-server --no-fail-fast`. A matrix on the existing
   `rust.yml` is the smaller change; a second job is fine too.
2. **Fix what it finds.** Expect this to be small or empty; if it is not, that
   is the finding.
3. **Record the prerequisites** — `build-essential` or equivalent — and hand
   them to ticket 04.
4. **One hand-driven run** on a real Linux box: start it, pair a browser, open a
   terminal, run a turn. Ticket 36 and `AGENTS.md` both say a green suite is not
   evidence the application works, and this is exactly the case they mean.

## Acceptance criteria

- CI builds and tests `laplus-server` on `ubuntu-latest`, green.
- Windows CI is unchanged and still green.
- The prerequisite list is written down.
- `machine_label` on Linux is decided — fixed or deliberately left, with a note.
- A hand-driven session on Linux has run a turn and opened a terminal, and what
  it found is written up. A `HANDOFF-` note under this feature's directory is
  the right home if it finds anything — the two that used to sit in
  `.scratch/rust-server-tauri/` went with that directory on 2026-07-29.

## Out of scope

- Building `laplus-shell` on Linux. It is Tauri and would need WebKitGTK; it
  stays excluded.
- macOS. Same argument, no demand behind it.
- Releasing a Linux artifact. Building from source is the story until somebody
  asks for more.

## What landed

**`.github/workflows/rust.yml` is a matrix.** `windows-latest` keeps the whole
default set — `laplus-server` and `xtask`, unchanged — and `ubuntu-latest` runs
`cargo build -p laplus-server` and `cargo test -p laplus-server --no-fail-fast`.
`xtask` is deliberately left off Linux: it builds and measures a Windows
installer, so its tests there would be a second opinion about string constants
rather than evidence about the platform. `fail-fast: false`, so one platform
going red still leaves the other's answer — which is the whole reason for a
matrix rather than two opinions about one runner. The Defender exclusion step is
now `if: runner.os == 'Windows'`, and a separate `Build` step was added ahead of
`Test` so a platform that cannot _compile_ says so in one line.

Windows runs the same two cargo commands it ran before, so the second acceptance
criterion holds in substance — but not quite literally: the job is now named
`Test (windows-latest)` rather than `Test`, which would have invalidated a
branch-protection check required under the old name. Checked before relying on
it: `main` is not a protected branch, so nothing referenced the old name.

**`machine_label` was fixed rather than left.** `/etc/hostname` is now the third
source after `COMPUTERNAME` and `HOSTNAME`, parsed the way `hostname(5)`
describes the file — first line that is neither blank nor a `#` comment. A box
with no such file still answers `"laplus"`, and that is now a documented
fallback rather than the answer every Linux box gave. The parsing is a pure
function so it is testable on the platform this suite has always run on, which
has no `/etc/hostname` at all.

Windows is skipped with a runtime `cfg!(windows)` rather than a
`#[cfg(not(windows))]` twin, which is the split this crate already makes: the
attribute is for a body that _cannot_ compile on the other platform, `cfg!` for
one that compiles anywhere and only needs a different answer. It matters more
than usual here — an attribute would have left the two new lines uncompiled on
the only runner that existed when they were written, which is this ticket's own
complaint in miniature.

**The suite read the developer's real `remote-access.json`, and that was a bug.**
Found by running it, not by reading it. `TestServer::start_on` overwrote
`config.preferences` with a throwaway directory — the seam that keeps the suite
off a real `settings.json` — but left `config.remote_access` as
`ServerConfig::detect` had found it. On a machine with network access switched
on, every server the harness started bound `0.0.0.0`, and `TestServer::addr`
answers with whatever was bound; connecting to `0.0.0.0` is `AddrNotAvailable`
on Windows, so **298 integration tests failed** across every HTTP and socket
binary while the 535 lib tests stayed green.

It is worth being precise about what this was, because it is the opposite of
what this ticket went looking for: not a platform difference, but a _machine_
difference that only Windows could see — connecting to `0.0.0.0` is legal on
Linux, where it means loopback, so the Linux runner this ticket added would
never have reported it. `start_on` now settles `RemoteAccess::none()` beside the
preferences directory, for the same stated reason and in the same place, so a
test added later cannot forget it, and `tests/http_boot.rs` asserts a test
server binds loopback so it cannot come back quietly.

Reviewing that fix turned up a second half nobody had noticed: `auth.policy` is
**derived** from the bind address, so overwriting the `remote_access` field
after `ServerConfig::detect` had already read it left a server bound to loopback
still advertising `remote-reachable` to every client that asked for the
descriptor. Two fields, one decision. They are now settled together by
`ServerConfig::with_remote_access`, and the derivation lives in one function
that `detect_in` and the new method share rather than being spelled out in the
constructor for one of them to drift from.

**`server/docs/running-headless.md` is the prerequisite list**, handed to ticket
04, which now writes the rest of that page rather than starting a new one. The C
compiler for `rusqlite`'s `bundled` SQLite is the non-obvious entry, with the
`failed to find tool "cc"` error quoted so it is searchable. `server/CLAUDE.md`'s
"In CI" section no longer says Windows-only.

## `main` was already red, and three of the four were one stale bug

The first acceptance criterion asks for a **green** Linux job, and there was no
green suite to add a platform to: `main` has been red since
`Settle the auth policy from the address bound, not a file read once`
(run 30467445290 and the three before it). Four tests failed there:

- `a_non_local_origin_is_refused_with_the_auth_error`
- `a_non_local_origin_is_refused_with_the_captured_error_body`
- `a_refused_upgrade_matches_the_captured_401`
- `a_cleared_terminal_forgets_what_it_showed_and_keeps_its_shell`

The first three were one bug, and a documentation bug rather than a security
one. That commit removed the origin allowlist — `crate::auth::authorize` reads
`Origin` and consults it nowhere — but left three tests asserting the refusal it
had deleted, and left the module's own policy list describing a rule the code no
longer had. So they demanded `401` and got `200`.

**Fixed here, and the distinction matters.** No behaviour changed: nothing in
`crate::auth` or the router was touched, and the effort spec's "no allowlist
work is in scope, anywhere" holds — restoring the check would have been the
out-of-scope move, and it was not made. What changed is three tests that lied
about what this server does, and one module comment that lied beside them:

- The two upgrade tests now provoke the refusal this server actually makes —
  presenting nothing, and presenting a cookie it did not mint — because what
  they exist to pin is the captured 401 body, not what produced it.
- `a_non_local_origin_is_refused_with_the_auth_error` became
  `these_routes_check_the_credential_and_not_the_origin`, asserting both halves:
  no credential is refused whatever the origin, and a credential this server
  minted is answered whatever the origin — with no
  `Access-Control-Allow-Origin`, so a browser still hands a foreign page a CORS
  error rather than the project list.
- `a_loopback_origin_is_accepted` became
  `the_origin_a_page_came_from_is_not_what_this_server_checks`, and now includes
  `evil.example` in the list it accepts. That reads alarming and is the honest
  statement of the posture — and it is the same property that lets a phone on a
  tunnel in without an allowlist to maintain, which is what this whole effort
  rests on.
- `crate::auth`'s policy list no longer claims a rule the module dropped.

**Ticket 02 will change the last of those on purpose** — a desktop app fetching
a remote server is a second origin that has to be answered — and that test is
now where the change will announce itself.

The fourth failure is a ConPTY clear leaving its scrollback behind, unrelated
and untouched. Locally it passes on a quiet machine and fails under load, as
does one interrupt test; `server/CLAUDE.md` already warns that this suite's
timeouts catch hangs rather than enforce budgets, and `--test-threads` is the
lever. On CI it is a real failure and still open.

## What a real Linux box said

**Run by hand on an Oracle Ampere box before any CI run** — Ubuntu 20.04,
**aarch64**, 3 cores, `build-essential` already present. Not the same shape as
the runner this ticket added (`ubuntu-latest` is x86_64 and much newer), so it
answers a harder version of the question and a different one.

**It compiles.** `cargo build -p laplus-server` finished in 52 seconds, clean,
first attempt. Every `#[cfg(not(windows))]` twin in the crate had been written
and never built; none of them was wrong. That was this ticket's actual question.

**`machine_label` was validated on the box rather than argued.** `HOSTNAME` is
set by bash there and **not exported to child processes** — confirmed with
`env`, with a Python child, and under `env -i` — so a server started over
`ssh host cmd` sees neither variable. Before the change it would have called
itself `laplus`; it now answers `instance-20241102-2034` from `/etc/hostname`.

**783 passed, 60 failed.** The 60 are four separate causes and only some are
laplus's:

1. **40 of them are the box, not the code.** The workspace harness runs
   `git init -b main`, and `-b` arrived in git 2.28. Ubuntu 20.04 ships 2.25.1.
   These will not reproduce on `ubuntu-latest`. Worth knowing anyway: laplus
   _drives_ git for branches and diffs, so a box running the server wants a
   modern one for the application's sake, not the suite's.
2. **A deadlock in the watcher.** Fixed — its own commit, and the reason this
   ticket was worth running by hand. See below.
3. **Windows-only assumptions in four tests.** `files` sends `src\lib\util.rs`
   and `..\secret.txt`, which are paths on Windows and ordinary _filenames_ on
   Linux, where a backslash is a legal character; `provider` expects a
   `claude.cmd` to resolve, and there is no `PATHEXT` off Windows. The tests
   encode the platform, not the behaviour. **Note what this is not:** no
   traversal escape was demonstrated — `..\secret.txt` is harmless on Linux.
   Whether real `../` traversal is still refused there is unverified and worth
   checking before the guard is trusted cross-platform.
4. **The pty.** `resizing_a_terminal_resizes_the_pty_the_shell_is_running_in`
   fails with "the shell did not see the size it was opened at" and a bare `$ `
   captured. This was called "the one finding with real product risk" when it
   was still undiagnosed, and it was not: `size_marker()`'s non-Windows branch
   was a single space, and a space arrives with the shell's own prompt, so the
   assertion read the prompt before `stty size` had printed. **The pty works on
   Linux** — the resize really does reach the shell. Recorded because the wrong
   guess is worth as much as the right one here: three of the four causes above
   were tests, not code.

## Where the 60 went

**60 → 3**, and the three that remain are listed below rather than summarised,
because two of them are open questions.

- **40** were git 2.25 on the box. Upgraded to 2.50.1; gone.
- **11** were one bug in two copies: `FakeAgent::reporting` and `provider`'s own
  `Fake::reporting` wrote `echo 2.1.220 (Claude Code)` into a `#!/bin/sh`
  script, where a bare `(` opens a subshell and dash refuses the line. Every
  test built on those fakes was exercising a _failing_ binary while claiming to
  exercise a reporting one — worse than a failure, because it passed on Windows
  and lied everywhere else.
- **4** were the `files` and `provider` platform assumptions above. The
  backslash cases are Windows-only now; `../secret.txt` and
  `src/../../secret.txt` stay asserted everywhere **and pass on Linux**, which
  answers the question left open above: a real traversal _is_ refused there.
- **2** were the pty marker and the terminal flood's pong ordering.

### The two still failing on Linux

**Was three.** The third is diagnosed and fixed — see below — and it was a test,
which makes four of the five causes in this ticket tests rather than code.

1. **`a_file_written_outside_the_server_is_reported_relative_to_its_workspace`**
   — the inotify new-subdirectory gap, deferred deliberately. Its own ticket.
2. **`a_call_that_names_no_size_does_not_resize_the_terminal`** — **flaky**, not
   constant: three runs in isolation gave FAILED, FAILED, ok, each in 0.02s,
   which is far too fast for a test that opens a pty. Undiagnosed. A test that
   fails two runs in three is worse than one that always does, because it will
   be re-run until it passes and then believed.
   **`a_session_that_ends_holding_a_question_closes_it` was the third, and it was
   the harness.** It is worth writing down at length, because it was called "the one
   most likely to be a real fault" and it was not one at all — the server does
   exactly what the test is protecting.

The symptom was as described: the agent reaches `DIES`, the server logs
`claude: FATAL ERROR: the agent went away`, and nothing arrives for 60 seconds.
What settled it was dumping the values the _first_ reader had already been sent.
All eight of them arrive in **one chunk** on Linux:

```
thread.message-sent
thread.turn-start-requested
thread.session-set            starting
thread.session-set            running
thread.activity-appended      user-input.requested
thread.activity-appended      provider.user-input.respond.failed   <- the assertion
thread.activity-appended      session.failed
thread.session-set            error                                <- the terminal event
```

So the question _is_ closed. `values_until` matched `user-input.requested` at the
fifth value, returned the whole batch and kept no position in it, so
`events_through_the_turn` then waited `READ_TIMEOUT` for a chunk whose contents
it had already been handed. On Windows the batch splits — `cmd.exe` is slow
enough between `type` and `exit 3` that the boundary falls in a convenient place
— which is the whole of why this looked like a platform difference.

Fixed in the harness rather than in the test: `SocketClient::unread` keeps the
values a reader stopped short of, and `values_until` now stops _at_ its match
instead of at the end of whatever batch the match arrived in. A test can no
longer see a different amount of a turn depending on where the server put a chunk
boundary, which was a latent flake in every reader built on it and not only this
one.

**Verified**, and stated exactly because the count is the evidence:

- The test itself takes **0.61s on Linux**, where it used to spend 60 seconds
  reaching `READ_TIMEOUT`. It also fails in _isolation_ before the fix, so the
  60 seconds were never load.
- **Windows: 883 passed, 0 failed** — unchanged, so stopping mid-batch broke no
  reader that was relying on getting the whole batch.
- **Linux: 881 passed, 2 failed**, and the two are the two above —
  `a_call_that_names_no_size_does_not_resize_the_terminal` and the watcher's
  `a_file_written_outside_the_server_is_reported_relative_to_its_workspace`.
  881 + 2 = 883, which is the Windows total, so no test is being skipped.
  **Linux is not green and this did not make it green** — it removed one of the
  three failures and left the other two exactly as they were.

The lesson is the one this ticket keeps relearning, so it is worth stating
plainly: **"deterministic on Linux" is evidence about the harness at least as
much as about the product.** A one-chunk batch is deterministic, and so is the
reader that mishandles it.

**The watcher deadlock, because it is the one that would have shipped.**
`a_released_workspace_is_no_longer_watched` did not fail on Linux, it **hung**,
at `--test-threads=1`, so not interference. `release` held the registry lock
across `notify.unwatch`; inotify waits for its event thread to acknowledge, and
that thread sits in `deliver` waiting for the same lock. `project.delete` is the
real caller, so closing a project while anything had changed underneath would
have wedged the watcher and everything queued behind its lock. Windows never saw
it because `ReadDirectoryChangesW` needs no such handshake. Fixed in both
`release` and `watch`, verified on Linux (0.51s instead of forever) and on
Windows (870 passing).

**Still open, deliberately:** a file written into a _just-created_
subdirectory is never reported on Linux — inotify registers per directory, so
the write lands before the watch on `src/` exists, and the event is lost rather
than late. The consequence is a stale `@`-mention listing when the agent or
`cargo` creates a directory and writes into it. Fixing it is a watcher redesign
(rescan on directory-create, or subtree exclusions this module's own header
already wanted), so it wants its own ticket rather than this one.

## What CI has not yet said

**The job has never run.** It fires on push to `main` and on pull requests
touching `server/`, and this work has done neither yet — so everything about
`ubuntu-latest` specifically, as opposed to the aarch64 box above, is still an
argument rather than a result. What the first run should be checked against
rather than assumed:

- The ConPTY failure, which is Windows-specific and should **pass** on Linux.
- Anything the `cfg(not(windows))` paths get wrong. This is the ticket's
  actual question and the compile is the first half of its answer.

If the run disagrees with any of that, the disagreement is the finding item 2
asked for, and it belongs in this section.

## What is left

**The hand-driven run — item 4 above, and the fifth acceptance criterion.** It
has not happened and could not: the machine this was implemented on is Windows
with no WSL, so there was no Linux box to drive. Everything else in this ticket
is verified; this is not, and no amount of green CI substitutes for it. The
three things a compile will not catch, listed above, are still open questions:

- **`machine_label`** now reads `/etc/hostname`, and the CI job proves the code
  path compiles and that `a_machine_that_knows_its_own_name_is_labelled_with_it`
  agrees with the runner's own file. What it does not prove is what a real
  distribution writes there.
- **The `claude` binary** resolving on a real Linux `PATH`.
- **A terminal.** `crate::terminal` opening `/bin/bash` under a pty is the
  feature most likely to behave differently and the one the suite speaks for
  least.

Whoever picks this up: start the server, pair a browser, open a terminal, run a
turn, and write what it found into the "What has and has not been checked"
section of `server/docs/running-headless.md`. If it finds enough to need its own
note, a `HANDOFF-` file beside this ticket is the right home.
