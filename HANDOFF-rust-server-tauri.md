# Handoff — lightcode: Tauri shell + Rust server, reusing t3code's UI

**Date:** 2026-07-26
**Status:** Plan agreed, no code written yet.
**Companion doc:** `t3code-electron-to-tauri-migration.md` (cited research on why the plain Electron→Tauri migration was rejected)

---

## Goal

Build a lightweight desktop coding-agent app by **reusing t3code's existing UI unchanged** and **replacing its Node/TypeScript server with a Rust one**, shipped in a Tauri shell.

Target artifact size: **~20–30 MB** (vs. t3code's 318 MB Windows installer).

---

## Why this shape (short version)

t3code today is a **server + three frontends**. `apps/web`, `apps/mobile`, and `apps/desktop` all talk to `apps/server` over HTTP. The Electron app is a window pointed at a local instance of that server.

Electron works as both browser engine *and* Node runtime (`ELECTRON_RUN_AS_NODE=1`, `apps/desktop/src/backend/DesktopBackendConfiguration.ts:351-359`), so one binary does two jobs and no separate Node is shipped.

Three options were considered:

| Option | Effort | Size | Verdict |
|---|---|---|---|
| 1. Prune the Electron build | weeks | 318 → ~200 MB | Cheapest; keeps all features. Fallback if 3 stalls. |
| 2. Tauri shell + keep the TS server | 6–10 mo | ~100–220 MB | **Rejected.** Rust binary has no Node, so you must bundle `node.exe` (~35 MB) as a sidecar. Most work, least payoff. |
| 3. **Tauri shell + Rust server (scoped)** | **2–4 mo** | **~20–30 MB** | **Chosen.** Nothing Node ships at all. |

Option 2's failure is the load-bearing insight: you delete 136 MB of Electron and add back 35 MB of Node, while inheriting a cross-engine QA matrix. Only rewriting the server removes the runtime entirely.

---

## What gets reused vs. rewritten

```
apps/web                100,385 LOC   REUSE AS-IS   entire UI
packages/client-runtime  13,728 LOC   REUSE AS-IS   client data layer
packages/contracts       11,903 LOC   SPEC          typed API schema — your blueprint
packages/shared               —       PORT/REF      shared helpers, port what you need
apps/server              87,585 LOC   REWRITE       the only thing you rebuild
```

**The UI needs no changes.** It's already a plain browser app — it ships standalone via `npx t3`, and all Electron coupling is confined to `apps/desktop/src/preload.ts` and feature-detected. It speaks HTTP to the server and doesn't care what language serves it.

Known minor exception: `-webkit-app-region` drag regions appear in 8+ places for the custom titlebar. Tauri on Windows uses WebView2 (Chromium), so these work; revisit only if targeting macOS/Linux.

---

## Contracts: implement vs. skip

`packages/contracts/src/` — 31 files. For a scoped v1:

**Implement:**
- `filesystem.ts` — file read/write/watch
- `git.ts` — status/diff/branch (shell out to `git`)
- `project.ts` — workspace/project model
- `provider.ts`, `providerInstance.ts`, `providerRuntime.ts` — agent driver surface (**the core**)
- `orchestration.ts` — session/run management
- `model.ts`, `editor.ts`, `keybindings.ts`, `baseSchemas.ts`

**Skip for v1:**
- `auth.ts` — no accounts
- `environment.ts`, `environmentHttp.ts` — remote/Tailscale envs
- `preview.ts`, `previewAutomation.ts` — the CDP preview subsystem (see Risks)
- `desktopBootstrap.ts`, `ipc.ts` — Electron-specific bootstrap
- `assets.ts`

Corresponding server subsystems to skip: `cloud/`, `auth/`, `sourceControl/`, `preview/`, WSL support, SSH, Tailscale.

---

## Proposed Rust stack

| Concern | Crate |
|---|---|
| HTTP + WebSocket | `axum` |
| Async runtime | `tokio` |
| Agent CLI subprocess | `tokio::process` |
| Terminal / PTY | `portable-pty` |
| Database | `rusqlite` (or `sqlx`) |
| Contract types | `serde` + `serde_json` |
| Desktop shell | `tauri` v2 |
| File watching | `notify` |

Windows-only to start (`webviewInstallMode: downloadBootstrapper`, documented as ~0 MB overhead). Avoids the Linux WebKitGTK problem entirely — Tauri's own AppImage docs note bundles grow "from the 2-6 MB range to 70+ MB."

---

## STEP 1 — Do this first (≈1 week spike)

**Do not start the full port before this passes.** It de-risks the single unknown.

Build a bare Rust binary that:

1. Spawns the user's installed `claude` binary as a subprocess
2. Streams its stdio protocol, parsing messages
3. Serves the minimum endpoints for the UI to render one chat session
4. Points the **existing, unmodified** `apps/web` dev server at it

**Pass:** you see agent output streaming in t3code's real UI, driven by Rust.
**Fail:** the protocol fights you → fall back to Option 1 (prune Electron). One week lost, not three months.

### Reference points in the existing code

- `apps/server/src/provider/Layers/ClaudeProvider.ts` — how the SDK is configured
- `apps/server/src/provider/Layers/ClaudeAdapter.ts` — wraps SDK query sessions behind a generic interface. **This is the shape to reimplement.**
- `apps/server/src/provider/Drivers/ClaudeExecutable.ts` — binary resolution, incl. Windows npm-shim handling
- `apps/server/src/textGeneration/ClaudeTextGeneration.ts`

Key fact: t3code **does not bundle Claude Code**. It resolves the user's installed binary from PATH and passes it to the SDK as `pathToClaudeCodeExecutable`. The SDK is a subprocess client, not an embedded harness. Your Rust code takes the SDK's place.

---

## STEP 2+ — Build order after the spike

1. `axum` server skeleton + contract types (`serde` structs from `packages/contracts`)
2. `filesystem` + `project` endpoints — get the file tree rendering
3. `provider*` + `orchestration` — full agent session lifecycle
4. `terminal` via `portable-pty`
5. `git` (shell out to the `git` binary; don't link libgit2 initially)
6. `persistence` via `rusqlite`
7. Wrap in Tauri, point the webview at the embedded server
8. Bundle + measure

---

## Risks

**1. Agent stdio protocol stability (the main one).**
The `claude` CLI's stdio wire format is not a stability-guaranteed public contract the way the HTTP API is. It can shift between releases. **Mitigation:** isolate it behind one Rust module so a protocol change is a contained fix; support one agent first; pin a known-good Claude Code version during development.

**2. Scope creep back toward parity.**
88K LOC of server exists for reasons. Resist reimplementing `cloud/`, `auth/`, multi-backend orchestration. If v1 grows past ~20K LOC of Rust, re-evaluate.

**3. Effect-TS semantics.**
`apps/server` is heavily Effect-based (structured concurrency, typed errors, resource scoping). Don't port Effect idioms literally — use Rust's own (`Result`, `?`, `tokio` tasks, RAII). Read the TS for *behavior*, not structure.

**4. Contract drift from upstream.**
You're pinning to t3code's contracts at a point in time. If you later want to pull UI updates from upstream, contract changes become your problem. Decide early whether this is a hard fork.

---

## Open decisions

- [ ] **Hard fork or track upstream?** Affects how carefully you mirror `packages/contracts`.
- [ ] **Which agents in v1?** Claude Code only is the recommendation. Codex (`packages/effect-codex-app-server`) and OpenCode (`@opencode-ai/sdk`) are separate protocols, separate work.
- [ ] **Windows-only, or cross-platform later?** Windows-only removes the WebKitGTK problem and the cross-engine QA matrix.
- [ ] **Licensing** — check t3code's `LICENSE` before shipping anything derived from `apps/web`.

---

## Facts worth not re-deriving

- t3code v0.0.28 real artifact sizes: 210.1 MB (mac arm64 dmg), 218.7 MB (Linux AppImage), 318.4 MB (Windows NSIS)
- Electron 41.5.0 runtime baseline: 110.9–136.1 MB → **~43–53% of every artifact is app payload, not Electron**
- Biggest payload items: `node-pty` 61.4 MB unpacked, `effect` 42.7 MB unpacked, `playwright-core` 11.9 MB
- Windows ships a **duplicate Linux/glibc `node_modules` tree** for the WSL backend (`scripts/build-desktop-artifact.ts:909-914`) — ~80 MB of the 100 MB Windows-vs-macOS gap
- WSL is **opt-in, default off** (`apps/desktop/src/settings/DesktopAppSettings.ts:77`), but baked into the installer regardless
- `tauri-plugin-updater` has **no delta/differential download support**; `electron-updater` does (`.blockmap`). t3code publishes nightlies every 3 hours.

---

## If this stalls

Fall back to **Option 1** — prune the Electron build. Weeks of work, zero risk, no feature loss:

1. Make the WSL Linux `node_modules` tree an opt-in download (~80 MB off Windows)
2. Bundle `effect` into `apps/server/dist/bin.mjs` (tens of MB)
3. Prune `node-pty` to the target triple only
4. Inline the one string literal extracted from `playwright-core` at build time
5. Narrow `asarUnpack` from `**/node_modules/**`

Gets Windows to ~200 MB and macOS to ~150–165 MB. This work is **not wasted** under Option 3 either — it's the same payload analysis.
