/**
 * laplus's own window, told apart from a browser tab.
 *
 * **This is deliberately not `isElectron`.** That flag answers "is there an
 * Electron preload bridge on this page", and every call that goes through
 * `window.desktopBridge` or `window.nativeApi` is gated on it. This webview has
 * no such bridge, so a page that claimed to be Electron would point all of
 * those at something that is not there. That is ticket 27's deciding argument,
 * and it still holds.
 *
 * What this flag claims instead is narrow and true: the page is being shown by
 * a Tauri window. `isTauri` is Tauri's own — defined on `window` by an
 * initialization script that runs before any page script, unconditionally and
 * on remote URLs too (`tauri::manager::webview`, the first entry in
 * `all_initialization_scripts`). laplus is served over `http://127.0.0.1:4773`
 * rather than from a Tauri scheme (ADR-0010), so "on remote URLs too" is the
 * part that matters, and it is why this is readable at module load exactly as
 * `isElectron` is.
 *
 * The one thing the UI does with it is draw its own window controls, because
 * nothing else will: WebView2 exposes no Window Controls Overlay, so
 * `navigator.windowControlsOverlay` is absent, the `.wco` class never applies,
 * and every `env(titlebar-area-*)` inset in this UI resolves to its fallback.
 * Upstream's Windows titlebar path is waiting for a browser API that will not
 * arrive here.
 */

/** The `plugin:window` commands `capabilities/titlebar.toml` grants this page. */
type WindowCommand = "minimize" | "toggle_maximize" | "close" | "is_maximized";

interface TauriInternals {
  invoke(command: string, payload?: unknown): Promise<unknown>;
}

interface TauriWindow {
  readonly isTauri?: boolean;
  readonly __TAURI_INTERNALS__?: TauriInternals;
}

function tauri(): TauriInternals | null {
  if (typeof window === "undefined") {
    return null;
  }
  return (window as unknown as TauriWindow).__TAURI_INTERNALS__ ?? null;
}

export const isDesktopShell =
  typeof window !== "undefined" && (window as unknown as TauriWindow).isTauri === true;

/**
 * Ask the shell to do something to its window.
 *
 * Rejections are reported rather than swallowed. The way this fails in practice
 * is a missing entry in `capabilities/titlebar.toml`: the command is denied,
 * the promise rejects, and a button that looks fine does nothing at all — which
 * is the single most expensive failure mode this file has, because there is
 * nothing to see. One console line naming the command is the difference between
 * a five-minute fix and an afternoon.
 */
export async function invokeWindowCommand(command: WindowCommand): Promise<unknown> {
  const internals = tauri();
  if (!internals) {
    return undefined;
  }

  try {
    return await internals.invoke(`plugin:window|${command}`);
  } catch (error) {
    console.error(
      `laplus: the desktop shell refused plugin:window|${command}. ` +
        `If this is "not allowed", the capability granting it to ` +
        `${window.location.origin} is missing or names another origin.`,
      error,
    );
    return undefined;
  }
}

/** Whether the window is maximised, for the middle button's two shapes. */
export async function isWindowMaximized(): Promise<boolean> {
  return (await invokeWindowCommand("is_maximized")) === true;
}
