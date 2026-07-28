# Migrating T3 Code from Electron to Tauri

Research note. Question: **can `pingdotgg/t3code` be migrated from Electron to Tauri, and what would it actually cost?**

Method: primary sources only — t3code's own source at commit `5719e8a`, the published GitHub release artifacts, official Tauri v2 docs (`v2.tauri.app`), official Electron docs, the `tauri-apps` GitHub repos, and the npm registry for package weights. Secondary sources are marked as such.

**Short answer: yes, technically; no, economically.** The Electron runtime is roughly half of the download, but the other half is application payload that Tauri does not touch — and the biggest single line item in that payload is a **Node.js runtime and `node_modules` tree that Tauri would force you to ship _again_, separately**, because Electron is currently doing double duty as both the browser engine and the Node runtime. Details below.

---

## 1. What t3code actually is

### Monorepo layout

pnpm workspace, `packages: apps/*, infra/*, oxlint-plugin-t3code, packages/*, scripts` (`t3code/pnpm-workspace.yaml:1-6`). The relevant apps:

| Path                            | Package name       | Role                                                                                                                         |
| ------------------------------- | ------------------ | ---------------------------------------------------------------------------------------------------------------------------- |
| `apps/server`                   | `t3`               | Node.js WebSocket + HTTP server. **This is the npm-published CLI** (`bin: { t3: "./dist/bin.mjs" }`) behind `npx t3@latest`. |
| `apps/web`                      | `@t3tools/web`     | React 19 + TanStack Router + Tailwind v4 renderer.                                                                           |
| `apps/desktop`                  | `@t3tools/desktop` | The Electron shell.                                                                                                          |
| `apps/mobile`, `apps/marketing` | —                  | Expo app, Astro site. Out of scope.                                                                                          |

`AGENTS.md:26-31` states the intent directly: _"`apps/server`: Node.js WebSocket server. Wraps Codex app-server (JSON-RPC over stdio)... `apps/web`: React/Vite UI... Connects to the server via WebSocket."_ `docs/architecture/overview.md:3` repeats it: _"T3 Code runs as a **Node.js WebSocket server** that wraps `codex app-server` (JSON-RPC over stdio) and serves a React web app."_

### The critical architectural fact

**The Electron main process is a supervisor for a spawned Node.js server process.** It is not where the app logic lives.

`apps/desktop/src/backend/DesktopBackendConfiguration.ts:351-367`:

```ts
return {
  executablePath: process.execPath,
  args: [environment.backendEntryPath, "--bootstrap-fd", "3"],
  entryPath: environment.backendEntryPath,
  cwd: environment.backendCwd,
  env: {
    ...backendChildEnvPatch(),
    ELECTRON_RUN_AS_NODE: "1",
  },
```

`process.execPath` is the Electron binary; `ELECTRON_RUN_AS_NODE=1` makes it behave as plain Node. Electron's own docs define this as _"Starts the process as a normal Node.js process"_ ([Electron environment variables](https://www.electronjs.org/docs/latest/api/environment-variables)). `environment.backendEntryPath` resolves to `apps/server/dist/bin.mjs` (`apps/desktop/src/app/DesktopEnvironment.ts:189`).

So the shipped Electron binary serves **two** purposes: the Chromium renderer _and_ the Node runtime for the backend. That is the single most important fact in this whole analysis, because Tauri removes the first and gives you nothing for the second.

Supervision (restart loop, readiness probing, log capture, graceful shutdown) lives in `apps/desktop/src/backend/DesktopBackendManager.ts` and `DesktopBackendPool.ts`. There are two backend instances on Windows: the native primary and a WSL one launched via `executablePath: "wsl.exe"` (`DesktopBackendConfiguration.ts:486`).

### Size of each surface

```
apps/desktop/src   19,766 LOC (66 non-test .ts files)
apps/server/src    87,585 LOC
apps/web/src      100,385 LOC
```

The Electron shell exposes **75 IPC channel constants** (`apps/desktop/src/ipc/channels.ts`), bridged to the renderer through `contextBridge.exposeInMainWorld("desktopBridge", {...})` (`apps/desktop/src/preload.ts:30-244`).

### Renderer

React 19.2.6 + React Compiler, TanStack Router, Tailwind v4 (CSS-native `@theme`, no JS config — `apps/web/src/index.css:1,117`), Lexical editor, `@xterm/xterm` 6 for the terminal UI (`apps/web/src/components/ThreadTerminalDrawer.tsx:2,21`), `@pierre/diffs`, Effect-TS for state. Bundled with `vite-plus` (`@voidzero-dev/vite-plus-core`, `pnpm-workspace.yaml:50-51`).

**The renderer is already browser-portable.** It ships as a plain hosted web app (`npx t3@latest`, `README.md:3,19`), and every Electron touchpoint is feature-detected: `apps/web/src/env.ts:6-8` gates on `window.desktopBridge !== undefined || window.nativeApi !== undefined`; `apps/web/src/hostedPairing.ts:34` has `isHostedStaticApp()`; `apps/web/src/contextMenuFallback.ts` provides a DOM context menu for non-Electron. There is **no** `ipcRenderer`, `require(`, `__dirname`, or `process.*` access anywhere in `apps/web/src` or `packages/client-runtime/src` — all Electron coupling is confined to `apps/desktop/src/preload.ts`. This is the one genuinely good news item in the report.

---

## 2. Where the size actually comes from

### Measured, not estimated: published release artifacts

`gh release view v0.0.28 --repo pingdotgg/t3code` (stable, 2026-06-29):

| Artifact                         | Size         |
| -------------------------------- | ------------ |
| `T3-Code-0.0.28-arm64.dmg`       | **210.1 MB** |
| `T3-Code-0.0.28-x64.dmg`         | 218.3 MB     |
| `T3-Code-0.0.28-x86_64.AppImage` | 218.7 MB     |
| `T3-Code-0.0.28-x64.exe` (NSIS)  | **318.4 MB** |

Latest nightly (`v0.0.29-nightly.20260725.899`) is bigger still: 231.9 MB (mac arm64 dmg), 247.5 MB (AppImage), 325.0 MB (Windows exe).

### The Electron runtime baseline

`gh api repos/electron/electron/releases/tags/v41.5.0` — t3code pins `"electron": "41.5.0"` (`apps/desktop/package.json`):

| Electron 41.5.0 runtime zip | Size     |
| --------------------------- | -------- |
| `darwin-arm64`              | 110.9 MB |
| `darwin-x64`                | 116.2 MB |
| `linux-x64`                 | 111.8 MB |
| `win32-x64`                 | 136.1 MB |

### The split

| Platform             | Shipped artifact | Electron runtime | **App-side remainder** | Electron share |
| -------------------- | ---------------- | ---------------- | ---------------------- | -------------- |
| macOS arm64          | 210.1 MB         | 110.9 MB         | **~99 MB**             | ~53%           |
| macOS x64            | 218.3 MB         | 116.2 MB         | **~102 MB**            | ~53%           |
| Linux x64 (AppImage) | 218.7 MB         | 111.8 MB         | **~107 MB**            | ~51%           |
| Windows x64 (NSIS)   | 318.4 MB         | 136.1 MB         | **~182 MB**            | ~43%           |

**Honesty caveat:** these are all compressed artifacts using _different_ compressors (DMG uses zlib/LZFSE, AppImage squashfs, NSIS LZMA, Electron's own zip uses deflate). The subtraction is indicative, not exact. Two independent cross-checks support it:

1. **Windows-vs-macOS delta.** Windows x64 is 100.1 MB larger than macOS x64, but the Electron runtime accounts for only 19.9 MB of that (136.1 − 116.2). The remaining ~80 MB is app-side, and the code explains exactly why: `scripts/build-desktop-artifact.ts:909-914` sets `supportedArchitectures` on Windows to `os: [win32, linux], libc: ["glibc"]` — **the Windows installer ships a complete second, Linux/glibc `node_modules` tree** so the WSL backend can run. The build comment at line 898-901 confirms the intent.

2. **Bottom-up from npm registry unpacked sizes** (exact pinned versions):

| Package                          | Version       | Unpacked    |
| -------------------------------- | ------------- | ----------- |
| `node-pty`                       | 1.1.0         | **61.4 MB** |
| `effect`                         | 4.0.0-beta.78 | **42.7 MB** |
| `playwright-core`                | 1.60.0        | 11.9 MB     |
| `@ff-labs/fff-bin-win32-x64`     | 0.9.4         | 6.2 MB      |
| `@ff-labs/fff-bin-linux-x64-gnu` | 0.9.4         | 5.3 MB      |
| `@pierre/diffs`                  | latest        | 5.0 MB      |
| `@anthropic-ai/claude-agent-sdk` | 0.3.170       | 4.5 MB      |

`effect` + `node-pty` alone are **104 MB uncompressed**, and both are shipped as real `node_modules`, not bundled. This is deliberate: `apps/server/vite.config.ts` only inlines workspace packages (`@pierre/diffs`, `@t3tools/*`, `effect-acp`, `effect-codex-app-server`); everything else stays external. And `scripts/build-desktop-artifact.ts:1409` sets `asarUnpack: [...DESKTOP_ASAR_UNPACK, "apps/server/dist/**", "**/node_modules/**"]` — the _entire_ `node_modules` tree is unpacked out of the asar, with the reason spelled out at lines 1396-1408: the WSL backend runs plain `wsl.exe -- node`, which cannot read inside an asar archive.

### The finding that matters

**Roughly half the macOS/Linux download and 57% of the Windows download is application payload that a Tauri migration does not touch at all.** And a large part of that payload exists _because_ the app runs a Node server — which under Tauri you would have to ship as a sidecar _in addition to_ everything you're already shipping.

---

## 3. Realistic size after a Tauri migration

### What Tauri actually gives you

Tauri uses the OS webview: WebView2 (Chromium) on Windows, WKWebView on macOS, WebKitGTK on Linux ([Tauri webview versions](https://v2.tauri.app/reference/webview-versions/)). The official size page ([v2.tauri.app/concept/size](https://v2.tauri.app/concept/size/)) makes no numeric claim — it only says Tauri "by default provides very small binaries" and lists Cargo profile flags (`lto`, `opt-level = "s"`, `strip`, `panic = "abort"`) plus `removeUnusedCommands`.

For a real number I measured a shipping Tauri v2 app's published artifacts — `tw93/Pake` v3.15.1, confirmed Tauri v2 (`src-tauri/Cargo.toml`: `tauri = "2.10.2"`, `tauri-build = "2.5.5"`, plus `tauri-plugin-shell`, `-http`, `-global-shortcut`, `-window-state`). Pake is essentially a webview wrapper with negligible app payload, so it approximates the Tauri floor:

| Pake artifact (Tauri 2.10.2)      | Size        |
| --------------------------------- | ----------- |
| `ChatGPT.dmg` (macOS)             | 9.9 MB      |
| `ChatGPT_x64.msi` (Windows)       | 3.7 MB      |
| `ChatGPT_x86_64.deb` (Linux)      | 4.7 MB      |
| `ChatGPT_x86_64.AppImage` (Linux) | **79.2 MB** |

_(Third-party app, used as an empirical Tauri floor — not a Tauri project claim.)_

### The platform asymmetry is severe

- **Windows.** Default `webviewInstallMode` is `downloadBootstrapper`, documented overhead **"0MB"** ([Windows installer docs](https://v2.tauri.app/distribute/windows-installer/)). `embedBootstrapper` is ~1.8 MB; `offlineInstaller` ~127 MB; `fixedVersion` ~180 MB. So the small number is only available if you accept a network dependency at install time and require the WebView2 runtime be present.
- **macOS.** WKWebView is always system-provided. Genuine ~110 MB saving.
- **Linux.** This is the weak spot. Tauri's own AppImage docs state the bundle _"bundle[s] all dependencies and files needed by the application"_ and that **"the file size grows from the 2-6 MB range to 70+ MB"** ([AppImage distribution](https://v2.tauri.app/distribute/appimage/)). Pake's 79.2 MB AppImage vs 4.7 MB deb confirms it empirically. A `.deb` avoids this but requires system `webkit2gtk-4.1`, converting a portable single file into a distro dependency problem.

### The sidecar you have to add back

The Node backend does not disappear. Official Node 24 distributions (t3code requires `node: ^24.13.1` at `package.json` and `engines.node: "^22.16 || ^23.11 || >=24.10"` at `apps/server/package.json`):

| Node 24.13.1 dist     | Size    |
| --------------------- | ------- |
| `darwin-arm64.tar.gz` | 48.6 MB |
| `darwin-x64.tar.gz`   | 49.8 MB |
| `linux-x64.tar.xz`    | 29.6 MB |
| `win-x64.zip`         | 34.5 MB |

Those tarballs include npm, headers and docs; a stripped `node` binary alone would be smaller. Call it **~30-45 MB compressed** per platform. Tauri supports this via `externalBin` sidecars with target-triple suffixes, spawned through `tauri-plugin-shell` with `CommandEvent::Stdout` streaming and `child.write()` for stdin ([sidecar docs](https://v2.tauri.app/develop/sidecar/)).

### Projected artifacts

| Platform                  | Today    | Tauri shell | App payload (unchanged) | Node sidecar | **Projected** | Saving           |
| ------------------------- | -------- | ----------- | ----------------------- | ------------ | ------------- | ---------------- |
| macOS arm64               | 210.1 MB | ~10 MB      | ~99 MB                  | ~35 MB       | **~145 MB**   | ~65 MB (**31%**) |
| Windows x64               | 318.4 MB | ~5 MB       | ~182 MB                 | ~35 MB       | **~222 MB**   | ~96 MB (**30%**) |
| Linux AppImage            | 218.7 MB | ~79 MB      | ~107 MB                 | ~30 MB       | **~216 MB**   | ~3 MB (**~1%**)  |
| Linux .deb (hypothetical) | n/a      | ~5 MB       | ~107 MB                 | ~30 MB       | ~142 MB       | ~35%             |

**You spend 15-27 engineer-months (§6) to make the download ~30% smaller on two platforms and ~0% smaller on Linux AppImage.** That is the honest headline.

---

## 4. Migration blockers, ranked

Legend: **[BLOCKER]** no viable path · **[RUST]** substantial Rust work, no plugin · **[PLUGIN]** first-party Tauri plugin exists · **[OK]** trivial.

### 1. [BLOCKER] The preview / embedded-browser subsystem and its CDP automation

The largest and most Electron-specific feature. `apps/desktop/src/preview/Manager.ts` (~3,044 lines) hosts per-tab Chromium `WebContents` from `<webview>` tags and drives them through **`webContents.debugger.attach("1.3")`** (line 811) — the Chrome DevTools Protocol. Methods used, by file:line in `Manager.ts`:

- `Runtime.enable` / `Runtime.evaluate` (814, 979) · `Runtime.consoleAPICalled`, `Runtime.exceptionThrown` (565, 584)
- `Network.enable` + `requestWillBeSent` / `responseReceived` / `loadingFailed` / `loadingFinished` (814, 614, 629, 648, 666)
- `Accessibility.enable` / `Accessibility.getFullAXTree` (814, 1916, 1983) — accessibility-tree snapshots for automation
- `Page.startScreencast` (1808-1814, `format: "jpeg", quality: 80`), `Page.stopScreencast` (1835), `Page.screencastFrame` / `screencastFrameAck` (734, 742), `Page.bringToFront` (2331)
- `Input.dispatchMouseEvent` (2137, 2143), `Input.dispatchKeyEvent` (2305, 2335), `Input.setIgnoreInputEvents` (869)
- `Emulation.setEmulatedMedia` (1702), `Emulation.setFocusEmulationEnabled` (2307, 2332)
- `Log.enable` / `Log.entryAdded` (814, 599)

Plus `webContents.capturePage` for screenshots (243, 1764, 1990), per-tab `session.fromPartition("persist:t3code-preview-…")` (`BrowserSession.ts:12,106-121`), and a guest preload (`PickPreload.ts`) for element picking. `PlaywrightInjectedRuntime.ts:13-14,126-141` extracts Playwright's `InjectedScript` class source out of `playwright-core/lib/coreBundle.js`, evaluates it in a `node:vm`, and injects it into guest pages via CDP `Runtime.evaluate` to get Playwright's selector engine inside the page. `webviewTag: true` is enabled on the main window specifically for this (`apps/desktop/src/window/DesktopWindow.ts:321-339`).

**Tauri parity: none.** WKWebView and WebKitGTK have no CDP. Tauri's multiple-webview-per-window API is explicitly experimental — `docs.rs/tauri/latest/tauri/webview/struct.WebviewBuilder.html` carries _"Available on crate feature `unstable` only"_, and `tauri-apps/tauri#10420` ("[bug] Broken positioning with multiwebview (unstable feature) example") is open. A Chromium backend for Tauri has been asked for and declined/left open (`tauri-apps/tauri#981` closed, `#14963` "Bundle chromium renderer" open). Even on Windows, WebView2 exposes no equivalent to Electron's `webContents.debugger`.

This feature would have to be **rewritten against a different engine or dropped**. Given it is exposed all the way to the product (`preview.automation.{click,type,press,scroll,evaluate,waitFor,snapshot}` in `preload.ts:203-220`, and MCP tools named `preview_*`), dropping it is a product decision, not a technical one.

### 2. [BLOCKER→RUST] Clerk authentication and macOS passkeys

`apps/desktop/src/app/DesktopClerk.ts:1-2,74-83` uses `createClerkBridge` from `@clerk/electron` with `passkeys: true`, `storage` from `@clerk/electron/storage`, and a `renderer: { scheme, host }` tied to the custom protocol. `preload.ts:7,12` calls `exposeClerkBridge({ passkeys: true })`. CI asserts the bridge is present in the built preload (`.github/workflows/ci.yml:40`, grepping for `__clerk_internal_electron_passkeys`).

The passkey path requires platform-native N-API binaries — `@clerk/electron-passkeys-darwin-{arm64,x64}` and `@clerk/electron-passkeys-win32-{arm64,x64}-msvc` (`scripts/build-desktop-artifact.ts:832-853`) — plus macOS Associated Domains entitlements generated at build time (`renderMacPasskeyEntitlements`, lines 771-799):

```xml
<key>com.apple.developer.associated-domains</key>
<array><string>webcredentials:…</string></array>
<key>com.apple.security.cs.allow-jit</key><true/>
<key>com.apple.security.cs.allow-unsigned-executable-memory</key><true/>
<key>com.apple.security.cs.disable-library-validation</key><true/>
```

`@clerk/electron` and `@clerk/electron-passkeys` are Electron-specific packages. Tauri has no equivalent, and Clerk ships no Tauri SDK. You would need a from-scratch native passkey bridge in Rust, or drop passkeys.

### 3. [RUST] node-pty and terminal emulation

`apps/server/package.json:34` declares `"node-pty": "^1.1.0"`. Used at `apps/server/src/terminal/NodePtyAdapter.ts:114` (`() => import("node-pty")`) and 146-152 (`nodePty.spawn(shell, args, { cwd, cols, rows, env, name: ... })`), behind an adapter interface in `PtyAdapter.ts`, orchestrated by `Manager.ts` (session lifecycle, shell resolution, resize, subprocess inspection, SIGTERM→SIGKILL escalation). It requires a native `spawn-helper` executable to be present and `chmod 0o755`'d (`NodePtyAdapter.ts:29-68`).

**Important nuance:** this is _not_ directly a Tauri problem — node-pty runs inside the **server** process, not the Electron main process. If the server is shipped as a Node sidecar, node-pty comes along unchanged. It becomes a _packaging_ problem, and the repo already has a hard-won solution for it: CI cross-builds a Linux `pty.node` (`.github/workflows/release.yml:261-300`, `node-gyp rebuild`) which is staged into the app (`scripts/build-desktop-artifact.ts:1497-1520, --wsl-prebuild`). All of that machinery survives a Tauri migration, and all of its weight survives too.

Tauri itself has **no PTY plugin** — `tauri-plugin-shell` spawns processes with piped stdio, not a pty. Searches of `tauri-apps/tauri` and `tauri-apps/plugins-workspace` for "pty"/"terminal" return nothing. So a _native-Rust_ terminal (e.g. `portable-pty`) would be a rewrite; keeping the Node sidecar avoids it.

### 4. [RUST] The custom `t3code://` protocol origin + CSP proxy

`apps/desktop/src/electron/ElectronProtocol.ts:11-13` defines `t3code://app/` (`t3code-dev://` in dev) as the **origin the entire renderer loads from**, registered with `Electron.protocol.handle` (lines 181-199) and proxying every request to the local backend over `Electron.net.fetch` (lines 107-150), injecting a per-request CSP that whitelists the Clerk frontend API and `https://challenges.cloudflare.com` (lines 67-95). The scheme is also declared to the OS via electron-builder `protocols` → `CFBundleURLTypes` (`scripts/build-desktop-artifact.ts:1429-1434`).

Tauri has custom URI scheme protocols (`register_uri_scheme_protocol` / `register_asynchronous_uri_scheme_protocol`, and `asset:` / `http://asset.localhost` per the [core JS API reference](https://v2.tauri.app/reference/javascript/api/namespacecore/)) plus a `app.security.csp` config. Achievable — but the URL shape differs per platform (`asset://` on macOS/Linux vs `http://asset.localhost` on Windows), which matters because Clerk's passkey relying-party resolution is keyed to `{ scheme, host }` (`DesktopClerk.ts:79-80`). Deep-linking parity is covered by `tauri-plugin-deep-link`.

### 5. [RUST] Window chrome and the custom titlebar

`apps/desktop/src/window/DesktopWindow.ts:184-203` uses `titleBarStyle: "hiddenInset"` + `trafficLightPosition: { x: 16, y: 18 }` on macOS, and `titleBarStyle: "hidden"` + `titleBarOverlay: { color, height: 40, symbolColor }` elsewhere. The renderer implements dragging with **`-webkit-app-region: drag/no-drag`** — a non-standard Chromium/Electron property — in at least 8 places: `apps/web/src/index.css:362,1017,1025`, `ChatView.tsx:5523`, `DiffPanel.tsx:532,715`, `chat/ExpandedImageDialog.tsx:51`, `chat/PanelLayoutControls.tsx:30,37,59,94`, `ui/sidebar.tsx:326`.

Tauri replaces this with the **`data-tauri-drag-region` HTML attribute** or `appWindow.startDragging()`, and the docs warn _"`data-tauri-drag-region` will only work on the element to which it is directly applied"_ ([window customization](https://v2.tauri.app/learn/window-customization/)) — so every drag region needs individual conversion. macOS traffic-light inset is available via `TitleBarStyle::Transparent`; there is no Windows `titleBarOverlay` equivalent (you draw your own controls).

`apps/web/src/lib/windowControlsOverlay.ts:13-23` uses `navigator.windowControlsOverlay` (Chromium-only) — already feature-detected, so it degrades rather than breaks.

### 6. [PLUGIN] Auto-updates — parity exists, but you lose deltas

Today: `electron-updater` ^6.6.2 wrapped at `apps/desktop/src/electron/ElectronUpdater.ts:7`, orchestrated in `apps/desktop/src/updates/DesktopUpdates.ts`. Two channels, `"latest" | "nightly"` (`updateChannels.ts:3-11`). Feed is GitHub Releases via the packaged `app-update.yml` (`DesktopUpdates.ts:288-296`). Poll: 15 s startup delay then every 4 minutes (lines 45-46). Install stops every backend in the pool then calls `quitAndInstall({ isSilent: true, isForceRunAfter: true })` (lines 475-485). Linux auto-update requires the AppImage build (line 237-239).

**Differential updates are in use**: CI publishes `.blockmap` files alongside every artifact (`.github/workflows/release.yml:563-599`), and the code selectively calls `setDisableDifferentialDownload` only when an arm64 host runs an x64 build (`DesktopUpdates.ts:243-245`).

`tauri-plugin-updater` supports AppImage / `app.tar.gz` / NSIS / MSI with mandatory signatures — _"Tauri's updater needs a signature to verify that the update is from a trusted source. This cannot be disabled"_ ([updater docs](https://v2.tauri.app/plugin/updater/)). It documents **no delta/differential support**. For a ~220 MB app shipping nightlies every 3 hours (`release.yml:3-23`, `cron "0 */3 * * *"`), losing blockmap deltas means every user redownloads the full artifact every time. **Migrating to Tauri could plausibly increase total bytes transferred to users even though the artifact is smaller.** This deserves emphasis — it partially or wholly cancels the headline size win.

### 7. [PLUGIN] Straightforward parity items

| Electron usage                                                                                                                       | Tauri v2 equivalent                                                                                                                                                                                                      |
| ------------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `dialog.showOpenDialog` / `showMessageBox` / `showErrorBox` (`ElectronDialog.ts`)                                                    | `tauri-plugin-dialog`                                                                                                                                                                                                    |
| `shell.openExternal` + `clipboard.writeText` (`ElectronShell.ts:8`)                                                                  | `tauri-plugin-opener`, `tauri-plugin-clipboard-manager`                                                                                                                                                                  |
| `nativeTheme` shouldUseDarkColors / themeSource / `on("updated")` (`ElectronTheme.ts:33-53`)                                         | `Window::theme()`, `set_theme`, `on_theme_changed`                                                                                                                                                                       |
| `safeStorage.encryptString/decryptString` (`ElectronSafeStorage.ts:67-80`)                                                           | `tauri-plugin-stronghold`, or a Rust keyring crate. **No first-party OS-keychain plugin is listed** on [v2.tauri.app/plugin/](https://v2.tauri.app/plugin/) — this is a gap requiring a third-party crate.               |
| `requestSingleInstanceLock` + `second-instance` (`DesktopClerk.ts:116-130`)                                                          | `tauri-plugin-single-instance`                                                                                                                                                                                           |
| Window bounds persistence (`DesktopWindow.ts:344-429`)                                                                               | `tauri-plugin-window-state`                                                                                                                                                                                              |
| `electron-store` settings                                                                                                            | `tauri-plugin-store`                                                                                                                                                                                                     |
| `Menu.setApplicationMenu` / `buildFromTemplate().popup()` (`ElectronMenu.ts`, `DesktopApplicationMenu.ts:132-202`)                   | Tauri v2 built-in `Menu`/`Submenu`/`PredefinedMenuItem`. Mostly parity; `nativeImage.createFromNamedImage("trash")` (`ElectronMenu.ts:116-136`) and zoom-factor-corrected popup positioning (lines 100-110) need rework. |
| Filesystem watching — `fs.watch` at `apps/server/serverSettings.ts:520`, `keybindings.ts:588`, `diagnostics/TraceDiagnostics.ts:523` | Runs in the Node sidecar; unchanged. (`tauri-plugin-fs` does offer `watch`/`watchImmediate` with a `watch` Cargo feature, if ever needed.)                                                                               |
| Tray icon, global shortcuts, `powerMonitor`                                                                                          | **Not used.** Grepped `apps/desktop/src` — no `Tray`, `globalShortcut`, or `powerMonitor`. Nothing to port.                                                                                                              |

### 8. [OK — the crucial non-blocker] Child-process spawning of agent CLIs

The app spawns `codex app-server` (JSON-RPC over stdio, `apps/server/src/provider/Layers/CodexSessionRuntime.ts:715-757` → `packages/effect-codex-app-server/src/protocol.ts:354,386`), `cursor-agent acp` (`CursorProvider.ts:408-424,946-961` via `packages/effect-acp`), `claude -p --output-format json` (`apps/server/src/textGeneration/ClaudeTextGeneration.ts`) plus `@anthropic-ai/claude-agent-sdk`'s own managed `claude` subprocess (`ClaudeAdapter.ts:3486`), an `opencode` HTTP server (`apps/server/src/provider/opencodeRuntime.ts`), `git`, `glab`, `az`.

All of these run **inside the Node server**, via Effect's `ChildProcessSpawner` (`apps/server/src/processRunner.ts:300`) — never in the Electron main process. So Tauri's shell-scope allowlist (`shell:allow-execute` / `shell:allow-spawn` with regex arg validators, per the [sidecar docs](https://v2.tauri.app/develop/sidecar/)) only needs to permit spawning _one_ thing: the Node sidecar. The sidecar's own children are unconstrained. This is a real point in favour of feasibility — and simultaneously the reason the Node runtime cannot be eliminated.

---

## 5. Web-engine compatibility risk

Lower than expected, because the renderer already ships as a plain browser app.

**Clean:** no `chrome.*`, no File System Access API (folder picking goes through the IPC bridge — `preload.ts:99`), no `navigator.serial/usb/hid/bluetooth`, no WebCodecs, no `OffscreenCanvas`, no WebGPU, no service worker in production (`msw` is a devDependency only, `apps/web/package.json:63`), no `structuredClone` / `Object.groupBy` / `Promise.withResolvers` / `Intl.Segmenter` / regex lookbehind. xterm.js uses the DOM renderer with only `@xterm/addon-fit` — no WebGL addon, so no GPU-path risk.

**Already guarded:** `document.startViewTransition` is feature-detected (`apps/web/src/components/chat/draftHeroTransition.ts:52`, called at 66); `navigator.windowControlsOverlay` returns null gracefully (`apps/web/src/lib/windowControlsOverlay.ts:13-23`).

**Real QA cost:**

- `-webkit-app-region` (8+ sites, §4.5) — must be replaced, not merely tested.
- `field-sizing: content` at `apps/web/src/components/ui/textarea.tsx:31` — narrow cross-engine support; needs a fallback for WebKit.
- Heavy `::-webkit-scrollbar*` styling throughout `index.css`, with `scrollbar-color` fallback in only a couple of places (1291, 1309). Fine on WKWebView/WebView2, but WebKitGTK behaviour varies.
- Tailwind v4 output relies on cascade layers, `@property`, `color-mix()`; the app also uses `oklch()` (`index.css:118,824,835,867,878`), `@container` (254, 260), `:has()` (1079, 1082), `backdrop-filter` (with an `@supports not` fallback at 762). All current on WKWebView and WebView2 — but this **raises the effective minimum OS version floor**, and on Linux it is a lottery: Tauri's own docs admit _"the diverse nature of the Linux ecosystem means it is very hard to compile accurate information"_ and list Ubuntu 22.04 shipping WebKitGTK 2.36 ([webview versions](https://v2.tauri.app/reference/webview-versions/)).
- Open WebKitGTK bugs in `tauri-apps/tauri` that would land on this app: `#14286` (font-weight offset of 100 on WebKitGTK), `#15656` (child webview renders with incorrect bounds on Ubuntu 26.04 + WebKitGTK 2.52.3), `#5600` (incorrect DPI scaling on Wayland), `#6559` (WebGL context lost), `#10665` (AVIF images not loading), `#14656` (MediaRecorder WebM playback fails on Linux).

Net: the renderer would _run_ on WKWebView/WebView2 with modest fixes. The ongoing cost is a permanent third-engine QA matrix that does not exist today — Electron guarantees one Chromium version everywhere.

---

## 6. Effort estimate

Assumes senior engineers competent in both Rust and the existing Effect-TS codebase. Ranges are wide because Rust rewrites of stateful supervision logic are notoriously variance-heavy.

| #   | Workstream                                                                                                                                                                                | Scope anchor                                                                                     | Estimate                        |
| --- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------ | ------------------------------- |
| 1   | Tauri shell foundation — window creation, titlebar, menus, protocol handler + CSP proxy, theme, dialogs, external open, store, single-instance, window state                              | ~19.7k LOC of Effect-TS main process; most of `apps/desktop/src/{electron,window,app,settings}`  | **2-3 mo**                      |
| 2   | IPC surface — 75 channels → Tauri commands + capabilities/permissions manifests, preserving the `DesktopBridge` contract so `apps/web` is untouched                                       | `apps/desktop/src/ipc/channels.ts` (75 constants), `preload.ts:30-244`                           | **1.5-2.5 mo**                  |
| 3   | Node backend as a sidecar — per-platform Node packaging, `externalBin` target triples, restart/backoff loop, fd3 bootstrap delivery, readiness probing, log rotation, WSL second instance | `DesktopBackendManager.ts`, `DesktopBackendPool.ts`, `DesktopBackendConfiguration.ts`, `wsl/`    | **1.5-2.5 mo**                  |
| 4   | Clerk auth + passkeys — no Tauri SDK, no Tauri passkey addon, Associated Domains entitlements to re-derive                                                                                | `DesktopClerk.ts`, `build-desktop-artifact.ts:718-886`                                           | **2-4 mo**, or blocked on Clerk |
| 5   | Preview / embedded browser + automation — **no Tauri equivalent exists**                                                                                                                  | `preview/Manager.ts` ~3,044 LOC, 20+ CDP methods, screencast, AX tree, Playwright InjectedScript | **4-8 mo**, or drop the feature |
| 6   | Updater + release engineering — Tauri updater + signing keys, and rebuilding CI: 4-target matrix, Apple API-key notarization, Azure Trusted Signing, AppImage, manifest merging           | `.github/workflows/release.yml` (~780 lines), `scripts/build-desktop-artifact.ts` (~2,000 lines) | **1.5-2.5 mo**                  |
| 7   | Cross-engine renderer QA + fixes — drag regions, `field-sizing`, scrollbars, WebKitGTK bugs                                                                                               | §5                                                                                               | **1-2 mo**                      |
| 8   | Regression, parity chase, buffer                                                                                                                                                          | —                                                                                                | **2-3 mo**                      |

**Total: ~15-27 engineer-months.** One engineer: 1.5-2.5 calendar years. A dedicated team of four: roughly 4-7 months of doing nothing else — during which the product ships nightlies every 3 hours and the Electron app keeps moving.

---

## 7. Verdict

**Don't do it.**

It is _technically_ possible for everything except the preview subsystem and Clerk passkeys, which are genuine blockers requiring either a feature cut or a from-scratch native implementation. But the economics are indefensible:

1. **The size problem is not an Electron problem.** ~47-57% of every artifact is application payload — a `node_modules` tree containing `effect` (42.7 MB) and `node-pty` (61.4 MB) unpacked, native `fff` search binaries, and on Windows a _duplicate Linux/glibc tree_ for WSL. None of that shrinks under Tauri.
2. **Tauri makes you re-ship the Node runtime.** Electron is currently doing double duty as browser engine _and_ Node runtime (`ELECTRON_RUN_AS_NODE=1`, `DesktopBackendConfiguration.ts:358`). Removing Electron means adding ~30-45 MB of Node sidecar back, eating a third of the saving.
3. **On Linux you save essentially nothing.** Tauri's AppImage bundles WebKitGTK: Tauri's own docs say 2-6 MB → 70+ MB, and a real Tauri v2 app's AppImage is 79 MB. 218.7 MB → ~216 MB.
4. **You may lose more in updates than you gain in downloads.** Nightlies ship every 3 hours with `electron-updater` blockmap deltas today; `tauri-plugin-updater` documents no delta support.
5. **You'd delete the most differentiated feature.** The CDP-driven in-app browser with element picking, screencast recording, AX-tree snapshots and Playwright-selector automation is exposed as a product surface. WKWebView and WebKitGTK have no DevTools Protocol.

Net: **~15-27 engineer-months to make the download ~30% smaller on macOS and Windows, ~1% smaller on Linux, while losing a flagship feature and the passkey login path.**

### Do this instead (if the real goal is size)

All Electron-side, all cheap, all ordered by payoff:

1. **Stop shipping the Linux/glibc `node_modules` tree inside the Windows installer.** `scripts/build-desktop-artifact.ts:909-914` sets `os: [win32, linux]` on Windows so the WSL backend works offline. That is ~80 MB of the 100 MB Windows-vs-macOS delta. Make WSL support an opt-in post-install download. **Expected: 318 MB → ~235 MB. Days of work, not months.**
2. **Bundle `effect` into `apps/server/dist/bin.mjs`.** 42.7 MB unpacked, externalized only because the WSL backend's plain `node` can't read inside an asar (`build-desktop-artifact.ts:1396-1408`). Ship a bundled entry for the non-WSL path and keep the externalized one for WSL. **Expected: tens of MB.**
3. **Prune `node-pty`.** 61.4 MB unpacked because the npm tarball carries prebuilds for every platform plus full source. Strip to the target triple at stage time — the build script already does per-arch staging for the WSL prebuild (lines 1497-1520).
4. **Drop the `playwright-core` runtime dependency.** 11.9 MB, and `PlaywrightInjectedRuntime.ts:126-141` only extracts one string literal out of `lib/coreBundle.js`. Extract it at build time and inline it.
5. **Narrow `asarUnpack`.** `build-desktop-artifact.ts:1409` unpacks `**/node_modules/**` wholesale. Restrict to what the WSL backend genuinely resolves.
6. **Trim Electron locales** via electron-builder's `electronLanguages`. Small but free.

Items 1-4 plausibly get Windows under ~200 MB and macOS to ~150-165 MB — **matching or beating the projected Tauri outcome, at a few weeks of effort instead of a few years, with zero feature loss and zero new QA matrix.**

If the goal is not size but "Rust in the stack" or "no Chromium", that is a different conversation — but it should be argued on those terms, not on bundle size.

---

## 8. Sources

### t3code source (commit `5719e8a`, `pingdotgg/t3code`)

- Workspace layout: `pnpm-workspace.yaml:1-6`, `AGENTS.md:26-31`, `docs/architecture/overview.md:3`
- Desktop package/deps: `apps/desktop/package.json`; build entry `apps/desktop/vite.config.ts`
- Node-as-backend: `apps/desktop/src/backend/DesktopBackendConfiguration.ts:351-367` (`process.execPath`, `ELECTRON_RUN_AS_NODE: "1"`), `:486` (`wsl.exe`); entry path `apps/desktop/src/app/DesktopEnvironment.ts:189`
- Backend supervision: `apps/desktop/src/backend/DesktopBackendManager.ts`, `DesktopBackendPool.ts`
- IPC: `apps/desktop/src/ipc/channels.ts` (75 channels), `apps/desktop/src/preload.ts:7,12,30-244,99,203-220`
- Preview/CDP: `apps/desktop/src/preview/Manager.ts:243,565,584,599,614,629,648,666,702-836,734,742,811,814,869,979,1023-1045,1256-1263,1702,1764,1807-1814,1835,1916,1983,1990,2137,2143,2305,2307,2331,2332,2335`; `WebviewPreferences.ts:41-42`; `BrowserSession.ts:12,106-121,131-140`; `PlaywrightInjectedRuntime.ts:13-14,126-141,184-214`; `GuestProtocol.ts`
- Windowing: `apps/desktop/src/window/DesktopWindow.ts:184-203,321-339,344-429,432-444,446-492,494-514,684-728`
- Protocol/CSP: `apps/desktop/src/electron/ElectronProtocol.ts:11-13,67-95,107-150,181-199`
- Menus: `apps/desktop/src/electron/ElectronMenu.ts:100-110,116-136,205-258`; `apps/desktop/src/window/DesktopApplicationMenu.ts:132-202`
- Other Electron wrappers: `ElectronSafeStorage.ts:67-80`, `ElectronShell.ts:8`, `ElectronDialog.ts`, `ElectronTheme.ts:33-53`, `ElectronApp.ts:59-65,152-161`
- Updater: `apps/desktop/src/electron/ElectronUpdater.ts:7`; `apps/desktop/src/updates/DesktopUpdates.ts:45-46,220-245,288-296,330-345,349-556,719-724,747-767`; `updates/updateChannels.ts:3-11`
- Clerk: `apps/desktop/src/app/DesktopClerk.ts:1-2,74-83,109-133,116-130`
- Server: `apps/server/package.json` (name `t3`, `node-pty ^1.1.0`, `engines.node`), `apps/server/vite.config.ts`
- PTY: `apps/server/src/terminal/NodePtyAdapter.ts:29-68,114,146-152`; `PtyAdapter.ts`; `Manager.ts:84`; `BunPtyAdapter.ts`
- Process spawning: `apps/server/src/processRunner.ts:300`; `packages/shared/src/shell.ts`; `apps/server/src/provider/Layers/CodexSessionRuntime.ts:715-757`; `packages/effect-codex-app-server/src/protocol.ts:354,386`; `apps/server/src/provider/Layers/CursorProvider.ts:408-424,946-961`; `apps/server/src/provider/Layers/ClaudeAdapter.ts:3486`; `apps/server/src/textGeneration/ClaudeTextGeneration.ts`; `apps/server/src/provider/opencodeRuntime.ts`
- File watching: `apps/server/src/serverSettings.ts:520`; `apps/server/src/keybindings.ts:588`; `apps/server/src/diagnostics/TraceDiagnostics.ts:523`
- Native search: `apps/server/src/workspace/WorkspaceSearchIndex.ts:1` (`@ff-labs/fff-node`)
- Build/packaging: `scripts/build-desktop-artifact.ts:39,582,718-760,771-799,832-886,888-928,898-914,946-958,1392-1440,1396-1409,1424-1440,1497-1520,1588`
- CI: `.github/workflows/ci.yml:35,38-41`; `.github/workflows/release.yml:3-23,255-300,320-348,419-479,484-498,517-538,544-556,563-599,701-710,741-780`
- Renderer: `apps/web/package.json`; `apps/web/src/env.ts:6-8`; `apps/web/src/hostedPairing.ts:34`; `apps/web/src/contextMenuFallback.ts`; `apps/web/src/index.css:1,117,118,254,260,362,762,824,835,867,878,1017,1025,1079,1082,1291,1309`; `apps/web/src/components/ThreadTerminalDrawer.tsx:2,21`; `apps/web/src/components/ui/textarea.tsx:31`; `apps/web/src/components/chat/draftHeroTransition.ts:52,66`; `apps/web/src/lib/windowControlsOverlay.ts:13-23`; `apps/web/src/browser/HostedBrowserWebview.tsx:21-34,203-235`; `apps/web/src/components/chat/PanelLayoutControls.tsx:30,37,59,94`; `apps/web/src/components/DiffPanel.tsx:532,715`; `apps/web/src/components/ui/sidebar.tsx:326`; `apps/web/src/components/ChatView.tsx:5523`; `apps/web/src/components/chat/ExpandedImageDialog.tsx:51`
- Note conventions: `.plans/README.md`, `.plans/19-remote-endpoints-hosted-static.md`, `docs/architecture/overview.md`

### Measured artifact sizes (primary, via `gh` / npm registry / nodejs.org, 2026-07-26)

- `gh release view v0.0.28 --repo pingdotgg/t3code` and `v0.0.29-nightly.20260725.899`
- `gh api repos/electron/electron/releases/tags/v41.5.0`
- `gh release view --repo tw93/Pake` (v3.15.1) + `gh api repos/tw93/Pake/contents/src-tauri/Cargo.toml` (confirms `tauri = "2.10.2"`)
- `registry.npmjs.org` `dist.unpackedSize` for `node-pty@1.1.0`, `effect@4.0.0-beta.78`, `playwright-core@1.60.0`, `@anthropic-ai/claude-agent-sdk@0.3.170`, `@ff-labs/fff-bin-*@0.9.4`, `@pierre/diffs`
- `nodejs.org/dist/v24.13.1/` `Content-Length` for darwin-arm64/darwin-x64/linux-x64/win-x64

### Official Tauri v2 docs

- App size: https://v2.tauri.app/concept/size/
- Sidecar / `externalBin` / stdio streaming: https://v2.tauri.app/develop/sidecar/
- Webview versions per platform: https://v2.tauri.app/reference/webview-versions/
- Windows installer + `webviewInstallMode` size table: https://v2.tauri.app/distribute/windows-installer/
- AppImage ("2-6 MB range to 70+ MB"): https://v2.tauri.app/distribute/appimage/
- Updater (mandatory signing, no deltas documented): https://v2.tauri.app/plugin/updater/
- Plugin catalogue: https://v2.tauri.app/plugin/
- File system plugin (`watch`/`watchImmediate`, scope model): https://v2.tauri.app/plugin/file-system/
- Window customization / `data-tauri-drag-region`: https://v2.tauri.app/learn/window-customization/
- Core JS API / `convertFileSrc` / `asset:` protocol: https://v2.tauri.app/reference/javascript/api/namespacecore/
- `WebviewBuilder` "Available on crate feature `unstable` only": https://docs.rs/tauri/latest/tauri/webview/struct.WebviewBuilder.html

### tauri-apps GitHub (parity / known limitations)

- `tauri-apps/tauri#10420` — multiwebview (unstable feature) positioning bug
- `tauri-apps/tauri#981` (closed) — "Would a Chrome Backend Be Useful for Tauri?"; `#14963` (open) — "Bundle chromium renderer"
- WebKitGTK: `#14286` font-weight offset; `#15656` child webview bounds on WebKitGTK 2.52.3; `#5600` Wayland DPI scaling; `#6559` WebGL context lost; `#10665` AVIF; `#14656` MediaRecorder WebM
- No results for "pty"/"terminal" in `tauri-apps/tauri` or `tauri-apps/plugins-workspace`; no results for "keyring keychain" in `plugins-workspace`

### Official Electron docs

- `ELECTRON_RUN_AS_NODE`: https://www.electronjs.org/docs/latest/api/environment-variables

### Low-trust corroboration

- `tw93/Pake` release artifact sizes are used as an empirical Tauri v2 floor. This is a third-party app, not a Tauri project claim; it is cited only because Tauri's own size docs give no numbers, and it is corroborated by Tauri's own "2-6 MB → 70+ MB" AppImage statement.
