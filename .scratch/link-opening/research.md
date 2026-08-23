# Link opening — why clicked links never reach the system browser

Research date: 2026-08-23

## Scope and provenance

The user report: "The links don't work it seems? It should open them in the
browser?" — clicking a link in the laplus UI does nothing instead of opening the
OS browser.

Primary evidence is this repository's own source, followed to file and line.
Dependency behaviour (Tauri/wry/WebView2 new-window handling) is cited from the
pinned sources in the local cargo registry, with the pins taken from
[Cargo.lock](../../server/Cargo.lock#L3944-L3946) (`tauri 2.11.5`) and
[Cargo.lock](../../server/Cargo.lock#L5484-L5486) (`wry 0.55.1`). Git history was
checked but has little to say: the tree begins at a single squashed commit
(`2c9487a "Found the repository on what laplus actually builds"`), so nothing
recent _broke_ link opening — every path below was born in its current state.

Upstream [`pingdotgg/t3code`](https://github.com/pingdotgg/t3code) `main` was
fetched for comparison only; those findings are labelled **upstream comparison,
secondary evidence**.

## Short answer

Two independent things are missing, and they fail in different runtimes:

1. **The shell half of link opening does not exist.** The renderer opens every
   external link with `target="_blank"` or `window.open(...)`, which inside a
   Tauri/WebView2 window becomes a WebView2 `NewWindowRequested`. wry answers
   that request by **cancelling it silently** when no handler is installed
   ([wry source, see F3](#f3)), and laplus-shell installs none. Upstream's
   Electron shell had a main-process half doing this job —
   `setWindowOpenHandler` → `shell.openExternal` — and the Tauri port never
   reimplemented it. **Every external-link click in the desktop window dies
   silently. This is almost certainly what the user hit** (**PROVEN**, by code
   construction).

2. **Every explicit "open in system browser" affordance in the UI is gated on an
   Electron bridge that laplus deliberately does not have.**
   `isPreviewSupportedInRuntime()` is `Boolean(window.desktopBridge?.preview)`
   ([previewStateStore.ts:451-454](../../apps/web/src/previewStateStore.ts#L451-L454)),
   and `window.desktopBridge` stays `undefined` everywhere in laplus — it is
   upstream's Electron preload object ([ADR 0021](../../server/docs/adr/0021-the-page-commands-the-shell-through-a-named-list.md),
   [desktopShell.ts:4-9](../../apps/web/src/desktopShell.ts#L4-L9)). So the
   right-click "Open in system browser" menu on chat links never appears, in the
   desktop app _and_ in a plain browser (**PROVEN**).

In a plain browser via `pnpm dev`, ordinary left-clicks on markdown links do
work (the browser handles `target="_blank"` itself); the bug is specific to the
Tauri window plus the dead affordances that would otherwise have offered a way
out.

## The two runtime contexts

|                                                  | (a) Tauri desktop window (`pnpm app`, release build)                                     | (b) plain browser via `pnpm dev` + `pnpm dev:server`                                                       |
| ------------------------------------------------ | ---------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------- |
| Left-click markdown link                         | **Nothing happens.** WebView2 `NewWindowRequested` cancelled by wry default (**PROVEN**) | Opens a new tab (normal browser behaviour) (**SUSPECTED** — standard behaviour, not runtime-verified here) |
| Right-click chat link → "Open in system browser" | Menu never appears — gate is always false (**PROVEN**)                                   | Same (**PROVEN**)                                                                                          |
| Terminal URL Ctrl+Click                          | Falls through to `window.open` → silent no-op (**PROVEN** by construction)               | Opens a new tab (**SUSPECTED** — user-gesture popup, should pass blockers)                                 |
| URLs inside tool output / diffs / work entries   | Plain text, never clickable, any runtime (**PROVEN**)                                    | Same (**PROVEN**)                                                                                          |

## Findings

### F1 — Markdown links: left-click relies entirely on webview default handling

[ChatMarkdown.tsx:1386-1441](../../apps/web/src/components/ChatMarkdown.tsx#L1386-L1441)
renders every non-file external anchor with `target="_blank"` and no left-click
handler of its own:

```tsx
target={isSameDocumentLink ? undefined : "_blank"}
rel={isSameDocumentLink ? undefined : "noopener noreferrer"}
onClick={(event) => {
  onClick?.(event);
  if (isSameDocumentLink && href) {
    handleMarkdownFragmentClick(event, href);
  }
}}
```

([lines 1398-1405](../../apps/web/src/components/ChatMarkdown.tsx#L1398-L1405))

So a plain left click is delegated to the host: fine in a real browser, fatal in
a Tauri window (F3). There is no JS fallback anywhere that calls
`api.shell.openExternal` on left-click — that call exists only in the
context-menu path ([line 1425](../../apps/web/src/components/ChatMarkdown.tsx#L1425)).

Coverage of link kinds:

- `http(s)` markdown links, GFM autolink literals (bare URLs — `remarkGfm` is
  enabled at [ChatMarkdown.tsx:173-177](../../apps/web/src/components/ChatMarkdown.tsx#L173-L177))
  and `<autolinks>` all become the same `target="_blank"` anchor → same fate.
- `#fragment` links stay same-document and work everywhere
  ([1391-1404](../../apps/web/src/components/ChatMarkdown.tsx#L1391-L1405)).
- Relative/file links are rewritten to `MarkdownFileLink` chips that open in an
  editor, not the browser
  ([1469-1491](../../apps/web/src/components/ChatMarkdown.tsx#L1469-L1491)) — out of scope for this bug.
- `mailto:` survives `defaultUrlTransform`
  ([1288-1290](../../apps/web/src/components/ChatMarkdown.tsx#L1288-L1290)) and renders as
  `target="_blank"` too, so in the desktop window it hits the same cancelled
  new-window request (**SUSPECTED** — not separately verified whether WV2 routes
  `mailto:` through `NewWindowRequested` or a launch event).
- If the preview mutation rejects, the failure is only reported where it can be:
  [1416-1424](../../apps/web/src/components/ChatMarkdown.tsx#L1416-L1424) logs via
  `reportMarkdownActionFailure`. But that whole path is unreachable today (F2),
  so rejection handling is moot.

### F2 — The "Open in system browser" context menu is unreachable in every runtime

The anchor's `onContextMenu` bails before showing anything:

```tsx
const canOpenInPreview = Boolean(threadRef) && isPreviewSupportedInRuntime();
...
onContextMenu={(event) => {
  if (!canOpenInPreview || !href || !faviconHost) return;
```

([ChatMarkdown.tsx:1393](../../apps/web/src/components/ChatMarkdown.tsx#L1393),
[1406-1408](../../apps/web/src/components/ChatMarkdown.tsx#L1406-L1408))

and `isPreviewSupportedInRuntime()` is:

```ts
return Boolean(window.desktopBridge?.preview);
```

([previewStateStore.ts:451-454](../../apps/web/src/previewStateStore.ts#L451-L454))

`window.desktopBridge` is upstream's Electron preload bridge. laplus has no
preload and defines it nowhere; ADR 0021 records that it deliberately stays
`undefined` ([ADR 0021, lines 75+](../../server/docs/adr/0021-the-page-commands-the-shell-through-a-named-list.md),
[desktopShell.ts:4-18](../../apps/web/src/desktopShell.ts#L4-L18), typing only at
[vite-env.d.ts:15-20](../../apps/web/src/vite-env.d.ts#L15-L20)). Therefore
`canOpenInPreview` is always false, and the menu defined in
[externalLinkContextMenu.ts:17-21](../../apps/web/src/components/chat/externalLinkContextMenu.ts#L17-L21)
("Open in integrated browser" / "Open in system browser" / "Copy Link") never
renders — including its working `api.shell.openExternal` leg at
[ChatMarkdown.tsx:1425](../../apps/web/src/components/ChatMarkdown.tsx#L1425). **PROVEN.**

The same dead gate shows up as a symptom elsewhere: pressing the preview toggle
inside laplus's own desktop window says _"Preview is desktop-only. Open laplus
in the desktop app…"_ — while already being the desktop app
([\_chat.tsx:115-124](../../apps/web/src/routes/_chat.tsx#L115-L124)). **PROVEN.**

### F3 — The Tauri shell cancels every new-window request, silently (root cause)

The shell builds one window and sets no handlers:

```rust
tauri::WebviewWindowBuilder::new(app, "main", tauri::WebviewUrl::External(url))
    .title("laplus")
    ...
    .decorations(false)
    .build()?;
```

([main.rs:205-216](../../server/crates/laplus-shell/src/main.rs#L205-L216)) — no
`.on_new_window(...)`, no `.on_navigation(...)`.

Tauri defaults both to `None` (`new_window_handler: None`,
tauri-2.11.5 `src/webview/mod.rs:354,433`; only `.on_new_window()` sets it,
`mod.rs:585-590`). With no handler, wry registers no `new_window_req_handler`,
and its WebView2 `NewWindowRequested` listener falls into:

```rust
} else {
  args.SetHandled(true)?;
}
```

wry-0.55.1 `src/webview2/mod.rs:781-783` (registry source; pin at
[Cargo.lock:5484-5486](../../server/Cargo.lock#L5484-L5486)). `SetHandled(true)`
means **cancelled**: the OS browser is never asked, no error surfaces in the
page, and `window.open` just returns null.

Both trigger paths land there:

- `<a target="_blank">` clicks (every external chat link, F1); and
- `window.open(url, "_blank", "noopener,noreferrer")` — which is what
  `shell.openExternal` actually executes in laplus, because its first leg is
  dead (F4):
  [localApi.ts:29-38](../../apps/web/src/localApi.ts#L29-L38):

```ts
openExternal: async (url) => {
  if (window.desktopBridge) { ... }
  window.open(url, "_blank", "noopener,noreferrer");
},
```

There is also no plugin route around this. The shell registers exactly one
plugin — the updater ([main.rs:142](../../server/crates/laplus-shell/src/main.rs#L142),
[Cargo.toml:21-33](../../server/crates/laplus-shell/Cargo.toml#L16-L40)) — no
`tauri-plugin-opener`, no `tauri-plugin-shell`
([main.rs:153-156](../../server/crates/laplus-shell/src/main.rs#L153-L156) names
only the two network-access commands). `tauri.conf.json` configures no opener
either ([tauri.conf.json:12-17](../../server/crates/laplus-shell/tauri.conf.json#L12-L17)),
and the capabilities grant only titlebar/window commands, network-access
commands, and updater commands
([capabilities/titlebar.toml](../../server/crates/laplus-shell/capabilities/titlebar.toml),
[network-access.toml](../../server/crates/laplus-shell/capabilities/network-access.toml),
[updater.toml](../../server/crates/laplus-shell/capabilities/updater.toml)). So even
if the UI called `@tauri-apps/plugin-opener`, the IPC would refuse it — no
permission exists to grant. **PROVEN** (by construction from pinned dependency
source; the final in-window confirmation is a 30-second manual test, see below).

Note this failure is _silent by design of wry_: unlike the titlebar buttons,
whose refusal at least rejects a promise that
[invokeWindowCommand logs](../../apps/web/src/desktopShell.ts#L100-L117),
`window.open` returns null without throwing, so nothing appears in DevTools
either.

### F4 — `shell.openExternal`'s first leg cannot run; its second leg only works in real browsers

[localApi.ts:29-38](../../apps/web/src/localApi.ts#L29-L38) prefers
`window.desktopBridge.openExternal(url)` — undefined in laplus (F2) — then falls
back to `window.open`. Consequences per runtime:

- Desktop window: fallback executes → F3 cancels it → silent no-op. **PROVEN.**
- Plain browser: `window.open` works (user gesture; `"noopener"` features don't
  block tab creation). Callers treat it as fire-and-forget — e.g. terminal
  drawer catches errors only to print them
  ([ThreadTerminalDrawer.tsx:612-619](../../apps/web/src/components/ThreadTerminalDrawer.tsx#L612-L619)),
  preview swallows them
  ([PreviewView.tsx:241-244](../../apps/web/src/components/preview/PreviewView.tsx#L241-L244)) —
  so even here a blocker-induced null return would look like "nothing happened"
  (**SUSPECTED**, minor).

### F5 — Terminal links: correct gesture wiring, same dead exit

Terminal URLs are detected ([terminal-links.ts:39,169-173](../../apps/web/src/terminal-links.ts#L39)) and activate only on
Ctrl+Click (Cmd on mac) — [isTerminalLinkActivation, terminal-links.ts:259-267](../../apps/web/src/terminal-links.ts#L259-L267);
plain clicks intentionally do nothing. Activation tries preview first
([ThreadTerminalDrawer.tsx:620-627](../../apps/web/src/components/ThreadTerminalDrawer.tsx#L620-L627)), but
`openTerminalLinkInPreview` short-circuits when
`isPreviewSupportedInRuntime()` is false and calls `fallbackToBrowser`
([openTerminalLinkInPreview.ts:47-55](../../apps/web/src/components/preview/openTerminalLinkInPreview.ts#L47-L55))
— i.e. straight into F4/F3 in the desktop window. So: Ctrl+Click works in a
plain browser, does nothing in the desktop window. **PROVEN** by construction.

### F6 — Preview panel affordances are unreachable in laplus entirely

`handleOpenInBrowser` in
[PreviewView.tsx:241-244](../../apps/web/src/components/preview/PreviewView.tsx#L241-L244)
calls `shell.openExternal`, but it is wired only when a live preview tab exists
(`onOpenInBrowser={tabId ? handleOpenInBrowser : undefined}`,
[line 625](../../apps/web/src/components/preview/PreviewView.tsx#L625)), and the guest
webview behind it requires `window.desktopBridge?.preview`
([previewBridge.ts:5-9](../../apps/web/src/components/preview/previewBridge.ts#L5-L9)) —
absent in laplus (F2). `PreviewEmptyState`
opens URLs into the preview panel, not the OS
([PreviewEmptyState.tsx:13,52-56](../../apps/web/src/components/preview/PreviewEmptyState.tsx#L13)).
No reachable open-in-browser button lives here. **PROVEN.**

### F7 — Links inside tool output, diffs, and work entries are not links at all

Tool/work rows render heading and preview as truncated spans
([MessagesTimeline.tsx:2075-2080](../../apps/web/src/components/chat/MessagesTimeline.tsx#L2075-L2080))
and expanded output as a plain `<pre>`
([MessagesTimeline.tsx:2155-2157](../../apps/web/src/components/chat/MessagesTimeline.tsx#L2155-L2157)).
No anchor elements, no handlers — a URL printed by a tool is unclickable text in
both runtimes. Only assistant/user prose goes through `ChatMarkdown`
([MessagesTimeline.tsx:1059,1649,1675](../../apps/web/src/components/chat/MessagesTimeline.tsx#L1059)).
**PROVEN.**

### Upstream comparison (secondary evidence)

The renderer half above matches upstream `main` almost line for line (same
anchor shape, same context-menu gate, `openExternal: (target) =>
api.shell.openExternal(target)`). What upstream has and laplus lacks is the
shell half. In upstream's Electron main process:

```ts
window.webContents.setWindowOpenHandler(({ url }) => {
  if (Option.isSome(ElectronShell.parseSafeExternalUrl(url))) {
    void runPromise(electronShell.openExternal(url));
  }
  return { action: "deny" };
});
window.webContents.on("will-navigate", (event, url) => {
  ... event.preventDefault();
  if (...) void runPromise(electronShell.openExternal(url));
});
```

(upstream `apps/desktop/src/window/DesktopWindow.ts`, fetched 2026-08-23.) Every
new-window request and off-origin navigation is intercepted and handed to the OS;
the `{ action: "deny" }` is safe because `openExternal` already ran. The Tauri
port inherited the renderer's reliance on that interception but never wrote its
equivalent (`on_new_window` + an opener command/plugin). Secondary evidence —
local files above are authoritative.

Git history adds nothing further: single root commit; the only `openExternal`
touch since is `faf6ec5` removing the source-control hosting surface (unrelated
call sites).

## Ranked root causes and fix directions (no fixes implemented)

1. **Shell: no new-window/navigation interception and no opener capability**
   (F3) — breaks every external link in the desktop window. Fix direction: in
   `laplus-shell`, either add `.on_navigation(...)`/`.on_new_window(...)` on the
   builder in [main.rs:200-219](../../server/crates/laplus-shell/src/main.rs#L200-L219)
   routing http(s) (and mailto) to an open-external mechanism, and/or register
   `tauri-plugin-opener` plus a capability granting `opener:allow-open-url` to
   the remote origin `http://127.0.0.1:*` (same `[remote]` shape as
   `titlebar.toml`). Follow the crate's own established pattern (command +
   capability naming loopback) if hand-rolling the command.
2. **UI: "system browser" affordances gated on the absent Electron bridge**
   (F2, F5) — removes the only explicit open-externally controls in _all_
   runtimes, and makes laplus's own window claim to be "not the desktop app".
   Fix direction: re-key the gates — e.g. keep integrated-browser-preview
   desktop-gated, but let laplus show the context menu with "Open in system
   browser"/"Copy Link" whenever a local API exists; audit other
   `desktopBridge?.preview` consumers
   ([previewBridge.ts](../../apps/web/src/components/preview/previewBridge.ts), `_chat.tsx`).
3. **`shell.openExternal` has no working desktop leg** (F4) — once (1) lands,
   point the `isDesktopShell` branch of
   [localApi.ts:29-38](../../apps/web/src/localApi.ts#L29-L38) at the shell command
   (`invokeShellCommand`-style, [desktopShell.ts:82-88](../../apps/web/src/desktopShell.ts#L82-L88))
   and surface failures instead of relying on `window.open`'s silent null.
4. **Plain-text URLs in tool output are not clickable** (F7) — design gap rather
   than regression; lowest priority, any fix is new linkification work.

## How to verify at runtime

Context (a) — the desktop window (expected to reproduce the bug):

1. `pnpm build:web`, then `cargo run -p laplus-shell` from `server/`.
2. In any thread, send yourself a message containing
   `[example](https://example.com)` and click it. Expected today: nothing
   happens, with no console error (silent by construction, F3).
3. Ctrl+Click a URL printed in the terminal drawer. Same expectation.
4. Right-click the link. Expected today: the webview's default menu (or
   nothing custom) — no "Open in system browser" item (F2).

Context (b) — plain browser (expected to mostly pass, isolating the shell):

1. `pnpm dev:server`, `pnpm dev`, open the printed localhost URL in Chrome.
2. Click the same link → new tab should open (F1's browser-default leg).
3. Ctrl+Click a terminal URL → new tab (F5's fallback leg).
4. Right-click the link → still no custom menu (confirms F2 is
   runtime-independent).

With `server/tools/ui-driver/` (per its README, a headless Chrome against a
running laplus — `node tools/ui-driver/probe-boot.mjs <url>`): the driver shares
an origin with the served UI, so it can assert the _page-side_ facts — anchors
carry `target="_blank" rel="noopener noreferrer"`, no element intercepts their
clicks, and `isPreviewSupportedInRuntime()` is false (no custom context menu).
It **cannot** exercise the Tauri window's `NewWindowRequested`, because that
fires in the WebView2 host, not the page — the cancellation is only observable
in the real window, which is why step 2 of context (a) above is the decisive
manual check. After any fix, repeat it and confirm example.com opens in the
default browser.

## Addendum, 2026-08-23: fixes landed

**F3 (root cause) — fixed in laplus-shell.** The builder now installs both
halves upstream's Electron shell owned ([main.rs](../../server/crates/laplus-shell/src/main.rs),
window): on_new_window hands every allowlisted request to the operating
system's default handler and denies it (a second webview of this application is
nobody's ask), and on_navigation sends a cross-origin http(s) navigation of
the app's own window to the system browser instead of letting the page replace
itself. The allowlist (externally_openable) is http/https/mailto — an
allowlist rather than a blocklist because these URLs come from model output;
ile://, custom schemes and websockets refuse quietly. Deliberately no
auri-plugin-opener: the plugin exists to expose opening _to the page_
through IPC permissions, which ADR-style minimal linking argues against; what
remains is one well-known launcher per platform spawned with the URL as a
single argument, no shell parsing involved. Zero dependency change.

This alone fixes left-clicks on chat links (F1), terminal Ctrl+Click's browser
fallback (F5), and preview "open in browser" in the desktop window — they all
travel as new-window requests.

**F2 — fixed in the UI.** The external-link context menu now appears whenever
there is an http(s) link to act on, in both runtimes; only the "Open in
integrated browser" item is gated on isPreviewSupportedInRuntime()
([externalLinkContextMenu.ts](../../apps/web/src/components/chat/externalLinkContextMenu.ts)
gained a previewAvailable option defaulting to true;
[ChatMarkdown.tsx](../../apps/web/src/components/chat/ChatMarkdown.tsx) passes
its computed gate through). The other consumers of that function keep their
semantics — laplus genuinely has no integrated browser webview anywhere, so
those gates are correct, not dead.

**Left as-is, deliberately:** localApi.shell.openExternal's window.open
fallback (now a working leg everywhere, since requests reach the shell); the
"Preview is desktop-only" wording in \_chat.tsx; plain-text URLs in tool
output (F7, design gap).

Verification: cargo test -p laplus-shell (9 tests, including the new scheme
allowlist test), full apps/web suite (1722), typecheck, lint, clippy clean for
the shell crate. Rebuilding `apps/web/dist` first was required — the embedded
bundle had gone stale against 0.1.9, which tripped
he_version_reported_is_the_one_the_ui_compares_against exactly as designed.
The decisive manual check remains: run the window, click a link, see the OS
browser open — the cancellation this fix removes was never observable outside
the WebView2 host.
