# 27 — The topbar is not the titlebar

**What to build:** one bar at the top of the window instead of two.

lightcode's window has the operating system's titlebar above the application's
own topbar. t3code has only the topbar, with the window controls drawn into its
right-hand end — so the app starts about thirty pixels higher and looks like one
piece rather than a web page in a frame.

**Status:** done

**Found by:** ticket 23, once a person looked at the window. Dragging the
topbar does nothing and dragging the OS titlebar works, which is correct for
what was built and is not what upstream feels like.

## Why it is not a small change

Three things have to happen together, and any one of them alone makes the
application worse rather than better:

- **The window has to be frameless** (`decorations: false`). On its own this
  removes the only way to close, minimise or move the window.
- **The UI has to draw the window controls.** Upstream draws them only when it
  can see Electron's preload bridge — `apps/web/src/env.ts` gates on
  `window.desktopBridge` or `window.nativeApi`. So either enough of that bridge
  is faked for the titlebar path (and the UI then believes it is in Electron
  everywhere else, and calls into a bridge that is not there), or the controls
  are drawn by the shell instead. Neither is a line of code.
  `t3code-electron-to-tauri-migration.md` §5 is the survey: Electron uses
  `titleBarOverlay` on Windows, and **Tauri has no equivalent — you draw your
  own**.
- **The drag regions have to work**, because a frameless window with an inert
  topbar cannot be moved at all. That means an injected script calling
  `startDragging` and a capability granting the page that command. One was
  written during ticket 23 and removed as unverifiable and out of scope; this is
  the ticket that makes it necessary rather than decorative, and it should come
  back with a person watching a window rather than on faith.

## What to settle before starting

Whether **any** of this is v1. The spec puts the custom-titlebar drag-region
question out of scope as one that "only matters off Windows", and the window is
completely usable as it is. This is polish on the most visible surface in the
application, which is a real argument for it and not the same as a defect.

The migration document costs the whole window-chrome area at months, but that is
for parity across three platforms with menus, theming and window state. One bar
on Windows is a much smaller thing hiding inside that number, and worth scoping
on its own before it inherits the estimate.

## Comments

### 2026-07-27 — triage. Not v1

`wontfix`, and the label is doing a specific job here: this ticket describes
something real and visible, so left open it reads as an outstanding defect in
the most obvious surface of the application. It is not one. The window is
completely usable, the spec already puts the drag-region question out of scope,
and the three changes named above have to land together or each makes the
application worse on its own.

The deciding argument is the second bullet. Faking `window.desktopBridge` to get
upstream's titlebar path to draw would make the UI believe it is in Electron
_everywhere else_ — every other call behind that gate would go to a bridge that
is not there. That is a large, diffuse risk taken on for thirty pixels.

**This is "not v1", not "never".** Reopen it when there is a person willing to
watch a window while it is built, and take the drag-region script that ticket 23
wrote and removed as the starting point rather than rediscovering the need for
it. Nothing else in the tracker depends on this, so it can come back at any time
without unblocking anything.

### 2026-07-28 — reopened. Asked for, and two findings

Off `wontfix` because the maintainer asked for it first in a session spent using
the application — it was the first of three things reported, ahead of two real
defects. "Not v1, not never" was the right call in July; this is the reopening it
described, and the person willing to watch a window is here.

The triage argument above is not weakened and should still be read first. Two
things have changed under it since:

- **The second bullet is worse than written.** It says Tauri has no
  `titleBarOverlay` equivalent, which is true, and the consequence is sharper
  than "you draw your own": WebView2 exposes no Window Controls Overlay API at
  all, so `navigator.windowControlsOverlay` is absent, the `.wco` class never
  applies, and every `env(titlebar-area-*)` inset in the UI resolves to its
  fallback. Upstream's Windows titlebar path cannot be switched on here even with
  the bridge faked — it is waiting for a browser API that will not arrive.

- **The second bullet is also cheaper than written.** Its objection is to faking
  `window.desktopBridge`, and that objection is correct and unchanged: `isElectron`
  also selects hash history (`apps/web/src/main.tsx:25`) and gates every other
  Electron-only feature. But the choice is not "fake the bridge or draw from the
  shell". A third option exists now that did not when this was triaged: a
  separate `isDesktopShell`, keyed on Tauri's own injected global, used _only_ by
  the titlebar path. `apps/web` became a fork this repository owns outright in
  ticket 32 (`docs/adr/0014`), so adding a module there is no longer a divergence
  to be defended.

Unchanged and still load-bearing: the drag regions. `-webkit-app-region: drag` is
inert in WebView2, so the topbars upstream marks as draggable do nothing. It
needs `data-tauri-drag-region` and a capability granting
`core:window:allow-start-dragging` — and because the window is pointed at
`http://127.0.0.1:4773` rather than a Tauri scheme (ADR-0010), that capability
has to name the loopback origin as a remote URL. Ticket 23's removed script is
still the starting point.

Nothing else in the tracker depends on this.

### 2026-07-28 — done. One bar, and it drags

The window has no frame. `decorations(false)` in the shell, the three buttons in
`apps/web/src/components/DesktopWindowControls.tsx`, and
`capabilities/titlebar.toml` to let a page served over loopback move the window
it is in — the three things this ticket said had to land together, landed
together.

**Ticket 23's removed script did not come back, and should not have.** The
reopening comment above names it as the starting point; it is not one. Tauri
ships that script itself — `tauri::window::plugin` injects a `drag.js` into every
webview, on remote URLs too — so the topbars needed an _attribute_
(`data-tauri-drag-region`) rather than a script. What ticket 23 could not verify
was never the JavaScript; it was the capability, and that is the part this
session could only settle by watching a window.

Two things found while building it, both of which changed the shape of the fix:

- **The port is a wildcard, because the port is a choice.** `--port` and
  `LAPLUS_PORT` both move it, and a capability naming 4773 is a window that
  cannot be dragged for anyone who overrode it — silently, since a denied
  command looks exactly like a dead button. `http://127.0.0.1:*` covers it; the
  host is deliberately not wildcarded, and
  `the_capability_covers_the_origin_the_server_serves` asserts both halves
  against the address `Server::http_url` actually produces.

- **The buttons fill the bar, which upstream's cannot.** Electron's
  `titleBarOverlay` is 40px in a 52px topbar, so upstream's caption glyphs sit
  six pixels above the controls beside them and it has no way to fix that — the
  operating system draws them. Ours are a component, so they are the bar's
  height, which is also Windows' own rule for caption buttons. That was the
  first thing reported when the window was looked at.

The spacing then needed one more pass, and the reason is worth keeping: the
topbar's right-hand padding and `--workspace-controls-right` are **derived in
that order**. Setting the padding to the buttons' width puts the panel toggles
_inside_ the band the header reserves for them, and the two end up a pixel
apart. `titlebar-boxes.mjs` is what said so — the three things in that corner
are positioned by three different rules and which one is wrong cannot be seen by
looking.

Verified against the real window rather than a browser, by measurement:

|                                       |                                         |
| ------------------------------------- | --------------------------------------- |
| drag, chat topbar                     | asked 40,40 → moved 40,40               |
| drag, sidebar chrome                  | asked −40,−40 → moved −40,−40           |
| minimise / maximise / restore / close | each one, from each state               |
| double-click the topbar               | maximised ↔ restored                    |
| right edge                            | resizes, 1442 → 1542                    |
| maximised                             | 1920x1032 at 0,0 — clear of the taskbar |

`tools/ui-driver/window-{find,shot,drag,click}.ps1` are what did it, and the
README there has the recipe. `window-find.ps1` is the one to read first: this
session spent a run of green results clicking a window sixteen pixels wide,
because `Get-Process().MainWindowHandle` returns tao's helper window once laplus
is minimised.

Still true, and still the reason none of this was cheap: WebView2 has no Window
Controls Overlay, and `isDesktopShell` is not `isElectron`.
