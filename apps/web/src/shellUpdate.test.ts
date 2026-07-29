import { describe, expect, it } from "vite-plus/test";

import {
  resolveDesktopUpdateButtonAction,
  shouldShowDesktopUpdateButton,
} from "./components/desktopUpdate.logic";
import {
  createShellUpdateBridge,
  type DownloadEvent,
  getShellUpdateBridge,
  type Invoke,
  type MakeChannel,
} from "./shellUpdate";

/**
 * The plugin, faked at the one seam that matters: `invoke`. Everything the
 * bridge does is a command name and a payload, so a recorded list of those is
 * the whole of what it can get wrong — plus the channel, which the fake hands
 * back so a test can push download events down it the way the plugin would.
 */
function harness(
  answers: Partial<Record<string, (payload: Record<string, unknown>) => unknown>> = {},
) {
  const calls: Array<{ command: string; payload: Record<string, unknown> }> = [];
  let emit: ((event: DownloadEvent) => void) | null = null;
  let disposed = 0;

  const invoke: Invoke = async (command, payload = {}) => {
    calls.push({ command, payload });
    const answer = answers[command];
    if (!answer) {
      throw new Error(`no fake for ${command}`);
    }
    return answer(payload);
  };

  const makeChannel: MakeChannel = (onMessage) => {
    emit = onMessage;
    return {
      payload: "__CHANNEL__:7",
      dispose() {
        disposed += 1;
      },
    };
  };

  return {
    bridge: createShellUpdateBridge(invoke, makeChannel),
    calls,
    disposals: () => disposed,
    emit: (event: DownloadEvent) => emit?.(event),
  };
}

/** Wait for the check kicked off by the first `getUpdateState` to land. */
async function settled(bridge: { getUpdateState: () => Promise<unknown> }) {
  await bridge.getUpdateState();
  await Promise.resolve();
  await Promise.resolve();
}

const AVAILABLE = {
  rid: 3,
  version: "0.2.0",
  currentVersion: "0.1.0",
};

describe("shellUpdate", () => {
  it("has no bridge where there is no Tauri window", () => {
    // A paired phone runs this same bundle in an ordinary browser, where there
    // are no internals to invoke — and replacing the application on somebody's
    // PC is not a thing a phone should be offered. `shouldShowDesktopUpdateButton`
    // is given `null` in that case and draws nothing.
    expect(getShellUpdateBridge()).toBe(undefined);
  });

  it("reports up-to-date when the endpoint offers nothing, and shows no pill", async () => {
    const { bridge } = harness({ "plugin:updater|check": () => null });

    await settled(bridge);
    const state = await bridge.getUpdateState();

    expect(state.status).toBe("up-to-date");
    expect(state.availableVersion).toBe(null);
    expect(state.checkedAt).not.toBe(null);
    expect(shouldShowDesktopUpdateButton(state)).toBe(false);
  });

  it("takes the current version from the plugin rather than from the UI bundle", async () => {
    const { bridge } = harness({ "plugin:updater|check": () => AVAILABLE });

    await settled(bridge);
    const state = await bridge.getUpdateState();

    // ADR-0011 makes the number this app reports over RPC the UI bundle's,
    // which is not what an installer is about. The plugin knows the real one.
    expect(state.currentVersion).toBe("0.1.0");
    expect(state.availableVersion).toBe("0.2.0");
    expect(state.status).toBe("available");
    expect(shouldShowDesktopUpdateButton(state)).toBe(true);
    expect(resolveDesktopUpdateButtonAction(state)).toBe("download");
  });

  it("checks once per window however many readers there are", async () => {
    const { bridge, calls } = harness({ "plugin:updater|check": () => AVAILABLE });

    await Promise.all([bridge.getUpdateState(), bridge.getUpdateState()]);
    await settled(bridge);

    expect(calls.filter((call) => call.command === "plugin:updater|check")).toHaveLength(1);
  });

  it("keeps a failed check off the screen", async () => {
    const { bridge } = harness({
      "plugin:updater|check": () => {
        throw new Error("dns went away");
      },
    });

    await settled(bridge);
    const state = await bridge.getUpdateState();

    expect(state.status).toBe("error");
    expect(state.errorContext).toBe("check");
    expect(state.message).toBe("dns went away");
    // Ticket 66's standing requirement: an unreachable network is not a
    // recurring user-visible error.
    expect(shouldShowDesktopUpdateButton(state)).toBe(false);
  });

  it("reports progress against the content length and ends downloaded", async () => {
    const { bridge, emit, calls, disposals } = harness({
      "plugin:updater|check": () => AVAILABLE,
      "plugin:updater|download": () => {
        emit({ event: "Started", data: { contentLength: 200 } });
        emit({ event: "Progress", data: { chunkLength: 50 } });
        emit({ event: "Progress", data: { chunkLength: 50 } });
        emit({ event: "Finished" });
        return 9;
      },
    });

    await settled(bridge);
    const seen: number[] = [];
    bridge.onUpdateState((state) => {
      if (state.downloadPercent !== null) seen.push(state.downloadPercent);
    });

    const result = await bridge.downloadUpdate();

    // Four from the channel, then a fifth when the download resolves and the
    // status becomes `downloaded` carrying the same percentage. The repeat is
    // the state changing for another reason, not a duplicated event.
    expect(seen).toEqual([0, 25, 50, 100, 100]);
    expect(result.completed).toBe(true);
    expect(result.state.status).toBe("downloaded");
    expect(result.state.downloadedVersion).toBe("0.2.0");
    expect(resolveDesktopUpdateButtonAction(result.state)).toBe("install");
    // The download names the update it found, and the channel is released.
    expect(calls.at(-1)).toEqual({
      command: "plugin:updater|download",
      payload: { rid: 3, onEvent: "__CHANNEL__:7" },
    });
    expect(disposals()).toBe(1);
  });

  it("leaves the percentage unknown when the response did not say how long it is", async () => {
    const { bridge, emit } = harness({
      "plugin:updater|check": () => AVAILABLE,
      "plugin:updater|download": () => {
        emit({ event: "Started", data: {} });
        emit({ event: "Progress", data: { chunkLength: 50 } });
        return 9;
      },
    });

    await settled(bridge);
    const result = await bridge.downloadUpdate();

    expect(result.state.downloadPercent).toBe(null);
  });

  it("keeps the update offerable when the download fails", async () => {
    const { bridge, disposals } = harness({
      "plugin:updater|check": () => AVAILABLE,
      "plugin:updater|download": () => {
        throw new Error("the release asset is gone");
      },
    });

    await settled(bridge);
    const result = await bridge.downloadUpdate();

    expect(result.accepted).toBe(true);
    expect(result.completed).toBe(false);
    expect(result.state.errorContext).toBe("download");
    // `resolveDesktopUpdateButtonAction` offers the download again on this
    // shape, which is why `availableVersion` has to survive the failure.
    expect(result.state.availableVersion).toBe("0.2.0");
    expect(resolveDesktopUpdateButtonAction(result.state)).toBe("download");
    expect(disposals()).toBe(1);
  });

  it("installs with both resource ids", async () => {
    const { bridge, emit, calls } = harness({
      "plugin:updater|check": () => AVAILABLE,
      "plugin:updater|download": () => {
        emit({ event: "Finished" });
        return 9;
      },
      "plugin:updater|install": () => undefined,
    });

    await settled(bridge);
    await bridge.downloadUpdate();
    const result = await bridge.installUpdate();

    expect(result.accepted).toBe(true);
    expect(calls.at(-1)).toEqual({
      command: "plugin:updater|install",
      payload: { updateRid: 3, bytesRid: 9 },
    });
  });

  it("refuses to install what was never downloaded", async () => {
    const { bridge, calls } = harness({ "plugin:updater|check": () => AVAILABLE });

    await settled(bridge);
    const result = await bridge.installUpdate();

    expect(result.accepted).toBe(false);
    expect(calls.some((call) => call.command === "plugin:updater|install")).toBe(false);
  });

  it("refuses to download what was never found", async () => {
    const { bridge, calls } = harness({ "plugin:updater|check": () => null });

    await settled(bridge);
    const result = await bridge.downloadUpdate();

    expect(result.accepted).toBe(false);
    expect(calls.some((call) => call.command === "plugin:updater|download")).toBe(false);
  });

  it("stops telling a listener that has unsubscribed", async () => {
    const { bridge } = harness({ "plugin:updater|check": () => AVAILABLE });

    const seen: string[] = [];
    const unsubscribe = bridge.onUpdateState((state) => seen.push(state.status));
    await settled(bridge);
    const afterSubscribed = seen.length;
    unsubscribe();
    await bridge.downloadUpdate();

    expect(afterSubscribed).toBeGreaterThan(0);
    expect(seen).toHaveLength(afterSubscribed);
  });
});
