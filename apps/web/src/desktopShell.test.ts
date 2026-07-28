import { afterEach, describe, expect, it, vi } from "vite-plus/test";

/**
 * Ticket 27. The gate that decides whether this UI draws its own window
 * controls, tested for the two things that would break it silently.
 *
 * A wrong answer here has no symptom to notice. Too eager and a browser tab
 * grows three buttons that call an IPC that is not there; too shy and laplus's
 * own window has a bar with no way to close it, which is the failure the whole
 * ticket is about. Neither shows up in a screenshot of the other.
 *
 * Imported dynamically because the flag is read once at module load, exactly as
 * `isElectron` is — so the global has to be in place before the import, and
 * each case needs its own.
 */

async function loadWith(globals: Record<string, unknown>) {
  vi.resetModules();
  const stub = { ...globals, location: { origin: "http://127.0.0.1:4773" } };
  vi.stubGlobal("window", stub);
  return import("./desktopShell");
}

afterEach(() => {
  vi.unstubAllGlobals();
  vi.resetModules();
});

describe("isDesktopShell", () => {
  it("is true in a Tauri window, which is the global Tauri injects before any page script", async () => {
    const { isDesktopShell } = await loadWith({
      isTauri: true,
      __TAURI_INTERNALS__: { invoke: () => Promise.resolve() },
    });
    expect(isDesktopShell).toBe(true);
  });

  it("is false in a browser tab", async () => {
    const { isDesktopShell } = await loadWith({});
    expect(isDesktopShell).toBe(false);
  });

  /**
   * The two globals are asked one question between them, so they are not
   * allowed to disagree. A page that claims Tauri without reachable internals
   * would draw the three buttons and then swallow every click — the one
   * outcome this gate exists to prevent, and the one with nothing to see.
   *
   * `isTauri` is a boolean, not a predicate
   * (`Object.defineProperty(window, 'isTauri', { value: true })`), so a stray
   * truthy value does not stand in for it either.
   */
  it("is false when the page claims Tauri but its IPC is not reachable", async () => {
    for (const internals of [undefined, {}, { invoke: "not-callable" }]) {
      const { isDesktopShell } = await loadWith({
        isTauri: true,
        ...(internals === undefined ? {} : { __TAURI_INTERNALS__: internals }),
      });
      expect(isDesktopShell).toBe(false);
    }
  });

  /**
   * The distinction ticket 27 turns on. `isElectron` gates every call into a
   * preload bridge; this shell has no such bridge, so the two flags must never
   * be read off the same thing.
   */
  it("does not key on Electron's bridge, and Electron does not turn it on", async () => {
    const { isDesktopShell } = await loadWith({
      desktopBridge: {},
      nativeApi: {},
    });
    expect(isDesktopShell).toBe(false);
  });
});

describe("invokeWindowCommand", () => {
  it("reaches Tauri's IPC under the plugin:window prefix the capability grants", async () => {
    const invoke = vi.fn().mockResolvedValue(undefined);
    const { invokeWindowCommand } = await loadWith({
      isTauri: true,
      __TAURI_INTERNALS__: { invoke },
    });

    await invokeWindowCommand("minimize");
    expect(invoke).toHaveBeenCalledWith("plugin:window|minimize");
  });

  /**
   * A denied command is the way this breaks in practice — a capability that
   * names the wrong origin refuses every one of them. It must not take the
   * button's click handler down with it, and it must say so out loud, because
   * the alternative is a button that looks fine and does nothing.
   */
  it("reports a refusal rather than rejecting", async () => {
    const error = new Error("window.minimize not allowed");
    const invoke = vi.fn().mockRejectedValue(error);
    const reported = vi.spyOn(console, "error").mockImplementation(() => {});
    const { invokeWindowCommand } = await loadWith({
      isTauri: true,
      __TAURI_INTERNALS__: { invoke },
    });

    await expect(invokeWindowCommand("close")).resolves.toBeUndefined();
    expect(reported).toHaveBeenCalled();
    expect(String(reported.mock.calls[0]?.[0])).toContain("plugin:window|close");
    reported.mockRestore();
  });

  it("does nothing at all in a browser, where there is no window to command", async () => {
    const { invokeWindowCommand, isWindowMaximized } = await loadWith({});
    await expect(invokeWindowCommand("toggle_maximize")).resolves.toBeUndefined();
    await expect(isWindowMaximized()).resolves.toBe(false);
  });
});
