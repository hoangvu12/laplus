# 27 — The topbar is not the titlebar

**What to build:** one bar at the top of the window instead of two.

lightcode's window has the operating system's titlebar above the application's
own topbar. t3code has only the topbar, with the window controls drawn into its
right-hand end — so the app starts about thirty pixels higher and looks like one
piece rather than a web page in a frame.

**Status:** needs-triage

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
