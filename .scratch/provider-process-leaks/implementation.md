# Provider lifecycle and Windows link fixes

Status: ready-for-human
Date: 2026-09-05

## Result

Implemented and built locally. The installed application was not replaced or
restarted; running the rebuilt shell is required to use these changes.

- Claude and Codex now share the existing 90-second idle eviction policy with
  owned OpenCode sessions. Eviction requires a saved continuation cursor and
  holds for an active turn, queued prompt, unanswered request or known live
  subagent. The next message resumes the saved provider conversation.
- Each Claude/Codex conversation owns a Windows kill-on-close Job Object;
  short-lived Codex probes own one too. Cleanup closes this job even when the
  root exited before cleanup ran. The process-wide job remains the crash
  backstop. Job creation/assignment failures stop the new launch and surface an
  error rather than leaving that session uncontained.
- Codex collaboration targets matching the root thread no longer create a
  self-child that holds the session in `running`. Genuine children remain
  tracked. Root `turn/started` notifications now identify automatic successors,
  open a laplus turn when needed, and allow their completion to settle it. Late
  terminal events for the preceding turn remain rejected.
- Windows external navigation now calls native ShellExecuteExW via `open`
  with `shellexecute-on-windows`, rather than spawning `explorer.exe` with a URL.
  HTTP/HTTPS/mailto scheme eligibility stays in the existing shell policy.

## Evidence and verification

Before the fix, `provider_conversation_idle` failed both provider cases with
`idle provider retained its process instead of being evicted` (15-second hang
detector per case). The Windows process disposal regression failed with
`session helper survived its exited root`. Both now pass.

The Codex socket regression reproduced `latestTurn.state = completed` alongside
`session.status = running`. The automatic-successor decoder regression dropped
its completion. Both now pass, with an additional socket test verifying the
automatic answer is stored and the conversation becomes ready.

Focused checks (run from `server/`, logs retained locally as ignored `.log` files):

```powershell
cargo test -p laplus-server --test provider_conversation_idle --test codex_lifecycle --test opencode_conversation_idle --no-fail-fast -- --test-threads=1
cargo test -p laplus-server --test socket_codex_turn --no-fail-fast -- --test-threads=1
cargo test -p laplus-server --lib process::supervision --no-fail-fast -- --test-threads=1
cargo test -p laplus-server --test socket_provider server_shutdown_cancels_and_reaps_an_in_flight_codex_probe --no-fail-fast
cargo test -p laplus-shell external_opening_is_an_allowlist_of_schemes_the_os_may_carry --no-fail-fast
cargo clippy -p laplus-server --lib -p laplus-shell --bin laplus
```

The initial socket binary run passed 38 tests and exposed two existing harness
problems: a capture expected release 0.1.12 rather than this build's 0.1.13,
and a persistent test listener inherited the developer's remote bind address,
causing Windows error 10049 before reaching provider code. The harness now checks
the current build version and uses the same isolated loopback policy as its
other constructor. Both failing cases passed their focused reruns.

Clippy completed with existing warnings. The web bundle and debug shell built
successfully. New Rust test files were formatted; unchanged legacy modules were
not reformatted. `git diff --check` passed.

Actual UI verification:

```powershell
node server/tools/ui-driver/probe-provider-lifecycle.mjs
node .scratch/link-opening/probe-native-browser.mjs
```

The first probe used isolated app data, credential homes and a fake Codex CLI.
Two composer submissions rendered automatic follow-up answers and settled;
the provider PID exited after each idle period, and the second invocation
resumed the same thread. The final probe verifies effective provider settings
before submitting and restricts executable lookup. During development of this
probe, an earlier setup inadvertently invoked one real Codex turn in its
temporary workspace; it finished, that instance was stopped, and this was
disclosed to the user. Existing user sessions were not touched.

The second probe clicked new-window and same-window HTTP anchors in the actual
Tauri WebView2 shell. The default Brave browser fetched the exact local URL and
query, and laplus retained its origin. See the
[native verification](../link-opening/native-verification.md) for limits.
All probe servers, shells and browser automation processes were stopped.

## Remaining limits

The exact intermittent HTTPS-to-Documents trigger was not captured. Native HTTP
dispatch was exercised; HTTPS content rendering and mailto were not. The code
change replaces the suspect Windows dispatch boundary without claiming to fix
every possible browser-association problem.

Windows job assignment still occurs immediately after spawn, as in ADR-0060.
This change does not close the pre-assignment descendant/crash race. Atomic job
assignment during creation remains a separate improvement. Linux keeps its
existing cooperative process cleanup; SessionJob is Windows-specific.

Saved self-child streams from older runs are historical data. Their `heard`
flag starts false on restore (`subagents.rs::Slot::restored`), so they do not
keep a restarted application running; newly filtered root events do not revive
them. No live database rows were rewritten. Missing terminal notifications and
provider background work not represented in known subagent state remain outside
the reproduced fixes; silence or final-looking text is not treated as proof
that a turn completed.

Research: [process evidence](findings.md), [Windows ownership](windows-cleanup-research.md),
[Codex completion](../codex-completion/research.md),
[Documents-window report](../link-opening/documents-window-research.md).
