import type { AdvertisedEndpoint, ExternalTunnelEndpointSnapshot } from "@t3tools/contracts";

/**
 * The parts of `ConnectionsSettings` that are decisions rather than markup,
 * split out so they can be tested — the shape `KeybindingsSettings.logic.ts` and
 * `SettingsPanels.logic.ts` already use in this directory.
 */

/**
 * What to show under a remote environment's label, so two of them can be told
 * apart.
 *
 * **The label alone cannot do it.** `environment.label` is the machine's
 * hostname as the server reports it — `COMPUTERNAME`, then `HOSTNAME`, then
 * `/etc/hostname` — so two laplus data directories on one machine answer with
 * the same one by construction, and so do two cloud instances built from one
 * image. The saved list then shows two identical rows with a `Disconnect` and a
 * `Remove` button each, and pressing one of them is a guess.
 *
 * That was found by driving ticket 06 of the headless-Linux effort, which gave
 * every laplus a distinct environment id and left it invisible: the id is in the
 * client's registry and in the route, and nowhere a person looks. The ticket's
 * own reasoning — that environments sharing a label "are now told apart by the
 * id" — was true everywhere except the settings list that sentence was about.
 *
 * **Upstream shows nothing here either, and its gap is hidden.**
 * `pingdotgg/t3code`'s row builds the same metadata line from an SSH target and
 * a `relayManaged` flag, so its remotes are labelled `SSH user@host` or
 * `T3 Connect` and only a bare bearer remote is left blank. laplus removed the
 * relay surface, so the unlabelled shape is the *only* remote shape it has.
 *
 * The port is the point, which is why this is the host and not the hostname: two
 * servers on one machine differ by port and by nothing else. A default port is
 * dropped because `443` under a tunnel hostname is noise on the one shape whose
 * name is already unique.
 *
 * A value that will not parse is shown as it is rather than swallowed. It was
 * stored by some other build or written by hand, and rendering nothing would be
 * the bug this exists to fix; `null` is only for having nothing at all to say.
 */
export const formatRemoteBackendHost = (httpBaseUrl: string): string | null => {
  const trimmed = httpBaseUrl.trim();
  if (trimmed === "") {
    return null;
  }
  try {
    return new URL(trimmed).host;
  } catch {
    return trimmed;
  }
};

export const mergeVerifiedExternalEndpoint = (
  endpoints: ReadonlyArray<AdvertisedEndpoint>,
  snapshot: ExternalTunnelEndpointSnapshot | null,
): ReadonlyArray<AdvertisedEndpoint> => {
  const external = snapshot?.advertisedEndpoint;
  if (!external || endpoints.some((endpoint) => endpoint.id === external.id)) return endpoints;
  return [...endpoints, external];
};

export const visibleNetworkAdvertisedEndpoints = (
  endpoints: ReadonlyArray<AdvertisedEndpoint>,
  networkAccessible: boolean,
): ReadonlyArray<AdvertisedEndpoint> =>
  endpoints.filter(
    (endpoint) =>
      endpoint.provider.id !== "tailscale" &&
      (networkAccessible || endpoint.provider.id === "cloudflare"),
  );

export const registeredExternalTunnelHostname = (
  snapshot: ExternalTunnelEndpointSnapshot,
): string => snapshot.httpsOrigin ?? "";
