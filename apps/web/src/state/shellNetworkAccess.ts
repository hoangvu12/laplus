/**
 * Where this server listens, asked of the Tauri shell.
 *
 * Upstream asks `window.desktopBridge`. laplus has none and deliberately does
 * not fake one — `desktopShell.ts` carries that argument, and it is not a style
 * preference: `desktopBridge` is consulted in two dozen files, so a partial one
 * would answer "yes, you are on the desktop" to every other feature that asks,
 * including the boot-credential lookup that would then stop falling back to the
 * URL fragment and leave the window unable to open a socket at all.
 *
 * What is shared instead is the *seam*.
 * {@link createDesktopNetworkAccessStateAtom} takes whatever can answer two
 * questions, and this passes it an object backed by three Tauri commands rather
 * than by Electron's IPC. Same atom, same snapshot shape, same panel.
 */
import type { AdvertisedEndpoint, DesktopServerExposureState } from "@t3tools/contracts";

import { invokeShellCommand, isDesktopShell } from "../desktopShell";
import { appAtomRegistry } from "~/rpc/atomRegistry";
import { createDesktopNetworkAccessStateAtom } from "./desktopNetworkAccess";

/**
 * One command answers the whole panel, and the atom asks for it in two halves.
 *
 * `laplus-shell`'s `network_access_state` returns the exposure state and the
 * advertised endpoints together, because it reads them from one snapshot of one
 * file and splitting it would let the halves disagree.
 */
export interface ShellNetworkAccessState extends DesktopServerExposureState {
  readonly advertisedEndpoints: ReadonlyArray<AdvertisedEndpoint>;
}

function readShellNetworkAccessState(): Promise<ShellNetworkAccessState> {
  return invokeShellCommand<ShellNetworkAccessState>("network_access_state");
}

/**
 * The two questions {@link createDesktopNetworkAccessStateAtom} asks, answered
 * from one command each.
 *
 * Two round trips where one would do, and deliberately: the alternative is
 * caching the first answer for the second to reuse, which is a cache to
 * invalidate for a call that reads an in-memory struct over a local IPC. The
 * atom already runs them concurrently.
 */
function shellNetworkAccessBridge() {
  if (!isDesktopShell) {
    return undefined;
  }
  return {
    getServerExposureState: () => readShellNetworkAccessState(),
    getAdvertisedEndpoints: () =>
      readShellNetworkAccessState().then((state) => state.advertisedEndpoints),
  };
}

// Annotated rather than inferred: the atom's own type reaches a symbol in
// `effect/Inspectable` that this module cannot name, and declaration emit fails
// on it (TS4023). Naming it through the factory sidesteps that without
// widening anything — the sibling atom is inferred only because it is declared
// in the same file as the factory.
export const shellNetworkAccessStateAtom: ReturnType<typeof createDesktopNetworkAccessStateAtom> =
  createDesktopNetworkAccessStateAtom(shellNetworkAccessBridge);

export function refreshShellNetworkAccessState(): void {
  appAtomRegistry.refresh(shellNetworkAccessStateAtom);
}

/**
 * Turn the switch. **This restarts the application.**
 *
 * The listener was bound before the window opened and cannot be moved out from
 * under the sockets on it, so the shell writes the file and relaunches — which
 * is what upstream's switch does, and what the confirmation dialog in
 * `ConnectionsSettings` already tells the user will happen.
 *
 * So this does not usefully resolve when the mode actually changes: the process
 * is gone. It resolves only when the mode asked for is the one already in
 * force, which is why the caller must not wait on it to close its dialog.
 */
export function setShellNetworkExposure(
  mode: DesktopServerExposureState["mode"],
): Promise<ShellNetworkAccessState> {
  return invokeShellCommand<ShellNetworkAccessState>("set_network_exposure", { mode });
}
