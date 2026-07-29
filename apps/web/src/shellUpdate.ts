/**
 * A newer laplus, from the window's point of view. Ticket 74.
 *
 * Upstream feeds its update pill from `window.desktopBridge` — Electron's
 * preload, which this application does not have and deliberately does not fake.
 * Several unrelated features branch on that object's presence
 * (`ConnectionsSettings`, `branding`, `browser`), so defining a partial one to
 * satisfy the pill would tell all of them a full Electron bridge is there. What
 * changes instead is the two readers of the update state, and this is what they
 * read.
 *
 * ## What is on the other end
 *
 * `tauri-plugin-updater`, registered in `laplus-shell`'s builder and granted to
 * this origin by `capabilities/updater.toml`. The commands are pressed directly
 * through `window.__TAURI_INTERNALS__`, exactly as `desktopShell.ts` presses the
 * window ones and for the same reason: `@tauri-apps/plugin-updater` would buy a
 * dependency, and the web bundle is 80% of the artifact (ticket 24). What the
 * package actually implements that is worth having is the `Channel`, which is
 * fifteen lines and is [`channel`] below.
 *
 * **Plugin commands rather than commands of our own.** A custom
 * `#[tauri::command]` is refused when it comes from a remote origin unless the
 * application ships an ACL manifest naming it (`tauri`'s `webview/mod.rs`, and
 * this page is a remote origin because the window is pointed at loopback —
 * ADR-0010). The plugin's commands arrive with their permissions already
 * defined, so this is both less machinery and the path the ACL was built for.
 *
 * ## What this deliberately does not do
 *
 * **It checks once per launch.** Ticket 66 asks that an unreachable network not
 * become a recurring visible error; a failed check leaves `status: "error"`,
 * which `shouldShowDesktopUpdateButton` does not render at all. That is the
 * intended behaviour rather than an oversight — a developer whose wifi is off
 * should see nothing, not a badge.
 *
 * **It reports one channel.** `DesktopUpdateState` carries `latest` and
 * `nightly` because upstream publishes both; this fork publishes one
 * (ADR-0020), so the answer is always `latest` and the release notes the
 * nightly tooltip would render stay empty.
 *
 * **It reports `x64` for both architectures.** This fork ships one Windows
 * artifact, so the Apple-Silicon-on-Intel warning the pill can draw never
 * applies, and claiming an arch this build does not have would be the only way
 * to make it fire.
 */

import type { DesktopUpdateActionResult, DesktopUpdateState } from "@t3tools/contracts";

/** What `plugin:updater|check` answers with when there is something to install. */
interface UpdateMetadata {
  readonly rid: number;
  readonly version: string;
  readonly currentVersion: string;
  readonly date?: string;
  readonly body?: string;
}

/** What the download channel carries, from the plugin's `DownloadEvent`. */
export type DownloadEvent =
  | { readonly event: "Started"; readonly data: { readonly contentLength?: number } }
  | { readonly event: "Progress"; readonly data: { readonly chunkLength: number } }
  | { readonly event: "Finished" };

/** The half of `DesktopBridge` the update pill and its atom actually use. */
export interface ShellUpdateBridge {
  getUpdateState(): Promise<DesktopUpdateState>;
  onUpdateState(listener: (state: DesktopUpdateState) => void): () => void;
  downloadUpdate(): Promise<DesktopUpdateActionResult>;
  installUpdate(): Promise<DesktopUpdateActionResult>;
}

export type Invoke = (command: string, payload?: Record<string, unknown>) => Promise<unknown>;

/**
 * A channel the plugin can send download progress down. `onEvent` is a required
 * argument of `plugin:updater|download`, so this is not optional machinery.
 */
export interface ChannelHandle {
  /** What goes in the payload; serialises to `__CHANNEL__:<id>`. */
  readonly payload: unknown;
  /** Release the callback slot. */
  dispose(): void;
}

export type MakeChannel = (onMessage: (event: DownloadEvent) => void) => ChannelHandle;

/**
 * The state a window starts in: enabled, having asked nothing yet.
 *
 * `currentVersion` is empty rather than guessed. The number this application
 * reports over RPC is the *UI bundle's* (ADR-0011), which is not the version an
 * installer or an update is about — and the plugin's own `check` answers with
 * the real one, so the honest thing is to have nothing to say until it does.
 */
export function initialUpdateState(): DesktopUpdateState {
  return {
    enabled: true,
    status: "idle",
    channel: "latest",
    currentVersion: "",
    hostArch: "x64",
    appArch: "x64",
    runningUnderArm64Translation: false,
    availableVersion: null,
    downloadedVersion: null,
    releaseNotes: [],
    downloadPercent: null,
    checkedAt: null,
    message: null,
    errorContext: null,
    canRetry: false,
  };
}

function failureMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/**
 * The bridge, over an `invoke` and a channel factory rather than over the
 * globals, so the whole of it can be driven by a test with no webview in sight.
 * [`shellUpdateBridge`] is the one wired to the real ones.
 */
export function createShellUpdateBridge(
  invoke: Invoke,
  makeChannel: MakeChannel,
): ShellUpdateBridge {
  let state = initialUpdateState();
  const listeners = new Set<(state: DesktopUpdateState) => void>();

  // The plugin hands out resource ids: one for the update it found, one for the
  // bytes once they are downloaded. `install` needs both, which is why the
  // download's answer is kept rather than only its effect on the state.
  let updateRid: number | null = null;
  let bytesRid: number | null = null;
  let checking: Promise<void> | null = null;

  function publish(next: Partial<DesktopUpdateState>): DesktopUpdateState {
    state = { ...state, ...next };
    for (const listener of listeners) {
      listener(state);
    }
    return state;
  }

  async function check(): Promise<void> {
    publish({ status: "checking", message: null, errorContext: null, canRetry: false });
    try {
      const found = (await invoke("plugin:updater|check", {})) as UpdateMetadata | null;
      const checkedAt = new Date().toISOString();
      if (!found) {
        publish({ status: "up-to-date", checkedAt, availableVersion: null });
        return;
      }
      updateRid = found.rid;
      publish({
        status: "available",
        checkedAt,
        availableVersion: found.version,
        currentVersion: found.currentVersion,
      });
    } catch (error) {
      // Not rendered by the pill, on purpose — see the note at the top of this
      // file. The console line is for the developer who wonders why nothing
      // appeared.
      console.error("laplus: could not check for an update.", error);
      publish({
        status: "error",
        errorContext: "check",
        message: failureMessage(error),
        canRetry: true,
      });
    }
  }

  return {
    async getUpdateState() {
      // Once per window, and shared: the atom reads this immediately and the
      // pill may read it again before the first answer lands.
      checking ??= check();
      return state;
    },

    onUpdateState(listener) {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },

    async downloadUpdate() {
      if (updateRid === null) {
        return { accepted: false, completed: false, state };
      }

      let contentLength: number | null = null;
      let received = 0;
      const channel = makeChannel((event) => {
        if (event.event === "Started") {
          contentLength = event.data.contentLength ?? null;
          received = 0;
          publish({ status: "downloading", downloadPercent: contentLength === null ? null : 0 });
          return;
        }
        if (event.event === "Progress") {
          received += event.data.chunkLength;
          // A percentage is only honest when the response said how long it was;
          // without that the pill draws "Downloading…" and is right to.
          publish({
            downloadPercent: contentLength === null ? null : (received / contentLength) * 100,
          });
          return;
        }
        publish({ downloadPercent: contentLength === null ? null : 100 });
      });

      try {
        bytesRid = (await invoke("plugin:updater|download", {
          rid: updateRid,
          onEvent: channel.payload,
        })) as number;
        return {
          accepted: true,
          completed: true,
          state: publish({ status: "downloaded", downloadedVersion: state.availableVersion }),
        };
      } catch (error) {
        return {
          accepted: true,
          completed: false,
          state: publish({
            status: "error",
            errorContext: "download",
            message: failureMessage(error),
            canRetry: true,
          }),
        };
      } finally {
        channel.dispose();
      }
    },

    async installUpdate() {
      if (updateRid === null || bytesRid === null) {
        return { accepted: false, completed: false, state };
      }

      try {
        // On Windows this does not return: the installer runs and the
        // application is exited, which Tauri documents as a limitation of
        // Windows installers rather than a choice. So the success path here is
        // mostly theoretical, and the failure path is the one that matters.
        //
        // **A retry after a failed install will fail too**, and visibly: the
        // plugin closes the downloaded-bytes resource inside this command
        // (`commands.rs`, `resources_table().close(bytes_rid)`), so the handle
        // this holds is spent either way. The pill goes on offering "Restart to
        // update" and each press produces the plugin's own error in a toast,
        // which is deliberate — the alternative shapes are a button that
        // silently does nothing, or one that hides itself and leaves a
        // downloaded update with no way to reach it. Relaunching laplus starts
        // the whole flow again.
        await invoke("plugin:updater|install", { updateRid, bytesRid });
        return { accepted: true, completed: true, state };
      } catch (error) {
        return {
          accepted: true,
          completed: false,
          state: publish({
            status: "error",
            errorContext: "install",
            message: failureMessage(error),
            canRetry: true,
          }),
        };
      }
    },
  };
}

interface TauriInternals {
  invoke(command: string, payload?: unknown): Promise<unknown>;
  transformCallback(callback: (response: unknown) => void, once?: boolean): number;
  unregisterCallback?(id: number): void;
}

interface TauriWindow {
  readonly isTauri?: boolean;
  readonly __TAURI_INTERNALS__?: TauriInternals;
}

function internals(): TauriInternals | null {
  if (typeof window === "undefined") {
    return null;
  }
  const found = (window as unknown as TauriWindow).__TAURI_INTERNALS__;
  return found &&
    typeof found.invoke === "function" &&
    typeof found.transformCallback === "function"
    ? found
    : null;
}

/**
 * Tauri's `Channel`, in the fifteen lines of it this needs.
 *
 * The wire format is fixed and is what `@tauri-apps/api` implements:
 * `transformCallback` registers a callback and returns its id, an argument
 * serialising to `__CHANNEL__:<id>` is recognised by the IPC as a channel, and
 * `unregisterCallback` releases the slot. The plugin's messages arrive wrapped
 * with an `index` for ordering, which is dropped here — the download events this
 * carries are a progress counter, and one arriving out of order would move a
 * percentage by a chunk.
 */
function channel(found: TauriInternals): MakeChannel {
  return (onMessage) => {
    const id = found.transformCallback((response) => {
      const message = (response as { message?: DownloadEvent }).message;
      onMessage((message ?? response) as DownloadEvent);
    });
    return {
      payload: `__CHANNEL__:${id}`,
      dispose() {
        found.unregisterCallback?.(id);
      },
    };
  };
}

let bridge: ShellUpdateBridge | null | undefined;

/**
 * The window's update bridge, or `undefined` where there is no window.
 *
 * A phone that paired over a tunnel gets `undefined`, which is the correct
 * answer twice over: its browser has no IPC, and replacing the application on
 * somebody's PC is not a thing a phone should offer to do.
 */
export function getShellUpdateBridge(): ShellUpdateBridge | undefined {
  if (bridge === undefined) {
    const found = internals();
    bridge = found ? createShellUpdateBridge((c, p) => found.invoke(c, p), channel(found)) : null;
  }
  return bridge ?? undefined;
}
