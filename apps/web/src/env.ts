/**
 * True when running inside the Electron preload bridge, false in a regular
 * browser. The preload script sets `window.nativeApi` via contextBridge before
 * any web-app code executes, so this is reliable at module load time.
 *
 * **In laplus this is always false.** The shell is Tauri, which injects no such
 * bridge — see `desktopShell.ts`. The single remaining reader is
 * `browser/ElectronBrowserHost`, whose `<webview>` host has no WebView2
 * equivalent; the flag is kept there so that component states its own
 * precondition rather than pretending to be reachable. For "is this laplus's
 * own window", use `isDesktopShell`.
 */
export const isElectron =
  typeof window !== "undefined" &&
  (window.desktopBridge !== undefined || window.nativeApi !== undefined);
