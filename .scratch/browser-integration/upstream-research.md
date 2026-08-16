# T3 upstream browser integration — desktop versus server/web

Research date: 2026-08-10

## Scope and provenance

Laplus has one configured Git remote, `origin = https://github.com/hoangvu12/laplus`.
Its README identifies the UI source as the separate upstream
[`pingdotgg/t3code`](https://github.com/pingdotgg/t3code). The upstream comparison
below is pinned to current `main` commit
[`78f462c4e18c8ea5e5037dc916389a3b72246025`](https://github.com/pingdotgg/t3code/tree/78f462c4e18c8ea5e5037dc916389a3b72246025)
(`chore(release): prepare v0.0.33`, 2026-08-10). Local repository identity is
supported by Laplus's [README](https://github.com/hoangvu12/laplus/blob/main/README.md)
and the configured remote can be reproduced with `git remote -v`.

This report uses upstream source and history as primary evidence. “Server/web”
means the ordinary browser client served by T3's Node server, including a
headless `t3 serve` deployment. It does not mean the web UI hosted inside the
Electron desktop shell.

## Short answer

T3 upstream has a deeply integrated browser **in the desktop app only**. It is
an Electron Chromium `<webview>` with browser chrome, multiple collaborative
tabs, responsive viewport controls, DevTools, annotation, screenshots,
recording, picture-in-picture, and agent automation.

The server participates, but it is not a browser runtime. It stores and
broadcasts per-thread preview-tab state and brokers MCP automation requests to a
connected desktop renderer. The ordinary server/web client explicitly reports
that preview is desktop-only and does not mount a browser host. Therefore a
standalone/headless server cannot browse, render, annotate, record, or execute
automation without a live desktop client owning the guest webview.

## Surface and ownership

| Concern     | Desktop app                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             | Server / ordinary web client                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| ----------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Browser UI  | Browser is a right-panel surface with URL chrome, back/forward/reload, multiple tabs, add-browser action, resizing/maximize, responsive device modes, zoom, appearance, annotation and overflow actions. [`PreviewView.tsx`](https://github.com/pingdotgg/t3code/blob/78f462c4e18c8ea5e5037dc916389a3b72246025/apps/web/src/components/preview/PreviewView.tsx), [`PreviewChromeRow.tsx`](https://github.com/pingdotgg/t3code/blob/78f462c4e18c8ea5e5037dc916389a3b72246025/apps/web/src/components/preview/PreviewChromeRow.tsx), [`RightPanelTabs.tsx`](https://github.com/pingdotgg/t3code/blob/78f462c4e18c8ea5e5037dc916389a3b72246025/apps/web/src/components/RightPanelTabs.tsx) | Shared UI knows the Browser surface, but marks it unavailable and explains that previews require desktop. Keyboard preview toggles also show “Preview is desktop-only.” [`PreviewPanel.tsx`](https://github.com/pingdotgg/t3code/blob/78f462c4e18c8ea5e5037dc916389a3b72246025/apps/web/src/components/preview/PreviewPanel.tsx), [`_chat.tsx`](https://github.com/pingdotgg/t3code/blob/78f462c4e18c8ea5e5037dc916389a3b72246025/apps/web/src/routes/_chat.tsx), [`previewStateStore.ts`](https://github.com/pingdotgg/t3code/blob/78f462c4e18c8ea5e5037dc916389a3b72246025/apps/web/src/previewStateStore.ts)                                |
| Runtime     | Electron enables `<webview>` and mounts one Chromium guest per active preview session through `ElectronBrowserHost` / `HostedBrowserWebview`. [`DesktopWindow.ts`](https://github.com/pingdotgg/t3code/blob/78f462c4e18c8ea5e5037dc916389a3b72246025/apps/desktop/src/window/DesktopWindow.ts), [`ElectronBrowserHost.tsx`](https://github.com/pingdotgg/t3code/blob/78f462c4e18c8ea5e5037dc916389a3b72246025/apps/web/src/browser/ElectronBrowserHost.tsx), [`HostedBrowserWebview.tsx`](https://github.com/pingdotgg/t3code/blob/78f462c4e18c8ea5e5037dc916389a3b72246025/apps/web/src/browser/HostedBrowserWebview.tsx)                                                              | No iframe, Playwright daemon, CDP browser, or server-side Chromium substitutes for that guest. `isPreviewSupportedInRuntime()` is exactly the presence of `window.desktopBridge?.preview`; `ElectronBrowserHost` returns `null` outside Electron. [`env.ts`](https://github.com/pingdotgg/t3code/blob/78f462c4e18c8ea5e5037dc916389a3b72246025/apps/web/src/env.ts), [`previewStateStore.ts`](https://github.com/pingdotgg/t3code/blob/78f462c4e18c8ea5e5037dc916389a3b72246025/apps/web/src/previewStateStore.ts)                                                                                                                             |
| Server role | Desktop reports navigation/loading/back-forward state to the server and consumes commands routed back to its webview.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   | Server owns collaborative tab metadata, revisions/epochs and subscriptions, scans local ports, and brokers preview automation over WebSocket. The contract itself says rendering is desktop-owned. [`preview.ts`](https://github.com/pingdotgg/t3code/blob/78f462c4e18c8ea5e5037dc916389a3b72246025/packages/contracts/src/preview.ts), [`PreviewAutomationBroker.ts`](https://github.com/pingdotgg/t3code/blob/78f462c4e18c8ea5e5037dc916389a3b72246025/apps/server/src/mcp/PreviewAutomationBroker.ts), [`Manager.ts`](https://github.com/pingdotgg/t3code/blob/78f462c4e18c8ea5e5037dc916389a3b72246025/apps/server/src/preview/Manager.ts) |

## Desktop capabilities

The desktop preview manager is much more than a URL wrapper:

- Navigation, back/forward, refresh/hard reload, zoom, detached DevTools,
  color-scheme emulation, cookies/cache clearing and crash recovery are exposed
  through the desktop preview IPC boundary.
  [`ipc/methods/preview.ts`](https://github.com/pingdotgg/t3code/blob/78f462c4e18c8ea5e5037dc916389a3b72246025/apps/desktop/src/ipc/methods/preview.ts)
- Responsive preview supports fill, freeform and named device presets; changing
  it changes CSS viewport layout rather than pretending to change the desktop
  user agent.
  [`previewAutomation.ts`](https://github.com/pingdotgg/t3code/blob/78f462c4e18c8ea5e5037dc916389a3b72246025/packages/contracts/src/previewAutomation.ts)
- Human annotation injects a picker preload, identifies DOM/React component
  context, captures the selected region, and can attach or immediately send the
  structured annotation and image to chat.
  [`PickPreload.ts`](https://github.com/pingdotgg/t3code/blob/78f462c4e18c8ea5e5037dc916389a3b72246025/apps/desktop/src/preview/PickPreload.ts),
  [`PreviewView.tsx`](https://github.com/pingdotgg/t3code/blob/78f462c4e18c8ea5e5037dc916389a3b72246025/apps/web/src/components/preview/PreviewView.tsx)
- Automation supports status, open/navigate, responsive resize, appearance,
  semantic snapshot plus PNG, locator/CSS/coordinate click, type, key press,
  scroll, JavaScript evaluation, conditional waiting, and recording. The
  desktop injects a Playwright selector runtime into the existing guest rather
  than launching a separate Playwright browser.
  [`tools.ts`](https://github.com/pingdotgg/t3code/blob/78f462c4e18c8ea5e5037dc916389a3b72246025/apps/server/src/mcp/toolkits/preview/tools.ts),
  [`PlaywrightInjectedRuntime.ts`](https://github.com/pingdotgg/t3code/blob/78f462c4e18c8ea5e5037dc916389a3b72246025/apps/desktop/src/preview/PlaywrightInjectedRuntime.ts),
  [`Manager.ts`](https://github.com/pingdotgg/t3code/blob/78f462c4e18c8ea5e5037dc916389a3b72246025/apps/desktop/src/preview/Manager.ts)
- Screenshot and recording evidence can be written as local artifacts and then
  revealed or copied; picture-in-picture can keep a preview visible outside the
  normal panel.
  [`ipc/methods/preview.ts`](https://github.com/pingdotgg/t3code/blob/78f462c4e18c8ea5e5037dc916389a3b72246025/apps/desktop/src/ipc/methods/preview.ts)

The feature accumulated in several upstream slices: live-owner automation
routing ([`ffae5410`](https://github.com/pingdotgg/t3code/commit/ffae5410)), browser
surface/automation/recording stabilization
([`44fb34ad`](https://github.com/pingdotgg/t3code/commit/44fb34ad)), background
capture and PiP ([`f4c39432`](https://github.com/pingdotgg/t3code/commit/f4c39432)),
and recent sites ([`72d673a8`](https://github.com/pingdotgg/t3code/commit/72d673a8)).

## Agent automation: server broker, desktop executor

Provider sessions receive an authenticated per-session MCP endpoint. The
preview toolkit calls a `PreviewAutomationBroker`, which selects a connected
host, sends an operation over `previewAutomation.connect`, waits for its
response, and can request that the desktop host focus the preview. The web
renderer's `PreviewAutomationHosts` consumes those requests and dispatches them
through desktop preview IPC.

Primary seams:

- MCP endpoint/tool publication:
  [`McpHttpServer.ts`](https://github.com/pingdotgg/t3code/blob/78f462c4e18c8ea5e5037dc916389a3b72246025/apps/server/src/mcp/McpHttpServer.ts)
- Broker:
  [`PreviewAutomationBroker.ts`](https://github.com/pingdotgg/t3code/blob/78f462c4e18c8ea5e5037dc916389a3b72246025/apps/server/src/mcp/PreviewAutomationBroker.ts)
- WebSocket methods:
  [`ws.ts`](https://github.com/pingdotgg/t3code/blob/78f462c4e18c8ea5e5037dc916389a3b72246025/apps/server/src/ws.ts)
- Renderer host/executor:
  [`PreviewAutomationHosts.tsx`](https://github.com/pingdotgg/t3code/blob/78f462c4e18c8ea5e5037dc916389a3b72246025/apps/web/src/components/preview/PreviewAutomationHosts.tsx)

This architecture makes the tab collaborative: the human and agent operate the
same visible browser session and share its cookies/page state. It also creates
the central server-version gap: no connected desktop automation host means the
broker has nowhere to execute.

## Persistence and recent sites

Two different kinds of state should not be conflated:

1. The server's preview state is collaborative, per-thread tab state: tab IDs,
   current navigation/status, viewport, revision and server epoch. It lets
   clients reconnect without confusing stale answers, but it is not a persisted
   browser engine.
2. Recent-site history is client-side Zustand persistence. It stores normalized
   URLs and optional titles per project, strips credentials, coalesces loopback
   aliases, caps history at 50 entries per project and 20 projects, migrates
   malformed persisted data defensively, and suggests entries in the empty
   browser tab alongside configured and discovered local servers.
   [`browserHistoryStore.ts`](https://github.com/pingdotgg/t3code/blob/78f462c4e18c8ea5e5037dc916389a3b72246025/apps/web/src/browserHistoryStore.ts),
   [`PreviewEmptyState.tsx`](https://github.com/pingdotgg/t3code/blob/78f462c4e18c8ea5e5037dc916389a3b72246025/apps/web/src/components/preview/PreviewEmptyState.tsx)

Although the history store lives in the shared web package, preview itself is
runtime-gated to desktop, so ordinary server/web users cannot currently use the
recent-sites UI.

## Security and isolation

Upstream treats preview pages as untrusted content:

- The main Electron app renderer uses `contextIsolation: true`,
  `nodeIntegration: false`, `sandbox: true`, and enables `<webview>` only so the
  controlled preview guest can exist.
- Preview guests use a persistent partition derived from environment scope.
  Electron strips its own product tokens from the user agent and permits only a
  small allowlist: clipboard read/sanitized write, notifications, and
  geolocation. `local-fonts` is deliberately denied as a fingerprinting/data
  exposure risk.
- Guests intentionally set `contextIsolation=false` so the annotation preload
  can see the page's React DevTools hook, but keep `sandbox=true` and
  `nodeIntegration=false`. A `will-attach-webview` handler validates that the
  requested partition is one of T3's preview partitions and force-applies the
  security-critical preferences. This is an explicit tradeoff, not the main
  window's policy.
- Cookies/storage and HTTP cache can be cleared across preview partitions.

Sources:
[`DesktopWindow.ts`](https://github.com/pingdotgg/t3code/blob/78f462c4e18c8ea5e5037dc916389a3b72246025/apps/desktop/src/window/DesktopWindow.ts),
[`WebviewPreferences.ts`](https://github.com/pingdotgg/t3code/blob/78f462c4e18c8ea5e5037dc916389a3b72246025/apps/desktop/src/preview/WebviewPreferences.ts),
[`BrowserSession.ts`](https://github.com/pingdotgg/t3code/blob/78f462c4e18c8ea5e5037dc916389a3b72246025/apps/desktop/src/preview/BrowserSession.ts).

## Exact desktop/server feature gaps

The ordinary server/web version has the chat and right-panel shell, server tab
state, port discovery, and the automation broker protocol. It does **not** have:

- a rendered browser guest or any alternative remote browser stream;
- navigation/history/cookies as a working browser session;
- DevTools, device viewport emulation, zoom, color-scheme emulation, data/cache
  clearing, or PiP;
- human element annotation and annotation screenshots;
- screenshot/recording artifact capture;
- an automation executor for MCP preview tools.

Consequently, T3's upstream implementation is not “browser integrated into the
server.” It is “browser integrated into desktop, with server-coordinated shared
state and agent control.” To give headless/web deployments parity, upstream
would need a new browser host (for example a server-side Chromium/Playwright
worker plus a streamed or proxied visual surface), an ownership/routing policy,
remote artifact delivery, authentication and network-egress controls, and a
replacement for Electron-specific annotation/DevTools/PiP behavior.

## Implication for Laplus comparison

Upstream is a useful parity target only after keeping three layers separate:

1. **UI/state parity:** panel chrome, tabs, recent sites, responsive controls.
2. **Desktop host parity:** a secure guest webview plus IPC for navigation,
   annotation, DevTools, capture, recording and automation.
3. **Server/agent parity:** collaborative tab registry, request broker, MCP
   tools and live-host routing.

Having the first two does not make browser automation available to agents; the
third requires a producer (MCP/provider integration). Having server contracts
and a broker does not make the headless server a browser; an executing host must
remain connected.
