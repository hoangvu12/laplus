import {
  ChevronDownIcon,
  ChevronsLeftRightEllipsisIcon,
  PlusIcon,
  QrCodeIcon,
  RefreshCwIcon,
  TerminalIcon,
  TriangleAlertIcon,
} from "lucide-react";
import { type ReactNode, memo, useCallback, useEffect, useMemo, useState } from "react";
import {
  AuthAccessReadScope,
  AuthAccessWriteScope,
  AuthAdministrativeScopes,
  AuthOrchestrationOperateScope,
  AuthOrchestrationReadScope,
  AuthRelayReadScope,
  AuthRelayWriteScope,
  AuthReviewWriteScope,
  AuthStandardClientScopes,
  AuthTerminalOperateScope,
  type AuthClientSession,
  type AuthEnvironmentScope,
  type AuthPairingLink,
  type AdvertisedEndpoint,
  type DesktopDiscoveredSshHost,
  type DesktopSshEnvironmentTarget,
  type DesktopServerExposureState,
  type EnvironmentId,
  type ExternalTunnelEndpointSnapshot,
  type ManagedCloudflareConnectorSnapshot,
  type CloudflareAccountSnapshot,
  type CloudflareAccountTunnel,
  type CloudflaredExecutable,
  type CloudflaredInstallationSnapshot,
  type CloudflaredRelease,
} from "@t3tools/contracts";
import { connectionStatusText } from "@t3tools/client-runtime/connection";
import {
  isAtomCommandInterrupted,
  squashAtomCommandFailure,
} from "@t3tools/client-runtime/state/runtime";
import * as DateTime from "effect/DateTime";
import * as Option from "effect/Option";

import { useCopyToClipboard } from "../../hooks/useCopyToClipboard";
import { cn } from "../../lib/utils";
import { formatElapsedDurationLabel, formatExpiresInLabel } from "../../timestampFormat";
import {
  cloudflareFailureMessage,
  cloudflareOwnershipLabel,
  cloudflareRowSummary,
  cloudflareWizardState,
  formatRemoteBackendHost,
  mergeVerifiedExternalEndpoint,
  registeredExternalTunnelHostname,
  selectableCloudflaredExecutables,
  visibleNetworkAdvertisedEndpoints,
  type CloudflareWizardPath,
} from "./ConnectionsSettings.logic";
import { resolveDesktopPairingUrl } from "./pairingUrls";
import {
  SettingsPageContainer,
  SettingsRow,
  SettingsSection,
  useRelativeTimeTick,
} from "./settingsLayout";
import { Input } from "../ui/input";
import { Checkbox } from "../ui/checkbox";
import {
  Dialog,
  DialogClose,
  DialogFooter,
  DialogDescription,
  DialogHeader,
  DialogPanel,
  DialogPopup,
  DialogTitle,
  DialogTrigger,
} from "../ui/dialog";
import { ScrollArea } from "../ui/scroll-area";
import {
  AlertDialog,
  AlertDialogClose,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogPopup,
  AlertDialogTitle,
} from "../ui/alert-dialog";
import { Popover, PopoverPopup, PopoverTrigger } from "../ui/popover";
import { QRCodeSvg } from "../ui/qr-code";
import { Spinner } from "../ui/spinner";
import { Switch } from "../ui/switch";
import { stackedThreadToast, toastManager } from "../ui/toast";
import { Tooltip, TooltipPopup, TooltipTrigger } from "../ui/tooltip";
import { Button } from "../ui/button";
import { Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from "../ui/empty";
import { Group, GroupSeparator } from "../ui/group";
import { AnimatedHeight } from "../AnimatedHeight";
import {
  Menu,
  MenuGroup,
  MenuGroupLabel,
  MenuItem,
  MenuPopup,
  MenuSeparator,
  MenuTrigger,
} from "../ui/menu";
import { Textarea } from "../ui/textarea";
import { getPairingTokenFromUrl, setPairingTokenOnUrl } from "../../pairingUrl";
import {
  beginCloudflareLogin,
  cancelCloudflareLogin,
  consentToCloudflareCertificate,
  createServerPairingCredential,
  configureManagedCloudflareConnector,
  discoverCloudflaredExecutables,
  forgetExternalTunnelEndpoint,
  installCloudflaredRelease,
  listCloudflareTunnels,
  readCloudflareAccount,
  readCloudflaredInstallation,
  readExternalTunnelEndpoint,
  readManagedCloudflareConnector,
  registerExternalTunnelEndpoint,
  adoptCloudflareTunnel,
  selectCloudflareTunnel,
  revokeOtherServerClientSessions,
  revokeServerClientSession,
  revokeServerPairingLink,
  retryManagedCloudflareConnector,
  startManagedCloudflareConnector,
  stopManagedCloudflareConnector,
  testExternalTunnelEndpoint,
  isLoopbackHostname,
  usePrimarySessionState,
  type ServerClientSessionRecord,
  type ServerPairingLinkRecord,
} from "~/environments/primary";
import { useUiStateStore } from "~/uiStateStore";
import {
  resolveServerConfigVersionMismatch,
  resolveServerSelfUpdateCapability,
} from "~/versionSkew";
import { authEnvironment } from "~/state/auth";
import { environmentCatalog } from "~/connection/catalog";
import {
  connectPairing as connectPairingAtom,
  connectSshEnvironment as connectSshEnvironmentAtom,
} from "~/connection/onboarding";
import { useEnvironmentQuery } from "~/state/query";
import {
  desktopNetworkAccessStateAtom,
  refreshDesktopNetworkAccessState,
} from "~/state/desktopNetworkAccess";
import {
  refreshShellNetworkAccessState,
  setShellNetworkExposure,
  shellNetworkAccessStateAtom,
} from "~/state/shellNetworkAccess";
import { isDesktopShell } from "~/desktopShell";
import { desktopSshHostsStateAtom } from "~/state/desktopSshHosts";
import {
  type EnvironmentPresentation,
  useEnvironments,
  usePrimaryEnvironment,
} from "~/state/environments";
import { useAtomCommand } from "../../state/use-atom-command";
import { ConnectionStatusDot } from "../ConnectionStatusDot";
import { ServerUpdateAction } from "../ServerUpdateAction";
import { ITEM_ROW_CLASSNAME, ITEM_ROW_INNER_CLASSNAME } from "./itemRows";

const DEFAULT_TAILSCALE_SERVE_PORT = 443;
const EMPTY_ADVERTISED_ENDPOINTS: ReadonlyArray<AdvertisedEndpoint> = [];
const EMPTY_DISCOVERED_SSH_HOSTS: ReadonlyArray<DesktopDiscoveredSshHost> = [];

/**
 * Cloudflare Tunnel: a compact Connections row in front of a modal wizard.
 *
 * **The wizard has one source of truth for how far setup got, and it is the
 * server.** `cloudflare_account.rs` computes the step from what is durably true
 * — a certificate on disk, a recorded consent, a recorded selection — and the
 * connector and endpoint snapshots say the rest. `cloudflareWizardState` in
 * `ConnectionsSettings.logic.ts` turns those three into the screen to show, so
 * a reopened dialog, a reloaded page and a restarted server cannot disagree
 * about progress. The only client-held piece is which path the developer just
 * picked, which is a choice not yet committed to anything rather than progress.
 *
 * The three paths are three ownerships, and keeping them apart is the point:
 * laplus may supervise a connector it was given a token for, it may be handed
 * an inactive tunnel to dedicate, and it may verify a hostname somebody else
 * runs — but never two of those for one hostname. ADR-0045.
 */
export function CloudflareTunnelSettingsRow({
  canWrite,
  onSnapshot,
}: {
  readonly canWrite: boolean;
  readonly onSnapshot?: (snapshot: ExternalTunnelEndpointSnapshot) => void;
}) {
  const [snapshot, setSnapshot] = useState<ExternalTunnelEndpointSnapshot | null>(null);
  const [managed, setManaged] = useState<ManagedCloudflareConnectorSnapshot | null>(null);
  const [account, setAccount] = useState<CloudflareAccountSnapshot | null>(null);
  const [installation, setInstallation] = useState<CloudflaredInstallationSnapshot | null>(null);
  const [cloudflaredExecutables, setCloudflaredExecutables] = useState<
    ReadonlyArray<CloudflaredExecutable>
  >([]);

  const [open, setOpen] = useState(false);
  const [chosenPath, setChosenPath] = useState<CloudflareWizardPath | null>(null);
  const [revisitingPathChoice, setRevisitingPathChoice] = useState(false);
  // The way out of an activation race: the server truthfully reports the
  // hostname as somebody else's, and every step derived from that selection
  // then leads back to the same screen. See `cloudflareWizardState`.
  const [revisitingTunnelChoice, setRevisitingTunnelChoice] = useState(false);
  // **Two hostnames, deliberately.** A laplus-managed connector's hostname and
  // an externally managed one are different claims about who owns a lifecycle,
  // and sharing one field made "Register" hand the managed hostname to the
  // external path — the ownership conflation ADR-0045 forbids.
  const [managedHostname, setManagedHostname] = useState("");
  const [externalHostname, setExternalHostname] = useState("");
  const [tunnelHostname, setTunnelHostname] = useState("");
  const [selectedTunnelId, setSelectedTunnelId] = useState("");
  const [executablePath, setExecutablePath] = useState("");
  const [connectorToken, setConnectorToken] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [pairingUrl, setPairingUrl] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const next = await readExternalTunnelEndpoint();
      setSnapshot(next);
      setExternalHostname(registeredExternalTunnelHostname(next));
      onSnapshot?.(next);
    } catch (cause) {
      setError(cloudflareFailureMessage(cause, "Could not load Cloudflare Tunnel state."));
    }
  }, [onSnapshot]);
  useEffect(() => {
    void refresh();
  }, [refresh]);

  const refreshInstallation = useCallback(async () => {
    try {
      const discovery = await discoverCloudflaredExecutables();
      setCloudflaredExecutables(discovery.executables);
      setInstallation(
        offersCloudflaredInstallation(discovery.executables)
          ? await readCloudflaredInstallation()
          : null,
      );
    } catch {
      // Discovery and the release preview are both advisory: the wizard's other
      // paths — an executable typed in by hand, an externally managed hostname —
      // stay usable without them.
    }
  }, []);

  const refreshAccount = useCallback(async () => {
    try {
      setAccount(await readCloudflareAccount());
    } catch {
      // Swallowed on purpose, and for two different callers: a session without
      // `access:read` is refused here by design (ADR-0047), and the poll that
      // watches a browser sign-in runs once a second. Neither has anything to
      // say that is not already on screen, and the wizard's external path needs
      // no Cloudflare account at all — so a refusal or a dropped request leaves
      // the dialog usable rather than blanking it. Anything the developer
      // *asked* for goes through `mutateAccount`, which does show its failure.
    }
  }, []);

  useEffect(() => {
    if (!open) return;
    void refreshInstallation();
    void refreshAccount();
  }, [open, refreshAccount, refreshInstallation]);

  const install = useCallback(
    async (release: CloudflaredRelease) => {
      setBusy(true);
      setError(null);
      try {
        const next = await installCloudflaredRelease({
          version: release.version,
          checksum: release.checksum,
        });
        setInstallation(next);
        setCloudflaredExecutables((await discoverCloudflaredExecutables()).executables);
        if (next.installedPath) setExecutablePath(next.installedPath);
      } catch (cause) {
        setError(cloudflareFailureMessage(cause, "The installation failed."));
        await refreshInstallation();
      } finally {
        setBusy(false);
      }
    },
    [refreshInstallation],
  );

  useEffect(() => {
    void Promise.all([readManagedCloudflareConnector(), discoverCloudflaredExecutables()])
      .then(([next, discovery]) => {
        const candidates = discovery.executables;
        setManaged(next);
        setCloudflaredExecutables(candidates);
        setExecutablePath(
          next.executablePath ??
            candidates.find((item) => item.compatibility === "compatible")?.path ??
            candidates[0]?.path ??
            "",
        );
        if (next.httpsOrigin) setManagedHostname(new URL(next.httpsOrigin).hostname);
      })
      .catch(() => undefined);
  }, []);

  useEffect(() => {
    if (!open || !managed?.configured) return;
    const refreshManaged = () => {
      void readManagedCloudflareConnector()
        .then(setManaged)
        .catch(() => undefined);
    };
    refreshManaged();
    const interval = window.setInterval(refreshManaged, 1_000);
    return () => window.clearInterval(interval);
  }, [managed?.configured, open]);

  // A browser sign-in finishes somewhere laplus cannot see, so the only way to
  // learn that it did is to keep asking. Stops the moment it is no longer
  // running, which is what makes cancellation and timeout visible too.
  useEffect(() => {
    if (!open || account?.loginState !== "awaiting-browser") return;
    const interval = window.setInterval(() => void refreshAccount(), 1_000);
    return () => window.clearInterval(interval);
  }, [account?.loginState, open, refreshAccount]);

  const mutateManaged = useCallback(
    async (operation: "configure" | "start" | "stop" | "retry") => {
      setBusy(true);
      setError(null);
      try {
        const next =
          operation === "configure"
            ? await configureManagedCloudflareConnector({
                hostname: managedHostname,
                executablePath,
                connectorToken,
              })
            : operation === "start"
              ? await startManagedCloudflareConnector()
              : operation === "stop"
                ? await stopManagedCloudflareConnector()
                : await retryManagedCloudflareConnector();
        setManaged(next);
        if (operation === "configure") setConnectorToken("");
      } catch (cause) {
        setError(cloudflareFailureMessage(cause, "The connector request failed."));
      } finally {
        setBusy(false);
      }
    },
    [connectorToken, executablePath, managedHostname],
  );

  const mutate = useCallback(
    async (operation: "register" | "test" | "forget") => {
      setBusy(true);
      setError(null);
      try {
        const next =
          operation === "register"
            ? await registerExternalTunnelEndpoint(externalHostname)
            : operation === "test"
              ? await testExternalTunnelEndpoint()
              : await forgetExternalTunnelEndpoint();
        setSnapshot(next);
        onSnapshot?.(next);
        if (next.httpsOrigin) setExternalHostname(next.httpsOrigin);
      } catch (cause) {
        setError(cloudflareFailureMessage(cause, "The request failed."));
      } finally {
        setBusy(false);
      }
    },
    [externalHostname, onSnapshot],
  );

  /** Every Cloudflare account action answers with the whole snapshot. */
  const mutateAccount = useCallback(async (run: () => Promise<CloudflareAccountSnapshot>) => {
    setBusy(true);
    setError(null);
    try {
      setAccount(await run());
    } catch (cause) {
      setError(cloudflareFailureMessage(cause, "The Cloudflare request failed."));
    } finally {
      setBusy(false);
    }
  }, []);

  const chooseTunnel = useCallback(async () => {
    setRevisitingTunnelChoice(false);
    await mutateAccount(() =>
      selectCloudflareTunnel({ tunnelId: selectedTunnelId, hostname: tunnelHostname }),
    );
    // Selecting an active tunnel registers it as an external endpoint
    // server-side, so the endpoint snapshot this row advertises has moved.
    await refresh();
  }, [mutateAccount, refresh, selectedTunnelId, tunnelHostname]);

  /**
   * Confirm dedication, then re-read everything the answer could have moved.
   *
   * **A refusal moves the server too, which is why this refreshes either way.**
   * An activation race reclassifies the selection as external and registers the
   * hostname as somebody else's — so a client that kept the snapshot it already
   * had would go on offering to dedicate a tunnel the server has just disowned,
   * with the refusal's sentence above a button that could only be refused
   * again. Driving the wizard headlessly is how that was found.
   *
   * Three reads because the answer arrives in three snapshots: the account says
   * which step the wizard is on, the connector says what laplus is now
   * supervising, and the endpoint says who owns the tunnel behind it.
   */
  const dedicateTunnel = useCallback(async () => {
    await mutateAccount(() => adoptCloudflareTunnel(executablePath));
    await Promise.all([
      refreshAccount(),
      readManagedCloudflareConnector()
        .then(setManaged)
        .catch(() => undefined),
      refresh(),
    ]);
  }, [executablePath, mutateAccount, refresh, refreshAccount]);

  const createPairing = useCallback(async () => {
    const origin = managed?.configured ? managed.httpsOrigin : snapshot?.httpsOrigin;
    if (!origin) return;
    setBusy(true);
    setError(null);
    try {
      const result = await createServerPairingCredential({ label: "Cloudflare Tunnel" });
      setPairingUrl(resolveDesktopPairingUrl(origin, result.credential));
    } catch (cause) {
      setError(cloudflareFailureMessage(cause, "Could not create a pairing link."));
    } finally {
      setBusy(false);
    }
  }, [managed?.configured, managed?.httpsOrigin, snapshot?.httpsOrigin]);

  const wizard = cloudflareWizardState({
    account,
    managed,
    external: snapshot,
    chosenPath,
    revisitingPathChoice,
    revisitingTunnelChoice,
  });
  const verified =
    managed?.configured === true
      ? managed.verificationState === "verified"
      : snapshot?.verificationState === "verified";
  // cloudflared is only run by the paths that run it. The external path never
  // touches an executable, so it is never asked to choose one — and the
  // connector panel carries its own picker, so the two never render together.
  const runsCloudflared =
    wizard.step === "sign-in" ||
    wizard.step === "choose-tunnel" ||
    wizard.step === "confirm-adoption" ||
    wizard.step === "adopting" ||
    wizard.step === "connector-token" ||
    wizard.step === "managed-connector";
  // Dedication runs cloudflared twice — once to re-read the tunnel's activity
  // and once to retrieve its credential — so the screen that asks for it is
  // also a screen that has to say which executable will do it.
  const picksExecutable =
    wizard.step === "sign-in" ||
    wizard.step === "choose-tunnel" ||
    wizard.step === "confirm-adoption";

  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <SettingsRow
        title="Cloudflare Tunnel"
        description={cloudflareRowSummary({
          state: wizard,
          managed,
          external: snapshot,
          managedStateLabel: managedCloudflareCompactState,
        })}
        status={
          managed?.failureMessage ? (
            <span className="text-destructive">{managed.failureMessage}</span>
          ) : account?.failureMessage ? (
            <span className="text-destructive">{account.failureMessage}</span>
          ) : snapshot?.verificationState === "failed" ? (
            <span className="text-destructive">{snapshot.failureMessage}</span>
          ) : undefined
        }
        control={
          <DialogTrigger render={<Button size="xs" variant="outline" />}>
            {snapshot?.configured || managed?.configured ? "Manage" : "Set up"}
          </DialogTrigger>
        }
      />
      <DialogPopup className="max-w-lg">
        <DialogHeader>
          <DialogTitle>Cloudflare Tunnel</DialogTitle>
          <DialogDescription>
            {wizard.position
              ? `Step ${wizard.position.index} of ${wizard.position.total} · ${wizard.label}`
              : wizard.label}
          </DialogDescription>
        </DialogHeader>
        <DialogPanel className="space-y-4">
          {runsCloudflared && offersCloudflaredInstallation(cloudflaredExecutables) ? (
            <CloudflaredInstallationPanel
              snapshot={installation}
              canWrite={canWrite}
              busy={busy}
              onInstall={(release) => void install(release)}
            />
          ) : null}
          {picksExecutable ? (
            <CloudflaredExecutablePicker
              executables={cloudflaredExecutables}
              executablePath={executablePath}
              canWrite={canWrite}
              busy={busy}
              onExecutablePathChange={setExecutablePath}
            />
          ) : null}

          {wizard.step === "choose-path" ? (
            <CloudflareSetupPathChoice
              canWrite={canWrite}
              onChoose={(path) => {
                setChosenPath(path);
                setRevisitingPathChoice(false);
              }}
            />
          ) : null}

          {wizard.step === "sign-in" ? (
            <CloudflareSignInStep
              account={account}
              canWrite={canWrite}
              busy={busy}
              executablePath={executablePath}
              onBegin={() => void mutateAccount(() => beginCloudflareLogin(executablePath))}
              onCancel={() => void mutateAccount(cancelCloudflareLogin)}
            />
          ) : null}

          {wizard.step === "consent" && account ? (
            <CloudflareCertificateConsentStep
              account={account}
              canWrite={canWrite}
              busy={busy}
              onConsent={(consented) =>
                void mutateAccount(() => consentToCloudflareCertificate(consented))
              }
            />
          ) : null}

          {wizard.step === "choose-tunnel" && account ? (
            <CloudflareTunnelChoiceStep
              account={account}
              canWrite={canWrite}
              busy={busy}
              selectedTunnelId={selectedTunnelId}
              hostname={tunnelHostname}
              onSelectTunnel={setSelectedTunnelId}
              onHostnameChange={setTunnelHostname}
              onRefresh={() => void mutateAccount(() => listCloudflareTunnels(executablePath))}
              onChoose={() => void chooseTunnel()}
            />
          ) : null}

          {wizard.step === "confirm-adoption" && account?.selection ? (
            <CloudflareAdoptionOffer
              selection={account.selection}
              loopbackOrigin={managed?.loopbackOrigin ?? null}
              canWrite={canWrite}
              busy={busy}
              onConfirm={() => void dedicateTunnel()}
            />
          ) : null}

          {wizard.step === "adopting" && managed?.configured ? (
            <CloudflareDedicatedConnectorPanel
              snapshot={managed}
              canWrite={canWrite}
              busy={busy}
              onStart={() => void mutateManaged("start")}
              onStop={() => void mutateManaged("stop")}
              onRetry={() => void mutateManaged("retry")}
            />
          ) : null}

          {wizard.step === "verify-hostname" && account?.selection ? (
            <section className="space-y-2" aria-label="Verify the tunnel hostname">
              <p className="text-sm font-medium">
                {account.selection.name} is already serving connections
              </p>
              <p className="text-xs text-muted-foreground">
                An active tunnel is operated outside laplus, so laplus records it as an external
                tunnel endpoint: it verifies and advertises{" "}
                <span className="font-medium">{account.selection.httpsOrigin}</span> and will never
                start, stop, reconfigure, or delete its connector.
              </p>
              {snapshot?.configured ? <CloudflareLayeredHealth snapshot={snapshot} /> : null}
              {canWrite ? (
                <Button
                  size="sm"
                  variant="outline"
                  disabled={busy}
                  onClick={() => setRevisitingTunnelChoice(true)}
                >
                  Choose a different tunnel
                </Button>
              ) : null}
            </section>
          ) : null}

          {wizard.step === "external-endpoint" ? (
            <section className="space-y-2" aria-label="Externally managed hostname">
              <p className="text-sm font-medium">Externally managed hostname</p>
              <div className="rounded-md border border-border/70 bg-muted/20 p-3 text-xs text-muted-foreground">
                laplus will verify and advertise this endpoint, but will never start, stop,
                reconfigure, or delete its connector. The hostname is public unless you
                independently protect it; laplus authentication remains required. Cloudflare Access
                may intercept pairing or WebSocket traffic.
              </div>
              <label className="block">
                <span className="text-sm font-medium">HTTPS hostname</span>
                <Input
                  className="mt-2"
                  aria-label="External HTTPS hostname"
                  placeholder="laplus.example.com"
                  value={externalHostname}
                  disabled={busy || !canWrite}
                  onChange={(event) => setExternalHostname(event.target.value)}
                />
              </label>
              {snapshot?.lastVerifiedAt ? (
                <p className="text-xs text-muted-foreground">
                  Last verified {formatAccessTimestamp(snapshot.lastVerifiedAt)}
                  {snapshot.verificationState !== "verified" ? " · stale" : ""}
                </p>
              ) : null}
              {snapshot?.configured ? <CloudflareLayeredHealth snapshot={snapshot} /> : null}
            </section>
          ) : null}

          {wizard.step === "connector-token" || wizard.step === "managed-connector" ? (
            <ManagedCloudflareConnectorPanel
              snapshot={managed}
              executables={cloudflaredExecutables}
              canWrite={canWrite}
              busy={busy}
              hostname={managedHostname}
              executablePath={executablePath}
              connectorToken={connectorToken}
              onHostnameChange={setManagedHostname}
              onExecutablePathChange={setExecutablePath}
              onConnectorTokenChange={setConnectorToken}
              onConfigure={() => void mutateManaged("configure")}
              onStart={() => void mutateManaged("start")}
              onStop={() => void mutateManaged("stop")}
              onRetry={() => void mutateManaged("retry")}
            />
          ) : null}

          {error ? <p className="text-xs text-destructive">{error}</p> : null}
          {/* Outside every step: a pairing link belongs to whichever endpoint
              was verified, and nesting it under one branch is how it became
              unreachable for a laplus-managed connector. */}
          {pairingUrl ? (
            <div className="flex flex-col items-center gap-3 rounded-md border p-3">
              <QRCodeSvg value={pairingUrl} className="size-40" />
              <Textarea readOnly value={pairingUrl} aria-label="Cloudflare pairing URL" />
            </div>
          ) : null}
        </DialogPanel>
        <DialogFooter>
          {wizard.canChangePath && canWrite ? (
            <Button
              variant="ghost"
              disabled={busy}
              onClick={() => {
                setChosenPath(null);
                setRevisitingPathChoice(true);
                setPairingUrl(null);
              }}
            >
              Change setup path
            </Button>
          ) : null}
          {snapshot?.configured && canWrite && !wizard.ownsConnector ? (
            <Button variant="destructive" disabled={busy} onClick={() => void mutate("forget")}>
              Forget
            </Button>
          ) : null}
          {snapshot?.configured && canWrite ? (
            <Button variant="outline" disabled={busy} onClick={() => void mutate("test")}>
              Test now
            </Button>
          ) : null}
          {verified && canWrite ? (
            <Button variant="outline" disabled={busy} onClick={() => void createPairing()}>
              Pair device
            </Button>
          ) : null}
          {wizard.offersExternalRegistration && canWrite ? (
            <Button
              disabled={busy || externalHostname.trim() === ""}
              onClick={() => void mutate("register")}
            >
              {busy ? "Working…" : snapshot?.configured ? "Update" : "Register"}
            </Button>
          ) : null}
        </DialogFooter>
      </DialogPopup>
    </Dialog>
  );
}

/**
 * The wizard's first screen: three ownerships, stated as three choices.
 *
 * Ordered least-privilege first. A connector token runs one tunnel and manages
 * no Cloudflare account; signing in hands this computer authority over every
 * tunnel in the account, which is why it is not the default.
 */
export function CloudflareSetupPathChoice({
  canWrite,
  onChoose,
}: {
  readonly canWrite: boolean;
  readonly onChoose: (path: CloudflareWizardPath) => void;
}) {
  const choices: ReadonlyArray<{
    readonly path: CloudflareWizardPath;
    readonly title: string;
    readonly description: string;
  }> = [
    {
      path: "connector-token",
      title: "Use a tunnel connector token",
      description:
        "Create the tunnel in Cloudflare and paste its connector token. laplus runs and supervises the connector and never gains account-wide authority.",
    },
    {
      path: "external",
      title: "Register a hostname someone else runs",
      description:
        "A tunnel already routes this server. laplus verifies and advertises the hostname and takes no lifecycle action on its connector.",
    },
    {
      path: "account",
      title: "Sign in to Cloudflare",
      description:
        "Discover the tunnels this account already has. Signing in leaves an account certificate on this computer with authority over every tunnel in the account.",
    },
  ];
  return (
    <section className="space-y-2" aria-label="Choose how to connect">
      {choices.map((choice) => (
        <button
          key={choice.path}
          type="button"
          disabled={!canWrite}
          onClick={() => onChoose(choice.path)}
          className="block w-full rounded-md border border-border/70 p-3 text-left hover:bg-muted/40 disabled:opacity-60"
        >
          <span className="text-sm font-medium">{choice.title}</span>
          <span className="mt-1 block text-xs text-muted-foreground">{choice.description}</span>
        </button>
      ))}
    </section>
  );
}

/**
 * Cloudflare's browser authorization, tracked from here rather than a terminal.
 *
 * The authorization URL is whatever cloudflared printed, shown rather than
 * opened for the developer: this dialog can be on a phone looking at a server
 * somewhere else, where "we opened your browser" would be a lie.
 */
export function CloudflareSignInStep({
  account,
  canWrite,
  busy,
  executablePath,
  onBegin,
  onCancel,
}: {
  readonly account: CloudflareAccountSnapshot | null;
  readonly canWrite: boolean;
  readonly busy: boolean;
  readonly executablePath: string;
  readonly onBegin: () => void;
  readonly onCancel: () => void;
}) {
  const awaiting = account?.loginState === "awaiting-browser";
  return (
    <section className="space-y-2" aria-label="Sign in to Cloudflare">
      <p className="text-sm font-medium">Authorize laplus with Cloudflare</p>
      <p className="text-xs text-muted-foreground">
        cloudflared opens Cloudflare&rsquo;s authorization page and writes an account certificate
        when you approve it. No terminal is involved, and you can stop at any point.
      </p>
      {awaiting && account?.authorizationUrl ? (
        <div className="space-y-2 rounded-md border border-border/70 p-3">
          <p className="text-xs font-medium">Waiting for the browser</p>
          <Textarea
            readOnly
            value={account.authorizationUrl}
            aria-label="Cloudflare authorization URL"
          />
        </div>
      ) : null}
      {account?.failureMessage ? (
        <p className="text-xs text-destructive">{account.failureMessage}</p>
      ) : null}
      {canWrite ? (
        <div className="flex flex-wrap gap-2">
          {awaiting ? (
            <Button size="sm" variant="outline" disabled={busy} onClick={onCancel}>
              Cancel sign-in
            </Button>
          ) : (
            <Button size="sm" disabled={busy || executablePath.trim() === ""} onClick={onBegin}>
              {account?.loginState === "not-started" ? "Sign in to Cloudflare" : "Try again"}
            </Button>
          )}
        </div>
      ) : null}
    </section>
  );
}

/**
 * The consent ADR-0045 requires before a certificate laplus did not create is
 * used.
 *
 * It names the file. A warning about "the account certificate" with no path is
 * consent to an abstraction, and a developer with two Cloudflare accounts on
 * one machine cannot otherwise tell which one they are handing over.
 */
export function CloudflareCertificateConsentStep({
  account,
  canWrite,
  busy,
  onConsent,
}: {
  readonly account: CloudflareAccountSnapshot;
  readonly canWrite: boolean;
  readonly busy: boolean;
  readonly onConsent: (consented: boolean) => void;
}) {
  return (
    <section className="space-y-2" aria-label="Confirm certificate use">
      <p className="text-sm font-medium">A Cloudflare account certificate is already here</p>
      <div className="rounded-md border border-border/70 bg-muted/20 p-3 text-xs text-muted-foreground">
        {account.certificateWarning}
      </div>
      <p className="break-all text-xs text-muted-foreground">{account.certificatePath}</p>
      {canWrite ? (
        <div className="flex flex-wrap gap-2">
          <Button size="sm" disabled={busy} onClick={() => onConsent(true)}>
            Use this certificate
          </Button>
          <Button size="sm" variant="outline" disabled={busy} onClick={() => onConsent(false)}>
            Don&rsquo;t use it
          </Button>
        </div>
      ) : null}
    </section>
  );
}

/**
 * The account's tunnels, and the one question the listing cannot answer.
 *
 * `tunnel list --output json` carries ids, names, timestamps and connections —
 * and no hostname and no management mode. So the hostname is asked for, and
 * activity is the only thing branched on.
 */
export function CloudflareTunnelChoiceStep({
  account,
  canWrite,
  busy,
  selectedTunnelId,
  hostname,
  onSelectTunnel,
  onHostnameChange,
  onRefresh,
  onChoose,
}: {
  readonly account: CloudflareAccountSnapshot;
  readonly canWrite: boolean;
  readonly busy: boolean;
  readonly selectedTunnelId: string;
  readonly hostname: string;
  readonly onSelectTunnel: (tunnelId: string) => void;
  readonly onHostnameChange: (hostname: string) => void;
  readonly onRefresh: () => void;
  readonly onChoose: () => void;
}) {
  const chosen = account.tunnels.find((tunnel) => tunnel.id === selectedTunnelId);
  return (
    <section className="space-y-3" aria-label="Choose a tunnel">
      <div>
        <p className="text-sm font-medium">Tunnels in this Cloudflare account</p>
        <p className="text-xs text-muted-foreground">
          {account.listedAt
            ? `Listed ${formatAccessTimestamp(account.listedAt)}`
            : "Not listed yet."}{" "}
          Listing reads Cloudflare and changes nothing, so it is always safe to run again.
        </p>
      </div>
      {account.tunnels.length === 0 ? (
        <p className="text-xs text-muted-foreground">
          This account has no tunnels laplus can offer.
        </p>
      ) : (
        <ul className="space-y-2">
          {account.tunnels.map((tunnel) => (
            <li key={tunnel.id}>
              <label className="flex cursor-pointer gap-2 rounded-md border border-border/70 p-3">
                <input
                  type="radio"
                  name="cloudflare-tunnel"
                  className="mt-1"
                  value={tunnel.id}
                  checked={tunnel.id === selectedTunnelId}
                  disabled={busy || !canWrite}
                  onChange={() => onSelectTunnel(tunnel.id)}
                />
                <span className="min-w-0">
                  <span className="block text-sm font-medium">{tunnel.name}</span>
                  <span className="block break-all text-xs text-muted-foreground">{tunnel.id}</span>
                  <span className="block text-xs text-muted-foreground">
                    {cloudflareTunnelActivityLabel(tunnel)}
                    {tunnel.createdAt
                      ? ` · created ${formatAccessTimestamp(tunnel.createdAt)}`
                      : ""}
                  </span>
                </span>
              </label>
            </li>
          ))}
        </ul>
      )}
      <label className="block">
        <span className="text-sm font-medium">HTTPS hostname</span>
        <Input
          className="mt-1"
          aria-label="Tunnel HTTPS hostname"
          placeholder="laplus.example.com"
          value={hostname}
          disabled={busy || !canWrite}
          onChange={(event) => onHostnameChange(event.target.value)}
        />
        <span className="mt-1 block text-xs text-muted-foreground">
          Cloudflare&rsquo;s tunnel list carries no hostname, so laplus asks for it and then
          verifies it rather than guessing one from the tunnel&rsquo;s name.
        </span>
      </label>
      {canWrite ? (
        <div className="flex flex-wrap gap-2">
          <Button size="sm" variant="outline" disabled={busy} onClick={onRefresh}>
            Refresh tunnel list
          </Button>
          <Button
            size="sm"
            disabled={busy || chosen === undefined || hostname.trim() === ""}
            onClick={onChoose}
          >
            Use this tunnel
          </Button>
        </div>
      ) : null}
    </section>
  );
}

export function cloudflareTunnelActivityLabel(tunnel: CloudflareAccountTunnel): string {
  return tunnel.activity === "active"
    ? `Active · ${tunnel.connectionCount} connection${tunnel.connectionCount === 1 ? "" : "s"} · externally managed`
    : "Inactive · can be dedicated to laplus";
}

/**
 * An inactive tunnel offered for dedication, and the ownership that would and
 * would not transfer.
 *
 * **Everything the confirmation is a confirmation of is on this screen.** The
 * tunnel it names, the hostname the developer supplied, where that hostname
 * would be routed to, what was observed about the tunnel, and which half of the
 * ownership moves — a consent that omits the loopback target is consent to an
 * abstraction. ADR-0045 makes this a separate, explicit step precisely because
 * the mutation behind it retrieves a run credential and writes a connector
 * configuration.
 *
 * The observed inactivity is what the *listing* proved, and the server re-reads
 * it immediately before mutating: a connector that starts in between makes the
 * tunnel externally managed, and the confirmation is refused rather than obeyed.
 */
export function CloudflareAdoptionOffer({
  selection,
  loopbackOrigin,
  canWrite,
  busy,
  onConfirm,
}: {
  readonly selection: NonNullable<CloudflareAccountSnapshot["selection"]>;
  readonly loopbackOrigin: string | null;
  readonly canWrite: boolean;
  readonly busy: boolean;
  readonly onConfirm: () => void;
}) {
  return (
    <section className="space-y-2" aria-label="Dedicate the tunnel">
      <p className="text-sm font-medium">Dedicate {selection.name} to this laplus environment</p>
      <dl className="grid grid-cols-[auto_1fr] gap-x-3 text-xs text-muted-foreground">
        <dt>Tunnel</dt>
        <dd className="break-all">{selection.tunnelId}</dd>
        <dt>Hostname</dt>
        <dd className="break-all">{selection.httpsOrigin}</dd>
        <dt>Routes to</dt>
        <dd className="break-all">{loopbackOrigin ?? "this laplus server on loopback"}</dd>
        <dt>Observed</dt>
        <dd>Inactive — no connector is serving it</dd>
      </dl>
      <div className="rounded-md border border-border/70 bg-muted/20 p-3 text-xs text-muted-foreground">
        Dedicating this tunnel lets laplus retrieve its run credential, write its own connector
        configuration, and supervise the connector. Its Cloudflare allocation and DNS record stay
        owned outside laplus and can never be deleted from here. The hostname is public unless you
        independently protect it; laplus authentication remains required.
      </div>
      <p className="text-xs text-muted-foreground">
        Not dedicated yet. laplus manages nothing until you confirm, so this tunnel is still only a
        choice.
      </p>
      {canWrite ? (
        <Button size="sm" disabled={busy} onClick={onConfirm}>
          {busy ? "Working…" : "Dedicate this tunnel"}
        </Button>
      ) : null}
    </section>
  );
}

/**
 * A dedicated tunnel laplus is supervising: an adopted one today, a
 * laplus-created one when ticket 06 lands.
 *
 * **Deliberately not {@link ManagedCloudflareConnectorPanel}.** That panel's
 * controls are a hostname and a connector token, and a dedicated tunnel has
 * neither to offer: Cloudflare does not hold its configuration, laplus does, and
 * the hostname is the one the dedication was confirmed against. What is left is
 * what the connector is doing, and the lifecycle actions that do not change
 * ownership.
 *
 * **`deletableAtCloudflare` is the server's answer, not a layout decision.** It
 * is `TunnelOwnership::deletable_at_cloudflare` in `public_exposure.rs` — the
 * same value ticket 07's deletion command refuses on — so an adopted tunnel is
 * never offered a control that would be refused, and the sentence says why
 * rather than leaving a developer to wonder where the option went.
 */
export function CloudflareDedicatedConnectorPanel({
  snapshot,
  canWrite,
  busy,
  onStart,
  onStop,
  onRetry,
}: {
  readonly snapshot: ManagedCloudflareConnectorSnapshot;
  readonly canWrite: boolean;
  readonly busy: boolean;
  readonly onStart: () => void;
  readonly onStop: () => void;
  readonly onRetry: () => void;
}) {
  return (
    <section className="space-y-3" aria-label="Dedicated Cloudflare tunnel">
      <div>
        <p className="text-sm font-medium">
          {snapshot.httpsOrigin} · {cloudflareOwnershipLabel(snapshot.tunnelOwnership)}
        </p>
        <p className="text-xs text-muted-foreground">
          laplus configures and supervises this connector from its own credential and configuration.
        </p>
      </div>
      <CloudflareConnectorStatus snapshot={snapshot} />
      <div className="rounded-md border border-border/70 bg-muted/20 p-3 text-xs text-muted-foreground">
        {snapshot.deletableAtCloudflare
          ? "laplus created this tunnel and its DNS record, so it can also delete them — separately, and with its own confirmation."
          : "laplus did not create this tunnel or its DNS record, so it can never delete either. Stopping the connector or forgetting the local setup leaves both untouched."}
      </div>
      {canWrite ? (
        <div className="flex flex-wrap gap-2">
          {snapshot.desiredState === "stopped" ? (
            <Button size="sm" disabled={busy} onClick={onStart}>
              Start connector
            </Button>
          ) : (
            <Button size="sm" variant="outline" disabled={busy} onClick={onStop}>
              Stop connector
            </Button>
          )}
          {snapshot.connectorState === "restart-exhausted" ||
          snapshot.connectorState === "failed" ? (
            <Button size="sm" disabled={busy} onClick={onRetry}>
              Retry connector
            </Button>
          ) : null}
        </div>
      ) : null}
    </section>
  );
}

/** What the connector is doing, shared by both connector panels. */
export function CloudflareConnectorStatus({
  snapshot,
}: {
  readonly snapshot: ManagedCloudflareConnectorSnapshot;
}) {
  return (
    <div className="rounded-md border border-border/70 p-3 text-xs">
      <p>
        Connector{" "}
        {snapshot.readiness === true
          ? "ready"
          : snapshot.readiness === false
            ? "not ready"
            : "readiness unknown"}
      </p>
      <p>Public endpoint {snapshot.verificationState}</p>
      {snapshot.failureMessage ? (
        <p className="text-destructive">{snapshot.failureMessage}</p>
      ) : null}
      {snapshot.publicFailureMessage ? (
        <p className="text-destructive">{snapshot.publicFailureMessage}</p>
      ) : null}
      {snapshot.logs.length > 0 ? (
        <pre className="mt-2 whitespace-pre-wrap text-muted-foreground">
          {snapshot.logs.join("\n")}
        </pre>
      ) : null}
    </div>
  );
}

/**
 * The cloudflared executables discovery found, as a list rather than a path to
 * retype.
 *
 * The server already looks for them and reports each one's source, version and
 * compatibility; leaving that in a bare text field meant a developer had to
 * know a path the server could have told them. A hand-typed path stays possible
 * and joins the list, so what is selected is visible either way.
 */
export function CloudflaredExecutablePicker({
  executables,
  executablePath,
  canWrite,
  busy,
  onExecutablePathChange,
}: {
  readonly executables: ReadonlyArray<CloudflaredExecutable>;
  readonly executablePath: string;
  readonly canWrite: boolean;
  readonly busy: boolean;
  readonly onExecutablePathChange: (value: string) => void;
}) {
  const selectable = selectableCloudflaredExecutables(executables, executablePath);
  return (
    <section className="space-y-2" aria-label="cloudflared executable">
      <p className="text-xs font-medium">cloudflared executable</p>
      {selectable.length === 0 ? (
        <p className="text-xs text-muted-foreground">
          No cloudflared was found on this machine. Enter a path below, or install one above.
        </p>
      ) : (
        <ul className="space-y-1">
          {selectable.map((executable) => (
            <li key={executable.path}>
              <label className="flex cursor-pointer gap-2 rounded-md border border-border/70 p-2 text-xs">
                <input
                  type="radio"
                  name="cloudflared-executable"
                  className="mt-0.5"
                  value={executable.path}
                  checked={executable.path === executablePath.trim()}
                  disabled={busy || !canWrite}
                  onChange={() => onExecutablePathChange(executable.path)}
                />
                <span className="min-w-0">
                  <span className="block break-all font-medium">{executable.path}</span>
                  <span className="block text-muted-foreground">
                    {cloudflaredExecutableSummary(executable)}
                  </span>
                  {executable.compatibility === "incompatible" ? (
                    <span className="block text-destructive">
                      {executable.failureMessage ?? "This cloudflared executable is incompatible."}
                    </span>
                  ) : null}
                </span>
              </label>
            </li>
          ))}
        </ul>
      )}
      <label className="block">
        <span className="text-xs text-muted-foreground">Or enter a path</span>
        <Input
          className="mt-1"
          aria-label="cloudflared executable path"
          value={executablePath}
          disabled={busy || !canWrite}
          onChange={(event) => onExecutablePathChange(event.target.value)}
        />
      </label>
    </section>
  );
}

export function cloudflaredExecutableSummary(executable: CloudflaredExecutable): string {
  const source =
    executable.source === "app-managed"
      ? "Installed by laplus"
      : executable.source === "user-selected"
        ? "Chosen by you"
        : executable.source === "system"
          ? "Found on this machine"
          : "Unknown source";
  const version = executable.version ? ` · ${executable.version}` : "";
  const compatibility =
    executable.compatibility === "compatible"
      ? " · compatible"
      : executable.compatibility === "incompatible"
        ? " · incompatible"
        : "";
  return `${source}${version}${compatibility}`;
}

/**
 * Whether the wizard should offer to install `cloudflared` at all.
 *
 * Only when this environment has nothing usable of its own: a compatible system
 * or user-selected executable is preferred and is never replaced, so offering a
 * download beside one would invite an installation nobody needs. An executable
 * laplus already installed keeps the panel visible, because that is where its
 * ownership and version are stated.
 *
 * Decided from discovery alone, and deliberately so: reading the installation
 * snapshot reaches Cloudflare's release feed, and an environment that will never
 * be offered a download should not put that request on the wire every time this
 * dialog opens.
 */
export function offersCloudflaredInstallation(
  executables: ReadonlyArray<CloudflaredExecutable>,
): boolean {
  if (executables.some((executable) => executable.source === "app-managed")) {
    return true;
  }
  return !executables.some(
    (executable) =>
      executable.compatibility === "compatible" && executable.source !== "app-managed",
  );
}

export function CloudflaredInstallationPanel({
  snapshot,
  canWrite,
  busy,
  onInstall,
}: {
  readonly snapshot: CloudflaredInstallationSnapshot | null;
  readonly canWrite: boolean;
  readonly busy: boolean;
  readonly onInstall: (release: CloudflaredRelease) => void;
}) {
  if (!snapshot) {
    return null;
  }
  if (snapshot.state === "installed") {
    return (
      <section
        className="rounded-md border border-border/70 p-3 text-xs"
        aria-label="App-managed cloudflared"
      >
        <p className="text-sm font-medium">cloudflared {snapshot.installedVersion} installed</p>
        <p className="text-muted-foreground">{snapshot.installedPath}</p>
        {snapshot.detectedVersion &&
        !snapshot.detectedVersion.includes(snapshot.installedVersion ?? "") ? (
          <p className="text-muted-foreground">
            Now reporting {snapshot.detectedVersion} — cloudflared updates itself, and laplus does
            not manage that.
          </p>
        ) : null}
        <p className="mt-1 text-muted-foreground">
          Installed and owned by laplus. It stays inside laplus&rsquo;s own data, your PATH is
          unchanged, and laplus replaces or removes only this copy.
        </p>
      </section>
    );
  }
  if (!snapshot.supported) {
    return (
      <section
        className="rounded-md border border-border/70 p-3 text-xs text-muted-foreground"
        aria-label="App-managed cloudflared"
      >
        <p className="text-sm font-medium text-foreground">
          laplus cannot install cloudflared here
        </p>
        <p>{snapshot.unsupportedMessage}</p>
      </section>
    );
  }
  const release = snapshot.release;
  return (
    <section
      className="space-y-2 rounded-md border border-border/70 p-3 text-xs"
      aria-label="App-managed cloudflared"
    >
      <p className="text-sm font-medium">Install cloudflared with laplus</p>
      {release ? (
        <dl className="grid grid-cols-[auto_1fr] gap-x-3 text-muted-foreground">
          <dt>Release</dt>
          <dd>{release.version}</dd>
          <dt>Platform</dt>
          <dd>
            {snapshot.platform} {snapshot.architecture}
          </dd>
          <dt>Artifact</dt>
          <dd>{release.assetName}</dd>
          <dt>Source</dt>
          <dd className="break-all">{release.downloadUrl}</dd>
          <dt>SHA-256</dt>
          <dd className="break-all">{release.checksum}</dd>
          <dt>Ownership</dt>
          <dd>
            App-managed: kept in laplus&rsquo;s data directory, with no PATH change and no
            elevation. A system executable is never overwritten.
          </dd>
        </dl>
      ) : (
        <p className="text-muted-foreground">{snapshot.releaseFailureMessage}</p>
      )}
      {snapshot.failureMessage ? (
        <p className="text-destructive">{snapshot.failureMessage}</p>
      ) : null}
      {canWrite && release ? (
        <Button
          size="sm"
          disabled={busy || snapshot.state === "installing"}
          onClick={() => onInstall(release)}
        >
          {snapshot.state === "installing"
            ? "Downloading and verifying…"
            : snapshot.state === "failed"
              ? `Retry cloudflared ${release.version}`
              : `Download and verify cloudflared ${release.version}`}
        </Button>
      ) : null}
    </section>
  );
}

export function managedCloudflareCompactState(
  snapshot: ManagedCloudflareConnectorSnapshot,
): string {
  switch (snapshot.connectorState) {
    case "unconfigured":
      return "Not configured";
    case "starting":
      return "Starting";
    case "ready":
      return snapshot.verificationState === "verified" ? "Publicly verified" : "Locally ready";
    case "degraded":
      return "Degraded";
    case "restart-exhausted":
      return "Restart exhausted";
    case "stopping":
      return "Stopping";
    case "stopped":
      return "Stopped";
    case "failed":
      return "Setup failed";
  }
}

export function ManagedCloudflareConnectorPanel({
  snapshot,
  executables,
  canWrite,
  busy,
  hostname,
  executablePath,
  connectorToken,
  onHostnameChange,
  onExecutablePathChange,
  onConnectorTokenChange,
  onConfigure,
  onStart,
  onStop,
  onRetry,
}: {
  readonly snapshot: ManagedCloudflareConnectorSnapshot | null;
  readonly executables: ReadonlyArray<CloudflaredExecutable>;
  readonly canWrite: boolean;
  readonly busy: boolean;
  readonly hostname: string;
  readonly executablePath: string;
  readonly connectorToken: string;
  readonly onHostnameChange: (value: string) => void;
  readonly onExecutablePathChange: (value: string) => void;
  readonly onConnectorTokenChange: (value: string) => void;
  readonly onConfigure: () => void;
  readonly onStart: () => void;
  readonly onStop: () => void;
  readonly onRetry: () => void;
}) {
  const incompatible = executables.find(
    (item) => item.path === executablePath && item.compatibility === "incompatible",
  );
  return (
    <section className="space-y-3" aria-label="Laplus-managed Cloudflare connector">
      <div>
        <p className="text-sm font-medium">Run a connector with laplus</p>
        <p className="text-xs text-muted-foreground">
          Use an existing compatible cloudflared executable and a tunnel connector token. Cloudflare
          retains control-plane ownership.
        </p>
      </div>
      <label className="block">
        <span className="text-xs font-medium">Hostname</span>
        <Input
          className="mt-1"
          aria-label="Managed connector hostname"
          value={hostname}
          disabled={busy || !canWrite}
          onChange={(event) => onHostnameChange(event.target.value)}
        />
      </label>
      <CloudflaredExecutablePicker
        executables={executables}
        executablePath={executablePath}
        canWrite={canWrite}
        busy={busy}
        onExecutablePathChange={onExecutablePathChange}
      />
      {incompatible ? (
        <p className="text-xs text-destructive">
          {incompatible.failureMessage ?? "This cloudflared executable is incompatible."}
        </p>
      ) : null}
      <label className="block">
        <span className="text-xs font-medium">Connector token</span>
        <Input
          className="mt-1"
          aria-label="Connector token"
          type="password"
          autoComplete="off"
          value={connectorToken}
          placeholder={
            snapshot?.configured
              ? "Stored privately; enter only to replace"
              : "Paste tunnel connector token"
          }
          disabled={busy || !canWrite}
          onChange={(event) => onConnectorTokenChange(event.target.value)}
        />
      </label>
      {snapshot ? <CloudflareConnectorStatus snapshot={snapshot} /> : null}
      {canWrite ? (
        <div className="flex flex-wrap gap-2">
          <Button
            size="sm"
            variant="outline"
            disabled={
              busy ||
              hostname.trim() === "" ||
              executablePath.trim() === "" ||
              (!snapshot?.configured && connectorToken.trim() === "") ||
              incompatible !== undefined
            }
            onClick={onConfigure}
          >
            Save connector
          </Button>
          {snapshot?.configured && snapshot.desiredState === "stopped" ? (
            <Button size="sm" disabled={busy} onClick={onStart}>
              Start connector
            </Button>
          ) : null}
          {snapshot?.configured && snapshot.desiredState === "running" ? (
            <Button size="sm" variant="outline" disabled={busy} onClick={onStop}>
              Stop connector
            </Button>
          ) : null}
          {snapshot?.connectorState === "restart-exhausted" ||
          snapshot?.connectorState === "failed" ? (
            <Button size="sm" disabled={busy} onClick={onRetry}>
              Retry connector
            </Button>
          ) : null}
        </div>
      ) : null}
    </section>
  );
}

export function CloudflareLayeredHealth({
  snapshot,
}: {
  readonly snapshot: ExternalTunnelEndpointSnapshot;
}) {
  return (
    <p className="text-xs text-muted-foreground">
      Connector {snapshot.health.connector} · HTTPS {snapshot.health.https} · WebSocket{" "}
      {snapshot.health.webSocket}
    </p>
  );
}

const accessTimestampFormatter = new Intl.DateTimeFormat(undefined, {
  dateStyle: "medium",
  timeStyle: "short",
});

function formatAccessTimestamp(value: string): string {
  const parsed = new Date(value);
  if (Number.isNaN(parsed.getTime())) {
    return value;
  }
  return accessTimestampFormatter.format(parsed);
}

const PAIRING_SCOPE_OPTIONS: ReadonlyArray<{
  readonly scope: AuthEnvironmentScope;
  readonly title: string;
  readonly description: string;
}> = [
  {
    scope: AuthOrchestrationReadScope,
    title: "View environment",
    description: "Read threads, status, diffs, and configuration.",
  },
  {
    scope: AuthOrchestrationOperateScope,
    title: "Operate tasks",
    description: "Start tasks and perform changes in the environment.",
  },
  {
    scope: AuthTerminalOperateScope,
    title: "Use terminals",
    description: "Create terminals and send input to running shells.",
  },
  {
    scope: AuthReviewWriteScope,
    title: "Write reviews",
    description: "Create comments while reviewing changes.",
  },
  {
    scope: AuthAccessReadScope,
    title: "View access",
    description: "Inspect pairing links and authorized clients.",
  },
  {
    scope: AuthAccessWriteScope,
    title: "Manage access",
    description: "Issue and revoke credentials for other clients.",
  },
  {
    scope: AuthRelayReadScope,
    title: "View relay",
    description: "Inspect managed relay connectivity.",
  },
  {
    scope: AuthRelayWriteScope,
    title: "Manage relay",
    description: "Change managed tunnel connectivity.",
  },
];

function AccessScopeSummary({
  scopes,
  label,
}: {
  readonly scopes: ReadonlyArray<AuthEnvironmentScope>;
  readonly label: string;
}) {
  const scopeCountLabel = `${scopes.length} ${scopes.length === 1 ? "scope" : "scopes"}`;

  return (
    <Popover>
      <PopoverTrigger
        openOnHover
        delay={250}
        closeDelay={100}
        render={
          <button
            type="button"
            aria-label={`${label}: show ${scopeCountLabel}`}
            className="cursor-help underline decoration-border underline-offset-2 outline-hidden hover:text-foreground focus-visible:text-foreground"
          />
        }
      >
        {scopeCountLabel}
      </PopoverTrigger>
      <PopoverPopup
        side="top"
        align="start"
        tooltipStyle
        className="w-max max-w-80 whitespace-normal"
      >
        <p className="mb-1 font-medium">Granted scopes</p>
        <div className="flex flex-col gap-0.5">
          {scopes.map((scope) => (
            <code key={scope} className="font-mono text-foreground/85">
              {scope}
            </code>
          ))}
        </div>
      </PopoverPopup>
    </Popover>
  );
}

function formatDesktopSshTarget(target: DesktopSshEnvironmentTarget): string {
  const authority = target.username ? `${target.username}@${target.hostname}` : target.hostname;
  return target.port ? `${authority}:${target.port}` : authority;
}

function parseManualDesktopSshTarget(input: {
  readonly host: string;
  readonly username: string;
  readonly port: string;
}): DesktopSshEnvironmentTarget {
  const rawHost = input.host.trim();
  if (rawHost.length === 0) {
    throw new Error("SSH host or alias is required.");
  }

  let hostname = rawHost;
  let username = input.username.trim() || null;
  let port: number | null = null;

  const atIndex = hostname.lastIndexOf("@");
  if (atIndex > 0) {
    const inlineUsername = hostname.slice(0, atIndex).trim();
    hostname = hostname.slice(atIndex + 1).trim();
    if (!username && inlineUsername.length > 0) {
      username = inlineUsername;
    }
  }

  const bracketedHostMatch = /^\[([^\]]+)\](?::(\d+))?$/u.exec(hostname);
  if (bracketedHostMatch) {
    hostname = bracketedHostMatch[1]!.trim();
    if (bracketedHostMatch[2]) {
      port = Number.parseInt(bracketedHostMatch[2], 10);
    }
  } else {
    const colonSegments = hostname.split(":");
    if (colonSegments.length === 2 && /^\d+$/u.test(colonSegments[1] ?? "")) {
      hostname = colonSegments[0]!.trim();
      port = Number.parseInt(colonSegments[1]!, 10);
    }
  }

  const rawPort = input.port.trim();
  if (rawPort.length > 0) {
    port = Number.parseInt(rawPort, 10);
  }

  if (hostname.length === 0) {
    throw new Error("SSH host or alias is required.");
  }

  if (port !== null && (!Number.isInteger(port) || port <= 0 || port > 65_535)) {
    throw new Error("SSH port must be between 1 and 65535.");
  }

  return {
    alias: hostname,
    hostname,
    username,
    port,
  };
}

function parsePairingUrlFields(
  input: string,
): { readonly host: string; readonly pairingCode: string } | null {
  const trimmed = input.trim();
  if (!trimmed) return null;

  try {
    const urlLikeInput =
      /^[a-zA-Z][a-zA-Z\d+.-]*:\/\//u.test(trimmed) || trimmed.startsWith("//")
        ? trimmed
        : `https://${trimmed}`;
    const url = new URL(urlLikeInput, window.location.origin);
    const pairingCode = getPairingTokenFromUrl(url);
    if (!pairingCode) return null;
    return {
      host: url.origin,
      pairingCode,
    };
  } catch {
    return null;
  }
}

function parseRemotePairingFields(input: { readonly host: string; readonly pairingCode: string }): {
  readonly host: string;
  readonly pairingCode: string;
} {
  const parsedPairingUrl = parsePairingUrlFields(input.host);
  if (parsedPairingUrl) return parsedPairingUrl;

  const host = input.host.trim();
  const pairingCode = input.pairingCode.trim();
  if (!host) {
    throw new Error("Enter a backend host.");
  }
  if (!pairingCode) {
    throw new Error("Enter a pairing code.");
  }
  return { host, pairingCode };
}

function formatDesktopSshConnectionError(error: unknown): string {
  const fallback = "Failed to connect SSH host.";
  const rawMessage = error instanceof Error ? error.message : fallback;
  const withoutIpcPrefix = rawMessage.replace(
    /^Error invoking remote method 'desktop:ensure-ssh-environment':\s*/u,
    "",
  );
  const withoutTaggedErrorPrefix = withoutIpcPrefix.replace(/^Ssh[A-Za-z]+Error:\s*/u, "");
  return withoutTaggedErrorPrefix.trim() || fallback;
}

const ENDPOINT_ROW_CLASSNAME = "rounded-xl px-3 py-2.5 sm:px-4";

type AccessSectionPresentation = "current" | "endpoint-rail";

function accessRowClassName(_presentation: AccessSectionPresentation) {
  return ITEM_ROW_CLASSNAME;
}

function endpointRowClassName(presentation: AccessSectionPresentation, isAvailable: boolean) {
  if (presentation === "endpoint-rail") {
    return cn("relative rounded-xl px-3 py-3 sm:px-4", !isAvailable && "bg-muted/15");
  }

  return cn(ENDPOINT_ROW_CLASSNAME, !isAvailable && "bg-muted/24");
}

function sortDesktopPairingLinks(links: ReadonlyArray<ServerPairingLinkRecord>) {
  return [...links].toSorted(
    (left, right) => new Date(right.createdAt).getTime() - new Date(left.createdAt).getTime(),
  );
}

function sortDesktopClientSessions(sessions: ReadonlyArray<ServerClientSessionRecord>) {
  return [...sessions].toSorted((left, right) => {
    if (left.current !== right.current) {
      return left.current ? -1 : 1;
    }
    if (left.connected !== right.connected) {
      return left.connected ? -1 : 1;
    }
    return new Date(right.issuedAt).getTime() - new Date(left.issuedAt).getTime();
  });
}

function toDesktopPairingLinkRecord(pairingLink: AuthPairingLink): ServerPairingLinkRecord {
  return {
    ...pairingLink,
    createdAt: DateTime.formatIso(pairingLink.createdAt),
    expiresAt: DateTime.formatIso(pairingLink.expiresAt),
  };
}

function toDesktopClientSessionRecord(clientSession: AuthClientSession): ServerClientSessionRecord {
  return {
    ...clientSession,
    issuedAt: DateTime.formatIso(clientSession.issuedAt),
    expiresAt: DateTime.formatIso(clientSession.expiresAt),
    lastConnectedAt:
      clientSession.lastConnectedAt === null
        ? null
        : DateTime.formatIso(clientSession.lastConnectedAt),
  };
}

function selectPairingEndpoint(
  endpoints: ReadonlyArray<AdvertisedEndpoint>,
  defaultEndpointKey?: string | null,
): AdvertisedEndpoint | null {
  const availableEndpoints = endpoints.filter((endpoint) => endpoint.status !== "unavailable");
  if (defaultEndpointKey) {
    const selectedEndpoint = availableEndpoints.find(
      (endpoint) => endpointDefaultPreferenceKey(endpoint) === defaultEndpointKey,
    );
    if (selectedEndpoint) {
      return selectedEndpoint;
    }
  }
  return (
    availableEndpoints.find((endpoint) => endpoint.isDefault) ??
    availableEndpoints.find((endpoint) => endpoint.reachability !== "loopback") ??
    null
  );
}

function isTailscaleHttpsEndpoint(endpoint: AdvertisedEndpoint): boolean {
  return endpoint.id.startsWith("tailscale-magicdns:");
}

function endpointDefaultPreferenceKey(endpoint: AdvertisedEndpoint): string {
  if (endpoint.id.startsWith("desktop-loopback:")) {
    return "desktop-core:loopback:http";
  }
  if (endpoint.id.startsWith("desktop-lan:")) {
    return "desktop-core:lan:http";
  }
  if (endpoint.id.startsWith("tailscale-ip:")) {
    return "tailscale:ip:http";
  }
  if (isTailscaleHttpsEndpoint(endpoint)) {
    return "tailscale:magicdns:https";
  }

  let scheme = "unknown";
  try {
    scheme = new URL(endpoint.httpBaseUrl).protocol.replace(/:$/u, "");
  } catch {
    // Keep the stored preference stable even if a custom endpoint is malformed.
  }

  return `${endpoint.provider.id}:${endpoint.reachability}:${scheme}:${endpoint.label}`;
}

function resolveCurrentOriginPairingUrl(credential: string): string {
  const url = new URL("/pair", window.location.href);
  return setPairingTokenOnUrl(url, credential).toString();
}

type PairingLinkListRowProps = {
  pairingLink: ServerPairingLinkRecord;
  endpointUrl: string | null | undefined;
  endpoints: ReadonlyArray<AdvertisedEndpoint>;
  defaultEndpointKey: string | null;
  presentation?: AccessSectionPresentation;
  revokingPairingLinkId: string | null;
  onRevoke: (id: string) => void;
};

const PairingLinkListRow = memo(function PairingLinkListRow({
  pairingLink,
  endpointUrl,
  endpoints,
  defaultEndpointKey,
  presentation = "current",
  revokingPairingLinkId,
  onRevoke,
}: PairingLinkListRowProps) {
  const nowMs = useRelativeTimeTick(1_000);
  const expiresAtMs = useMemo(
    () => new Date(pairingLink.expiresAt).getTime(),
    [pairingLink.expiresAt],
  );
  const [isRevealDialogOpen, setIsRevealDialogOpen] = useState(false);

  const currentOriginPairingUrl = useMemo(
    () => resolveCurrentOriginPairingUrl(pairingLink.credential),
    [pairingLink.credential],
  );
  const endpointPairingUrl = useMemo(() => {
    const endpoint = selectPairingEndpoint(endpoints, defaultEndpointKey);
    return endpoint ? resolveDesktopPairingUrl(endpoint.httpBaseUrl, pairingLink.credential) : null;
  }, [defaultEndpointKey, endpoints, pairingLink.credential]);
  const endpointCopyOptions = useMemo(() => {
    const options: Array<{
      readonly key: string;
      readonly label: string;
      readonly url: string;
      readonly detail: string;
    }> = [];
    for (const endpoint of endpoints) {
      if (endpoint.status === "unavailable") {
        continue;
      }
      options.push({
        key: endpointDefaultPreferenceKey(endpoint),
        label: endpoint.label,
        url: resolveDesktopPairingUrl(endpoint.httpBaseUrl, pairingLink.credential),
        detail: "Backend pairing URL",
      });
    }
    return options;
  }, [endpoints, pairingLink.credential]);
  const shareablePairingUrl =
    endpointPairingUrl ??
    (endpointUrl != null && endpointUrl !== ""
      ? resolveDesktopPairingUrl(endpointUrl, pairingLink.credential)
      : isLoopbackHostname(window.location.hostname)
        ? null
        : currentOriginPairingUrl);
  const revealValue = shareablePairingUrl ?? pairingLink.credential;
  const canCopyToClipboard =
    typeof window !== "undefined" &&
    window.isSecureContext &&
    navigator.clipboard?.writeText != null;

  const { copyToClipboard } = useCopyToClipboard<"code" | "link">({
    onCopy: (kind) => {
      toastManager.add({
        type: "success",
        title: kind === "link" ? "Pairing URL copied" : "Pairing code copied",
        description:
          kind === "link"
            ? "Open it in the client you want to pair to this environment."
            : "Paste it into another client to finish pairing.",
      });
    },
    onError: (error, kind) => {
      setIsRevealDialogOpen(true);
      toastManager.add(
        stackedThreadToast({
          type: "error",
          title: canCopyToClipboard
            ? kind === "link"
              ? "Could not copy pairing URL"
              : "Could not copy pairing code"
            : "Clipboard copy unavailable",
          description: canCopyToClipboard ? error.message : "Showing the full value instead.",
        }),
      );
    },
  });

  const copyPairingValue = useCallback(
    (value: string, kind: "code" | "link") => {
      copyToClipboard(value, kind);
    },
    [copyToClipboard],
  );

  const handleCopyCode = useCallback(() => {
    copyPairingValue(pairingLink.credential, "code");
  }, [copyPairingValue, pairingLink.credential]);

  const handleCopyDefaultLink = useCallback(() => {
    if (!shareablePairingUrl) return;
    copyPairingValue(shareablePairingUrl, "link");
  }, [copyPairingValue, shareablePairingUrl]);

  const expiresAbsolute = formatAccessTimestamp(pairingLink.expiresAt);

  const primaryLabel = pairingLink.label ?? "Pairing link";
  const defaultEndpointCopyOption =
    endpointCopyOptions.find((option) => option.key === defaultEndpointKey) ??
    endpointCopyOptions[0] ??
    null;
  const defaultEndpointCopyLabel = defaultEndpointCopyOption?.label ?? "URL";
  const renderEndpointMenuItems = (
    options: typeof endpointCopyOptions = endpointCopyOptions,
    renderDetail = true,
  ) =>
    options.map((option) => (
      <MenuItem key={option.key} onClick={() => copyPairingValue(option.url, "link")}>
        <span className="min-w-0 flex-1">
          <span className="block truncate">{option.label}</span>
          {renderDetail ? (
            <span className="block truncate text-[11px] text-muted-foreground">
              {option.detail}
            </span>
          ) : null}
        </span>
      </MenuItem>
    ));
  const renderPairingCodeMenuItem = (renderDetail = true) => (
    <MenuItem onClick={handleCopyCode}>
      <span className="min-w-0 flex-1">
        <span className="block truncate">Copy code</span>
        {renderDetail ? (
          <span className="block truncate text-[11px] text-muted-foreground">Token only</span>
        ) : null}
      </span>
    </MenuItem>
  );
  const renderCompactEndpointGroup = (
    label: string,
    options: typeof endpointCopyOptions,
    includeSeparator: boolean,
  ) =>
    options.length > 0 ? (
      <>
        {includeSeparator ? <MenuSeparator /> : null}
        <MenuGroup>
          <MenuGroupLabel>{label}</MenuGroupLabel>
          {renderEndpointMenuItems(options, false)}
        </MenuGroup>
      </>
    ) : null;
  const renderGroupedCopyMenuItems = (options?: { codeFirst?: boolean }) => (
    <>
      {options?.codeFirst ? (
        <>
          <MenuGroup>
            <MenuGroupLabel>Pairing code</MenuGroupLabel>
            {renderPairingCodeMenuItem(false)}
          </MenuGroup>
          {endpointCopyOptions.length > 0 ? <MenuSeparator /> : null}
        </>
      ) : null}
      {renderCompactEndpointGroup("Pairing URLs", endpointCopyOptions, false)}
      {!options?.codeFirst ? (
        <>
          {endpointCopyOptions.length > 0 ? <MenuSeparator /> : null}
          <MenuGroup>
            <MenuGroupLabel>Pairing code</MenuGroupLabel>
            {renderPairingCodeMenuItem(false)}
          </MenuGroup>
        </>
      ) : null}
    </>
  );

  if (expiresAtMs <= nowMs) {
    return null;
  }

  return (
    <div className={accessRowClassName(presentation)}>
      <div className={ITEM_ROW_INNER_CLASSNAME}>
        <div className="min-w-0 flex-1 space-y-1">
          <div className="flex min-h-5 items-center gap-1.5">
            <ConnectionStatusDot
              tooltipText={`Link created at ${formatAccessTimestamp(pairingLink.createdAt)}`}
              dotClassName="bg-amber-400"
            />
            <h3 className="text-sm font-medium text-foreground">{primaryLabel}</h3>
            <Popover>
              {shareablePairingUrl ? (
                <>
                  <PopoverTrigger
                    openOnHover
                    delay={250}
                    closeDelay={100}
                    render={
                      <button
                        type="button"
                        className="inline-flex size-4 shrink-0 items-center justify-center rounded-sm text-muted-foreground/50 outline-none hover:text-foreground"
                        aria-label="Show QR code"
                      />
                    }
                  >
                    <QrCodeIcon aria-hidden className="size-3" />
                  </PopoverTrigger>
                  <PopoverPopup side="top" align="start" tooltipStyle className="w-max">
                    <QRCodeSvg
                      value={shareablePairingUrl}
                      size={88}
                      level="M"
                      marginSize={2}
                      title="Pairing link — scan to open on another device"
                    />
                  </PopoverPopup>
                </>
              ) : null}
            </Popover>
          </div>
          <p className="text-xs text-muted-foreground" title={expiresAbsolute}>
            {formatExpiresInLabel(pairingLink.expiresAt, nowMs)}
            <span aria-hidden> · </span>
            <AccessScopeSummary scopes={pairingLink.scopes} label="Pairing link scopes" />
          </p>
          {shareablePairingUrl === null ? (
            <p className="text-[11px] text-muted-foreground/70">
              Copy the token and pair from another client using this backend&apos;s reachable host.
            </p>
          ) : null}
        </div>
        <div className="flex w-full shrink-0 items-center gap-2 sm:w-auto sm:justify-end">
          <Dialog open={isRevealDialogOpen} onOpenChange={setIsRevealDialogOpen}>
            {canCopyToClipboard ? (
              <>
                {shareablePairingUrl ? (
                  <Group aria-label="Copy selected endpoint">
                    <Button
                      size="xs"
                      variant="outline"
                      className="max-w-56"
                      title={`Copy pairing URL for: ${defaultEndpointCopyLabel}`}
                      onClick={handleCopyDefaultLink}
                    >
                      <span className="truncate">
                        Copy pairing URL for: {defaultEndpointCopyLabel}
                      </span>
                    </Button>
                    <GroupSeparator />
                    <Menu>
                      <MenuTrigger
                        render={
                          <Button
                            size="icon-xs"
                            variant="outline"
                            aria-label="Choose endpoint to copy"
                          />
                        }
                      >
                        <ChevronDownIcon className="size-3.5" />
                      </MenuTrigger>
                      <MenuPopup align="end" className="min-w-60">
                        {renderGroupedCopyMenuItems()}
                      </MenuPopup>
                    </Menu>
                  </Group>
                ) : (
                  <Button size="xs" variant="outline" onClick={handleCopyCode}>
                    Copy code
                  </Button>
                )}
              </>
            ) : (
              <DialogTrigger render={<Button size="xs" variant="outline" />}>
                {shareablePairingUrl ? "Show link" : "Show code"}
              </DialogTrigger>
            )}
            <DialogPopup className="max-w-md">
              <DialogHeader>
                <DialogTitle>{shareablePairingUrl ? "Pairing link" : "Pairing code"}</DialogTitle>
                <DialogDescription>
                  {shareablePairingUrl
                    ? "Clipboard copy is unavailable here. Open or manually copy this full pairing URL on the device you want to connect."
                    : "Clipboard copy is unavailable here. Manually copy this code into another client."}
                </DialogDescription>
              </DialogHeader>
              <DialogPanel className="space-y-4">
                <Textarea
                  readOnly
                  value={revealValue}
                  rows={shareablePairingUrl ? 4 : 3}
                  className="text-xs leading-relaxed"
                  onFocus={(event) => event.currentTarget.select()}
                  onClick={(event) => event.currentTarget.select()}
                />
                {shareablePairingUrl ? (
                  <div className="flex justify-center rounded-xl border border-border/60 bg-muted/30 p-4">
                    <QRCodeSvg
                      value={shareablePairingUrl}
                      size={132}
                      level="M"
                      marginSize={2}
                      title="Pairing link — scan to open on another device"
                    />
                  </div>
                ) : null}
              </DialogPanel>
              <DialogFooter variant="bare">
                <Button variant="outline" onClick={() => setIsRevealDialogOpen(false)}>
                  Done
                </Button>
                {canCopyToClipboard ? (
                  <Button variant="outline" size="xs" onClick={handleCopyCode}>
                    Copy code
                  </Button>
                ) : null}
              </DialogFooter>
            </DialogPopup>
          </Dialog>
          <Button
            size="xs"
            variant="destructive-outline"
            disabled={revokingPairingLinkId === pairingLink.id}
            onClick={() => void onRevoke(pairingLink.id)}
          >
            {revokingPairingLinkId === pairingLink.id ? "Revoking…" : "Revoke"}
          </Button>
        </div>
      </div>
    </div>
  );
});

type ConnectedClientListRowProps = {
  clientSession: ServerClientSessionRecord;
  presentation?: AccessSectionPresentation;
  revokingClientSessionId: string | null;
  onRevokeSession: (sessionId: ServerClientSessionRecord["sessionId"]) => void;
};

const ConnectedClientListRow = memo(function ConnectedClientListRow({
  clientSession,
  presentation = "current",
  revokingClientSessionId,
  onRevokeSession,
}: ConnectedClientListRowProps) {
  const nowMs = useRelativeTimeTick(1_000);
  const isLive = clientSession.current || clientSession.connected;
  const lastConnectedAt = clientSession.lastConnectedAt;
  const statusTooltip = isLive
    ? lastConnectedAt
      ? `Connected for ${formatElapsedDurationLabel(lastConnectedAt, nowMs)}`
      : "Connected"
    : lastConnectedAt
      ? `Last connected at ${formatAccessTimestamp(lastConnectedAt)}`
      : "Not connected yet.";
  const deviceInfoBits = [
    clientSession.client.deviceType !== "unknown"
      ? clientSession.client.deviceType[0]?.toUpperCase() + clientSession.client.deviceType.slice(1)
      : null,
    clientSession.client.os ?? null,
    clientSession.client.browser ?? null,
    clientSession.client.ipAddress ?? null,
  ].filter((value): value is string => value !== null);
  const primaryLabel =
    clientSession.client.label ??
    ([clientSession.client.os, clientSession.client.browser].filter(Boolean).join(" · ") ||
      clientSession.subject);

  return (
    <div className={accessRowClassName(presentation)}>
      <div className={ITEM_ROW_INNER_CLASSNAME}>
        <div className="min-w-0 flex-1 space-y-1">
          <div className="flex min-h-5 items-center gap-1.5">
            <ConnectionStatusDot
              tooltipText={statusTooltip}
              dotClassName={isLive ? "bg-success" : "bg-muted-foreground/30"}
              pingClassName={isLive ? "bg-success/60 duration-2000" : null}
            />
            <h3 className="text-sm font-medium text-foreground">{primaryLabel}</h3>
            {clientSession.current ? (
              <span className="text-[10px] text-muted-foreground/80 rounded-md border border-border/50 bg-muted/50 px-1 py-0.5">
                This device
              </span>
            ) : null}
          </div>
          <p className="text-xs text-muted-foreground">
            {deviceInfoBits.length > 0 ? (
              <>
                {deviceInfoBits.join(" · ")}
                <span aria-hidden> · </span>
              </>
            ) : null}
            <AccessScopeSummary scopes={clientSession.scopes} label="Client scopes" />
          </p>
        </div>
        <div className="flex w-full shrink-0 items-center gap-2 sm:w-auto sm:justify-end">
          {!clientSession.current ? (
            <Button
              size="xs"
              variant="destructive-outline"
              disabled={revokingClientSessionId === clientSession.sessionId}
              onClick={() => void onRevokeSession(clientSession.sessionId)}
            >
              {revokingClientSessionId === clientSession.sessionId ? "Revoking…" : "Revoke"}
            </Button>
          ) : null}
        </div>
      </div>
    </div>
  );
});

type AuthorizedClientsHeaderActionProps = {
  clientSessions: ReadonlyArray<ServerClientSessionRecord>;
  isRevokingOtherClients: boolean;
  onRevokeOtherClients: () => void;
};

const AuthorizedClientsHeaderAction = memo(function AuthorizedClientsHeaderAction({
  clientSessions,
  isRevokingOtherClients,
  onRevokeOtherClients,
}: AuthorizedClientsHeaderActionProps) {
  const [dialogOpen, setDialogOpen] = useState(false);
  const [pairingLabel, setPairingLabel] = useState("");
  const [pairingScopes, setPairingScopes] = useState<ReadonlyArray<AuthEnvironmentScope>>([
    ...AuthStandardClientScopes,
  ]);
  const [isCreatingPairingLink, setIsCreatingPairingLink] = useState(false);

  const handleCreatePairingLink = useCallback(async () => {
    setIsCreatingPairingLink(true);
    try {
      await createServerPairingCredential({ label: pairingLabel, scopes: pairingScopes });
      setPairingLabel("");
      setPairingScopes([...AuthStandardClientScopes]);
      setDialogOpen(false);
    } catch (error) {
      const message = error instanceof Error ? error.message : "Failed to create pairing URL.";
      toastManager.add(
        stackedThreadToast({
          type: "error",
          title: "Could not create pairing URL",
          description: message,
        }),
      );
    } finally {
      setIsCreatingPairingLink(false);
    }
  }, [pairingLabel, pairingScopes]);

  const togglePairingScope = useCallback((scope: AuthEnvironmentScope, checked: boolean) => {
    setPairingScopes((current) =>
      checked ? [...current, scope] : current.filter((currentScope) => currentScope !== scope),
    );
  }, []);

  return (
    <div className="flex items-center gap-2">
      <Button
        size="xs"
        variant="destructive-outline"
        disabled={
          isRevokingOtherClients || clientSessions.every((clientSession) => clientSession.current)
        }
        onClick={() => void onRevokeOtherClients()}
      >
        {isRevokingOtherClients ? "Revoking…" : "Revoke others"}
      </Button>
      <Dialog
        open={dialogOpen}
        onOpenChange={(open) => {
          setDialogOpen(open);
          if (!open) {
            setPairingLabel("");
            setPairingScopes([...AuthStandardClientScopes]);
          }
        }}
      >
        <DialogTrigger
          render={
            <Button size="xs" variant="default">
              <PlusIcon className="size-3" />
              Create link
            </Button>
          }
        />
        <DialogPopup className="max-w-md">
          <DialogHeader>
            <DialogTitle>Create pairing link</DialogTitle>
            <DialogDescription>
              Generate a one-time link that another device can use to pair with this backend as an
              authorized client.
            </DialogDescription>
          </DialogHeader>
          <DialogPanel className="space-y-5">
            <label className="block">
              <span className="mb-1.5 block text-xs font-medium text-foreground">
                Client label (optional)
              </span>
              <Input
                value={pairingLabel}
                onChange={(event) => setPairingLabel(event.target.value)}
                placeholder="e.g. Living room iPad"
                disabled={isCreatingPairingLink}
                autoFocus
              />
            </label>
            <section className="space-y-3">
              <div className="flex items-center justify-between gap-3">
                <div>
                  <h3 className="text-xs font-medium text-foreground">Permissions</h3>
                  <p className="text-xs text-muted-foreground">
                    Limit what the paired client can do.
                  </p>
                </div>
                <div className="flex gap-1">
                  <Button
                    size="xs"
                    variant="outline"
                    disabled={isCreatingPairingLink}
                    onClick={() => setPairingScopes([AuthOrchestrationReadScope])}
                  >
                    Read only
                  </Button>
                  <Button
                    size="xs"
                    variant="outline"
                    disabled={isCreatingPairingLink}
                    onClick={() => setPairingScopes([...AuthStandardClientScopes])}
                  >
                    Standard
                  </Button>
                </div>
              </div>
              <div className="divide-y divide-border/60 rounded-lg border border-input bg-muted/25">
                {PAIRING_SCOPE_OPTIONS.map(({ scope, title, description }) => (
                  <label
                    key={scope}
                    className="flex cursor-pointer items-start gap-3 px-3 py-2.5 transition-colors hover:bg-muted/40"
                  >
                    <Checkbox
                      className="mt-0.5"
                      checked={pairingScopes.includes(scope)}
                      disabled={isCreatingPairingLink}
                      onCheckedChange={(checked) => togglePairingScope(scope, checked === true)}
                    />
                    <span className="min-w-0">
                      <span className="block text-xs font-medium text-foreground">{title}</span>
                      <span className="block text-xs leading-snug text-muted-foreground">
                        {description}
                      </span>
                    </span>
                  </label>
                ))}
              </div>
              {pairingScopes.length === 0 ? (
                <p className="text-xs text-destructive">Select at least one permission.</p>
              ) : pairingScopes.includes(AuthAccessWriteScope) ? (
                <p className="text-xs text-warning">
                  This client can create or revoke access for other devices.
                </p>
              ) : null}
            </section>
          </DialogPanel>
          <DialogFooter variant="bare">
            <Button
              variant="outline"
              disabled={isCreatingPairingLink}
              onClick={() => setDialogOpen(false)}
            >
              Cancel
            </Button>
            <Button
              disabled={isCreatingPairingLink || pairingScopes.length === 0}
              onClick={() => void handleCreatePairingLink()}
            >
              {isCreatingPairingLink ? "Creating…" : "Create link"}
            </Button>
          </DialogFooter>
        </DialogPopup>
      </Dialog>
    </div>
  );
});

type PairingClientsListProps = {
  endpointUrl: string | null | undefined;
  endpoints: ReadonlyArray<AdvertisedEndpoint>;
  defaultEndpointKey: string | null;
  presentation?: AccessSectionPresentation;
  isLoading: boolean;
  pairingLinks: ReadonlyArray<ServerPairingLinkRecord>;
  clientSessions: ReadonlyArray<ServerClientSessionRecord>;
  revokingPairingLinkId: string | null;
  revokingClientSessionId: string | null;
  onRevokePairingLink: (id: string) => void;
  onRevokeClientSession: (sessionId: ServerClientSessionRecord["sessionId"]) => void;
};

const PairingClientsList = memo(function PairingClientsList({
  endpointUrl,
  endpoints,
  defaultEndpointKey,
  presentation = "current",
  isLoading,
  pairingLinks,
  clientSessions,
  revokingPairingLinkId,
  revokingClientSessionId,
  onRevokePairingLink,
  onRevokeClientSession,
}: PairingClientsListProps) {
  return (
    <>
      {pairingLinks.map((pairingLink) => (
        <PairingLinkListRow
          key={pairingLink.id}
          pairingLink={pairingLink}
          endpointUrl={endpointUrl}
          endpoints={endpoints}
          defaultEndpointKey={defaultEndpointKey}
          presentation={presentation}
          revokingPairingLinkId={revokingPairingLinkId}
          onRevoke={onRevokePairingLink}
        />
      ))}

      {clientSessions.map((clientSession) => (
        <ConnectedClientListRow
          key={clientSession.sessionId}
          clientSession={clientSession}
          presentation={presentation}
          revokingClientSessionId={revokingClientSessionId}
          onRevokeSession={onRevokeClientSession}
        />
      ))}

      {pairingLinks.length === 0 && clientSessions.length === 0 && !isLoading ? (
        <div className={accessRowClassName(presentation)}>
          <p className="text-xs text-muted-foreground/60">No pairing links or client sessions.</p>
        </div>
      ) : null}
    </>
  );
});

type AdvertisedEndpointListRowProps = {
  endpoint: AdvertisedEndpoint;
  isDefault: boolean;
  presentation?: AccessSectionPresentation;
  onSetDefault: (endpoint: AdvertisedEndpoint) => void;
  onSetupTailscaleServe: (endpoint: AdvertisedEndpoint) => void;
  onDisableTailscaleServe: (endpoint: AdvertisedEndpoint) => void;
  isUpdatingTailscaleServe: boolean;
};

const AdvertisedEndpointListRow = memo(function AdvertisedEndpointListRow({
  endpoint,
  isDefault,
  presentation = "current",
  onSetDefault,
  onSetupTailscaleServe,
  onDisableTailscaleServe,
  isUpdatingTailscaleServe,
}: AdvertisedEndpointListRowProps) {
  const isAvailable = endpoint.status === "available";
  const needsTailscaleSetup = isTailscaleHttpsEndpoint(endpoint) && endpoint.status !== "available";
  const canDisableTailscaleServe =
    isTailscaleHttpsEndpoint(endpoint) && endpoint.status === "available";
  const shouldShowEndpointUrl = !needsTailscaleSetup;
  const isEndpointRail = presentation === "endpoint-rail";
  return (
    <div className={endpointRowClassName(presentation, isAvailable)}>
      {isEndpointRail && isDefault ? (
        <span className="absolute inset-y-2 left-0 w-1 rounded-r-full bg-primary" aria-hidden />
      ) : null}
      <div className="flex min-h-6 min-w-0 flex-col gap-2 sm:-my-0.5 sm:flex-row sm:items-center">
        <div className="flex min-w-0 items-baseline gap-3">
          <h3 className="shrink-0 text-sm leading-5 font-medium text-foreground">
            {endpoint.label}
          </h3>
          {shouldShowEndpointUrl ? (
            <p
              className="min-w-0 truncate text-xs leading-5 text-muted-foreground"
              title={endpoint.httpBaseUrl}
            >
              {endpoint.httpBaseUrl}
            </p>
          ) : null}
          {!isAvailable ? (
            <span className="shrink-0 rounded-md border border-border/70 px-1 py-0.5 text-[10px] text-muted-foreground">
              Setup required
            </span>
          ) : null}
        </div>
        <div className="ml-auto flex min-h-6 shrink-0 items-center justify-end gap-2">
          {isDefault ? (
            <span className="rounded-md border border-primary/30 bg-primary/10 px-1 py-0.5 text-[10px] text-primary">
              Default
            </span>
          ) : null}
          {needsTailscaleSetup ? (
            <Button
              size="xs"
              variant="outline"
              onClick={() => onSetupTailscaleServe(endpoint)}
              disabled={isUpdatingTailscaleServe}
            >
              {isUpdatingTailscaleServe ? "Restarting…" : "Setup"}
            </Button>
          ) : null}
          {canDisableTailscaleServe ? (
            <Button
              size="xs"
              variant="destructive-outline"
              onClick={() => onDisableTailscaleServe(endpoint)}
              disabled={isUpdatingTailscaleServe}
            >
              {isUpdatingTailscaleServe ? "Restarting…" : "Disable"}
            </Button>
          ) : null}
          {!needsTailscaleSetup && !isDefault ? (
            <Button size="xs" variant="outline" onClick={() => onSetDefault(endpoint)}>
              Set as default
            </Button>
          ) : null}
        </div>
      </div>
    </div>
  );
});

function NetworkAccessDescription({
  endpoint,
  hiddenEndpointCount,
  expanded,
  onToggleExpanded,
  fallback,
}: {
  endpoint: AdvertisedEndpoint | null;
  hiddenEndpointCount: number;
  expanded: boolean;
  onToggleExpanded: () => void;
  fallback: ReactNode;
}) {
  if (!endpoint) {
    return fallback;
  }

  const summary = (
    <>
      <span className="min-w-0 truncate">{endpoint.httpBaseUrl}</span>
      {hiddenEndpointCount > 0 ? (
        <span className="shrink-0 text-xs font-medium">
          {expanded ? "Hide" : `+${hiddenEndpointCount}`}
        </span>
      ) : null}
    </>
  );

  return (
    <span className="inline-flex min-w-0 max-w-full items-baseline gap-1">
      <span className="shrink-0">Reachable at</span>
      {hiddenEndpointCount > 0 ? (
        <button
          type="button"
          className="inline-flex min-w-0 max-w-full items-baseline gap-2 border-b border-dotted border-muted-foreground/60 text-left text-muted-foreground underline-offset-4 hover:border-foreground hover:text-foreground"
          onClick={onToggleExpanded}
          aria-expanded={expanded}
        >
          {summary}
        </button>
      ) : (
        <span className="inline-flex min-w-0 max-w-full items-baseline gap-2">{summary}</span>
      )}
    </span>
  );
}

type SavedBackendListRowProps = {
  environment: EnvironmentPresentation;
  removingEnvironmentId: EnvironmentId | null;
  onConnect: (environmentId: EnvironmentId) => void;
  onRemove: (environmentId: EnvironmentId) => void;
};

function SavedBackendListRow({
  environment,
  removingEnvironmentId,
  onConnect,
  onRemove,
}: SavedBackendListRowProps) {
  const environmentId = environment.environmentId;
  const connectionState = environment.connection.phase;
  const isConnected = connectionState === "connected";
  const isConnecting = connectionState === "connecting" || connectionState === "reconnecting";
  const stateDotClassName =
    connectionState === "connected"
      ? "bg-success"
      : connectionState === "connecting" || connectionState === "reconnecting"
        ? "bg-warning"
        : connectionState === "error"
          ? "bg-destructive"
          : "bg-muted-foreground/40";
  const statusTooltip = connectionStatusText(environment.connection);
  const errorTraceId = environment.connection.traceId;
  const { copyToClipboard: copyTraceIdToClipboard } = useCopyToClipboard<{ traceId: string }>({
    target: "trace ID",
    onCopy: ({ traceId }) => {
      toastManager.add({
        type: "success",
        title: "Trace ID copied",
        description: traceId,
      });
    },
    onError: (error) => {
      toastManager.add(
        stackedThreadToast({
          type: "error",
          title: "Could not copy trace ID",
          description: error.message,
        }),
      );
    },
  });
  const copyTraceId = useCallback(
    (traceId: string) => {
      copyTraceIdToClipboard(traceId, { traceId });
    },
    [copyTraceIdToClipboard],
  );
  const versionMismatch = resolveServerConfigVersionMismatch(environment.serverConfig);
  const sshTarget =
    environment.entry.target._tag === "SshConnectionTarget" &&
    Option.isSome(environment.entry.profile) &&
    environment.entry.profile.value._tag === "SshConnectionProfile"
      ? environment.entry.profile.value.target
      : null;
  // The host a remote link was paired with, which is the only thing in this row
  // that differs between two laplus servers on one machine: they share a
  // hostname, so they share `environment.label`. See
  // `formatRemoteBackendHost`, which carries the whole reasoning.
  const remoteHost =
    Option.isSome(environment.entry.profile) &&
    environment.entry.profile.value._tag === "BearerConnectionProfile"
      ? formatRemoteBackendHost(environment.entry.profile.value.httpBaseUrl)
      : null;
  const metadataBits = [
    sshTarget ? `SSH ${formatDesktopSshTarget(sshTarget)}` : null,
    remoteHost,
  ].filter((value): value is string => value !== null);

  return (
    <div className={ITEM_ROW_CLASSNAME}>
      <div className={ITEM_ROW_INNER_CLASSNAME}>
        <div className="min-w-0 flex-1 space-y-1">
          <div className="flex min-h-5 items-center gap-1.5">
            <ConnectionStatusDot
              tooltipText={statusTooltip}
              dotClassName={stateDotClassName}
              pingClassName={
                connectionState === "connecting" || connectionState === "reconnecting"
                  ? "bg-warning/60 duration-2000"
                  : null
              }
            />
            <h3 className="text-sm font-medium text-foreground">{environment.label}</h3>
          </div>
          {metadataBits.length > 0 ? (
            <p className="text-xs text-muted-foreground">{metadataBits.join(" · ")}</p>
          ) : null}
          {versionMismatch ? (
            <div className="flex flex-wrap items-center gap-2">
              <p className="flex items-center gap-1 text-warning text-xs">
                <TriangleAlertIcon className="size-3.5 shrink-0" />
                Version drift: client {versionMismatch.clientVersion}, server{" "}
                {versionMismatch.serverVersion}.
              </p>
              <ServerUpdateAction
                environmentId={environmentId}
                serverLabel={`${environment.label} server`}
                selfUpdate={resolveServerSelfUpdateCapability(environment.serverConfig)}
                targetVersion={versionMismatch.clientVersion}
              />
            </div>
          ) : null}
          {environment.connection.error ? (
            <p className="flex min-w-0 items-center gap-2 text-destructive text-xs">
              <span className="truncate">{connectionStatusText(environment.connection)}</span>
              {errorTraceId ? (
                <button
                  type="button"
                  className="shrink-0 underline underline-offset-2"
                  onClick={() => copyTraceId(errorTraceId)}
                >
                  Copy trace ID
                </button>
              ) : null}
            </p>
          ) : null}
        </div>
        <div className="flex w-full shrink-0 items-center gap-2 sm:w-auto sm:justify-end">
          {
            <>
              {!isConnected ? (
                <Button
                  size="xs"
                  variant="outline"
                  disabled={removingEnvironmentId === environmentId}
                  onClick={() => void onRemove(environmentId)}
                >
                  {removingEnvironmentId === environmentId ? "Removing…" : "Remove"}
                </Button>
              ) : null}
              <Button
                size="xs"
                variant="outline"
                disabled={isConnecting || removingEnvironmentId === environmentId}
                onClick={() =>
                  void (isConnected ? onRemove(environmentId) : onConnect(environmentId))
                }
              >
                {isConnected
                  ? removingEnvironmentId === environmentId
                    ? "Disconnecting…"
                    : "Disconnect"
                  : isConnecting
                    ? "Connecting…"
                    : "Connect"}
              </Button>
            </>
          }
        </div>
      </div>
    </div>
  );
}

interface DesktopSshHostRowProps {
  target: DesktopDiscoveredSshHost;
  connectingHostAlias: string | null;
  onConnect: (target: DesktopDiscoveredSshHost) => void;
}

const DesktopSshHostRow = memo(function DesktopSshHostRow({
  target,
  connectingHostAlias,
  onConnect,
}: DesktopSshHostRowProps) {
  const address = formatDesktopSshTarget(target);
  const showAddress = address !== target.alias;
  const buttonLabel = connectingHostAlias === target.alias ? "Adding…" : "Add environment";

  return (
    <div className="rounded-xl px-3 py-3 sm:px-4">
      <div className={ITEM_ROW_INNER_CLASSNAME}>
        <div className="min-w-0 flex-1">
          <h3 className="truncate text-sm font-medium text-foreground">{target.alias}</h3>
          {showAddress ? <p className="truncate text-xs text-muted-foreground">{address}</p> : null}
        </div>
        <div className="flex w-full shrink-0 items-center gap-2 sm:w-auto sm:justify-end">
          <Button
            size="xs"
            variant="outline"
            disabled={connectingHostAlias === target.alias}
            onClick={() => onConnect(target)}
          >
            {connectingHostAlias === target.alias ? (
              <RefreshCwIcon className="size-3 animate-spin" />
            ) : null}
            {buttonLabel}
          </Button>
        </div>
      </div>
    </div>
  );
});

function EmptyRemoteEnvironments() {
  return (
    <Empty className="min-h-52">
      <EmptyMedia variant="icon">
        <ChevronsLeftRightEllipsisIcon />
      </EmptyMedia>
      <EmptyHeader>
        <EmptyTitle>No saved remote environments</EmptyTitle>
        <EmptyDescription>Click “Add environment” to pair another environment.</EmptyDescription>
      </EmptyHeader>
    </Empty>
  );
}

export function ConnectionsSettings() {
  const desktopBridge = window.desktopBridge;
  const { environments } = useEnvironments();
  const primaryEnvironment = usePrimaryEnvironment();
  const connectPairing = useAtomCommand(connectPairingAtom, { reportFailure: false });
  const connectSshEnvironment = useAtomCommand(connectSshEnvironmentAtom, {
    reportFailure: false,
  });
  const removeEnvironment = useAtomCommand(environmentCatalog.remove, { reportFailure: false });
  const retryEnvironment = useAtomCommand(environmentCatalog.retryNow, { reportFailure: false });
  const primaryEnvironmentId = primaryEnvironment?.environmentId ?? null;
  const primarySessionState = usePrimarySessionState();
  const currentSessionScopes = desktopBridge
    ? AuthAdministrativeScopes
    : primarySessionState.data?.authenticated
      ? (primarySessionState.data.scopes ?? null)
      : null;
  const currentAuthPolicy = desktopBridge ? null : (primarySessionState.data?.auth.policy ?? null);
  const savedEnvironments = useMemo(
    () =>
      environments
        .filter((environment) => environment.entry.target._tag !== "PrimaryConnectionTarget")
        .toSorted((left, right) => left.label.localeCompare(right.label)),
    [environments],
  );
  const savedDesktopSshEnvironmentsByAlias = useMemo(
    () =>
      savedEnvironments.reduce<Record<string, EnvironmentPresentation>>(
        (accumulator, environment) => {
          const profile = environment.entry.profile;
          if (
            environment.entry.target._tag === "SshConnectionTarget" &&
            Option.isSome(profile) &&
            profile.value._tag === "SshConnectionProfile"
          ) {
            accumulator[profile.value.target.alias] = environment;
          }
          return accumulator;
        },
        {},
      ),
    [savedEnvironments],
  );
  const savedDesktopSshEnvironmentKeys = useMemo(() => {
    const keys = new Set<string>();
    for (const environment of savedEnvironments) {
      const profile = environment.entry.profile;
      if (
        environment.entry.target._tag !== "SshConnectionTarget" ||
        Option.isNone(profile) ||
        profile.value._tag !== "SshConnectionProfile"
      ) {
        continue;
      }
      const target = profile.value.target;
      keys.add(target.alias);
      keys.add(formatDesktopSshTarget(target));
    }
    return keys;
  }, [savedEnvironments]);
  const [sshConnectionError, setSshConnectionError] = useState<string | null>(null);
  const [connectingSshHostAlias, setConnectingSshHostAlias] = useState<string | null>(null);

  const [desktopServerExposureMutationError, setDesktopServerExposureMutationError] = useState<
    string | null
  >(null);
  const [desktopAccessManagementMutationError, setDesktopAccessManagementMutationError] = useState<
    string | null
  >(null);
  const [revokingDesktopPairingLinkId, setRevokingDesktopPairingLinkId] = useState<string | null>(
    null,
  );
  const [revokingDesktopClientSessionId, setRevokingDesktopClientSessionId] = useState<
    string | null
  >(null);
  const [isRevokingOtherDesktopClients, setIsRevokingOtherDesktopClients] = useState(false);
  const [addBackendDialogOpen, setAddBackendDialogOpen] = useState(false);
  const [savedBackendMode, setSavedBackendMode] = useState<"remote" | "ssh">("remote");
  const [savedBackendHost, setSavedBackendHost] = useState("");
  const [savedBackendPairingCode, setSavedBackendPairingCode] = useState("");
  const [savedBackendSshHost, setSavedBackendSshHost] = useState("");
  const [savedBackendSshUsername, setSavedBackendSshUsername] = useState("");
  const [savedBackendSshPort, setSavedBackendSshPort] = useState("");
  const [savedBackendError, setSavedBackendError] = useState<string | null>(null);
  const [isAddingSavedBackend, setIsAddingSavedBackend] = useState(false);
  const [removingSavedEnvironmentId, setRemovingSavedEnvironmentId] =
    useState<EnvironmentId | null>(null);
  const [isUpdatingDesktopServerExposure, setIsUpdatingDesktopServerExposure] = useState(false);
  const [isDesktopServerExposureDialogOpen, setIsDesktopServerExposureDialogOpen] = useState(false);
  const [isUpdatingTailscaleServe, setIsUpdatingTailscaleServe] = useState(false);
  const [pendingTailscaleServeEndpoint, setPendingTailscaleServeEndpoint] =
    useState<AdvertisedEndpoint | null>(null);
  const [disableTailscaleServeDialogOpen, setDisableTailscaleServeDialogOpen] = useState(false);
  const [tailscaleServePortInput, setTailscaleServePortInput] = useState(
    String(DEFAULT_TAILSCALE_SERVE_PORT),
  );
  const [pendingDesktopServerExposureMode, setPendingDesktopServerExposureMode] = useState<
    DesktopServerExposureState["mode"] | null
  >(null);
  const primaryServerConfig = primaryEnvironment?.serverConfig ?? null;
  const primaryVersionMismatch = resolveServerConfigVersionMismatch(primaryServerConfig);
  const [isAdvertisedEndpointListExpanded, setIsAdvertisedEndpointListExpanded] = useState(false);
  const [externalTunnelSnapshot, setExternalTunnelSnapshot] =
    useState<ExternalTunnelEndpointSnapshot | null>(null);
  const acceptExternalTunnelSnapshot = useCallback(
    (snapshot: ExternalTunnelEndpointSnapshot) => setExternalTunnelSnapshot(snapshot),
    [],
  );
  const defaultAdvertisedEndpointKey = useUiStateStore(
    (state) => state.defaultAdvertisedEndpointKey,
  );
  const setDefaultAdvertisedEndpointKey = useUiStateStore(
    (state) => state.setDefaultAdvertisedEndpointKey,
  );
  const canManageLocalBackend = currentSessionScopes?.includes(AuthAccessWriteScope) ?? false;
  const canReadLocalBackendAccess = currentSessionScopes?.includes(AuthAccessReadScope) ?? false;
  const authAccessChanges = useEnvironmentQuery(
    canManageLocalBackend && primaryEnvironmentId !== null
      ? authEnvironment.accessChanges({
          environmentId: primaryEnvironmentId,
          input: null,
        })
      : null,
  );
  // Two ways to answer one question. Upstream's bridge when there is one;
  // otherwise the Tauri shell, which answers the same two questions over its
  // own commands — `~/state/shellNetworkAccess` is why that is a separate seam
  // rather than a `window.desktopBridge` laplus pretends to have.
  //
  // `canManageNetworkAccess` and not `desktopBridge` from here down: every
  // control in this section is about where *this* server listens, and in laplus
  // that is the shell's to answer.
  const canManageNetworkAccess = Boolean(desktopBridge) || isDesktopShell;
  const desktopNetworkAccess = useEnvironmentQuery(
    canManageLocalBackend && canManageNetworkAccess
      ? desktopBridge
        ? desktopNetworkAccessStateAtom
        : shellNetworkAccessStateAtom
      : null,
  );
  const refreshNetworkAccess = useCallback(() => {
    if (desktopBridge) {
      refreshDesktopNetworkAccessState();
      return;
    }
    refreshShellNetworkAccessState();
  }, [desktopBridge]);
  const desktopSshHosts = useEnvironmentQuery(
    desktopBridge && addBackendDialogOpen && savedBackendMode === "ssh"
      ? desktopSshHostsStateAtom
      : null,
  );
  const discoveredSshHosts = desktopSshHosts.data ?? EMPTY_DISCOVERED_SSH_HOSTS;
  const unsavedDiscoveredSshHosts = useMemo(
    () =>
      discoveredSshHosts.filter((target) => {
        const address = formatDesktopSshTarget(target);
        return (
          !savedDesktopSshEnvironmentKeys.has(target.alias) &&
          !savedDesktopSshEnvironmentKeys.has(address)
        );
      }),
    [discoveredSshHosts, savedDesktopSshEnvironmentKeys],
  );
  const hasLoadedDiscoveredSshHosts =
    desktopSshHosts.data !== null || desktopSshHosts.error !== null;
  const isLoadingDiscoveredSshHosts = desktopSshHosts.isPending;
  const discoveredSshHostsError = sshConnectionError ?? desktopSshHosts.error;
  const desktopServerExposureState = desktopNetworkAccess.data?.serverExposureState ?? null;
  const desktopAdvertisedEndpoints = useMemo(() => {
    const endpoints = desktopNetworkAccess.data?.advertisedEndpoints ?? EMPTY_ADVERTISED_ENDPOINTS;
    return mergeVerifiedExternalEndpoint(endpoints, externalTunnelSnapshot);
  }, [desktopNetworkAccess.data?.advertisedEndpoints, externalTunnelSnapshot]);
  const desktopServerExposureError =
    desktopServerExposureMutationError ?? desktopNetworkAccess.error;
  const desktopAccessManagementError =
    desktopAccessManagementMutationError ?? authAccessChanges.error;
  const isLoadingDesktopAccessManagement =
    authAccessChanges.isPending && authAccessChanges.data === null;
  const desktopPairingLinks = useMemo(() => {
    const event = authAccessChanges.data;
    if (event?.type !== "snapshot") return [];
    return sortDesktopPairingLinks(
      event.payload.pairingLinks.map((pairingLink: AuthPairingLink) =>
        toDesktopPairingLinkRecord(pairingLink),
      ),
    );
  }, [authAccessChanges.data]);
  const desktopClientSessions = useMemo(() => {
    const event = authAccessChanges.data;
    if (event?.type !== "snapshot") return [];
    return sortDesktopClientSessions(
      event.payload.clientSessions.map((clientSession: AuthClientSession) =>
        toDesktopClientSessionRecord(clientSession),
      ),
    );
  }, [authAccessChanges.data]);
  const isLocalBackendNetworkAccessible = canManageNetworkAccess
    ? desktopServerExposureState?.mode === "network-accessible"
    : currentAuthPolicy === "remote-reachable";
  const trimmedTailscaleServePortInput = tailscaleServePortInput.trim();
  const parsedTailscaleServePort = Number(trimmedTailscaleServePortInput);
  const isTailscaleServePortValid =
    /^\d+$/u.test(trimmedTailscaleServePortInput) &&
    Number.isInteger(parsedTailscaleServePort) &&
    parsedTailscaleServePort >= 1 &&
    parsedTailscaleServePort <= 65_535;

  const pendingTailscaleServeBaseUrl = useMemo(() => {
    if (!pendingTailscaleServeEndpoint) return null;
    if (!isTailscaleServePortValid) return pendingTailscaleServeEndpoint.httpBaseUrl;
    if (parsedTailscaleServePort === DEFAULT_TAILSCALE_SERVE_PORT) {
      return pendingTailscaleServeEndpoint.httpBaseUrl;
    }
    try {
      const url = new URL(pendingTailscaleServeEndpoint.httpBaseUrl);
      url.port = String(parsedTailscaleServePort);
      return url.toString().replace(/\/$/u, "");
    } catch {
      return pendingTailscaleServeEndpoint.httpBaseUrl;
    }
  }, [isTailscaleServePortValid, parsedTailscaleServePort, pendingTailscaleServeEndpoint]);

  const handleDesktopServerExposureChange = useCallback(
    async (checked: boolean) => {
      if (!canManageNetworkAccess) return;
      setIsUpdatingDesktopServerExposure(true);
      setDesktopServerExposureMutationError(null);
      const mode = checked ? "network-accessible" : "local-only";
      try {
        // In the Tauri shell this does not come back when the mode actually
        // changes — the listener cannot be re-bound under its open sockets, so
        // the shell writes the file and relaunches, which is what the dialog
        // above already says will happen. The lines after it run only when the
        // mode asked for was the one already in force.
        if (desktopBridge) {
          await desktopBridge.setServerExposureMode(mode);
        } else {
          await setShellNetworkExposure(mode);
        }
        refreshNetworkAccess();
        setIsDesktopServerExposureDialogOpen(false);
        setIsUpdatingDesktopServerExposure(false);
      } catch (error) {
        const message =
          error instanceof Error ? error.message : "Failed to update network exposure.";
        setIsDesktopServerExposureDialogOpen(false);
        setDesktopServerExposureMutationError(message);
        toastManager.add(
          stackedThreadToast({
            type: "error",
            title: "Could not update network access",
            description: message,
          }),
        );
        setIsUpdatingDesktopServerExposure(false);
      }
    },
    [canManageNetworkAccess, desktopBridge, refreshNetworkAccess],
  );

  const handleConfirmDesktopServerExposureChange = useCallback(() => {
    if (pendingDesktopServerExposureMode === null) return;
    const checked = pendingDesktopServerExposureMode === "network-accessible";
    void handleDesktopServerExposureChange(checked);
  }, [handleDesktopServerExposureChange, pendingDesktopServerExposureMode]);

  const handleConfirmTailscaleServeSetup = useCallback(async () => {
    if (!desktopBridge) return;
    if (!isTailscaleServePortValid) return;
    setIsUpdatingTailscaleServe(true);
    setDesktopServerExposureMutationError(null);
    try {
      await desktopBridge.setTailscaleServeEnabled({
        enabled: true,
        port: parsedTailscaleServePort,
      });
      refreshDesktopNetworkAccessState();
      setPendingTailscaleServeEndpoint(null);
    } catch (error) {
      const message =
        error instanceof Error ? error.message : "Failed to configure Tailscale HTTPS.";
      setDesktopServerExposureMutationError(message);
      toastManager.add(
        stackedThreadToast({
          type: "error",
          title: "Could not set up Tailscale HTTPS",
          description: message,
        }),
      );
    } finally {
      setIsUpdatingTailscaleServe(false);
    }
  }, [desktopBridge, isTailscaleServePortValid, parsedTailscaleServePort]);

  const handleStartTailscaleServeSetup = useCallback(
    (endpoint: AdvertisedEndpoint) => {
      setTailscaleServePortInput(
        String(desktopServerExposureState?.tailscaleServePort ?? DEFAULT_TAILSCALE_SERVE_PORT),
      );
      setPendingTailscaleServeEndpoint(endpoint);
    },
    [desktopServerExposureState?.tailscaleServePort],
  );

  const handleConfirmTailscaleServeDisable = useCallback(async () => {
    if (!desktopBridge) return;
    setIsUpdatingTailscaleServe(true);
    setDesktopServerExposureMutationError(null);
    try {
      await desktopBridge.setTailscaleServeEnabled({
        enabled: false,
        port: desktopServerExposureState?.tailscaleServePort ?? DEFAULT_TAILSCALE_SERVE_PORT,
      });
      refreshDesktopNetworkAccessState();
      setDisableTailscaleServeDialogOpen(false);
    } catch (error) {
      const message = error instanceof Error ? error.message : "Failed to disable Tailscale HTTPS.";
      setDesktopServerExposureMutationError(message);
      toastManager.add(
        stackedThreadToast({
          type: "error",
          title: "Could not disable Tailscale HTTPS",
          description: message,
        }),
      );
    } finally {
      setIsUpdatingTailscaleServe(false);
    }
  }, [desktopBridge, desktopServerExposureState?.tailscaleServePort]);

  const handleStartTailscaleServeDisable = useCallback((_endpoint: AdvertisedEndpoint) => {
    setDisableTailscaleServeDialogOpen(true);
  }, []);

  const handleRevokeDesktopPairingLink = useCallback(async (id: string) => {
    setRevokingDesktopPairingLinkId(id);
    setDesktopAccessManagementMutationError(null);
    try {
      await revokeServerPairingLink(id);
    } catch (error) {
      const message = error instanceof Error ? error.message : "Failed to revoke pairing link.";
      setDesktopAccessManagementMutationError(message);
      toastManager.add(
        stackedThreadToast({
          type: "error",
          title: "Could not revoke pairing link",
          description: message,
        }),
      );
    } finally {
      setRevokingDesktopPairingLinkId(null);
    }
  }, []);

  const handleRevokeDesktopClientSession = useCallback(
    async (sessionId: ServerClientSessionRecord["sessionId"]) => {
      setRevokingDesktopClientSessionId(sessionId);
      setDesktopAccessManagementMutationError(null);
      try {
        await revokeServerClientSession(sessionId);
      } catch (error) {
        const message = error instanceof Error ? error.message : "Failed to revoke client access.";
        setDesktopAccessManagementMutationError(message);
        toastManager.add(
          stackedThreadToast({
            type: "error",
            title: "Could not revoke client access",
            description: message,
          }),
        );
      } finally {
        setRevokingDesktopClientSessionId(null);
      }
    },
    [],
  );

  const handleRevokeOtherDesktopClients = useCallback(async () => {
    setIsRevokingOtherDesktopClients(true);
    setDesktopAccessManagementMutationError(null);
    try {
      const revokedCount = await revokeOtherServerClientSessions();
      toastManager.add({
        type: "success",
        title: revokedCount === 1 ? "Revoked 1 other client" : `Revoked ${revokedCount} clients`,
        description: "Other paired clients will need a new pairing link before reconnecting.",
      });
    } catch (error) {
      const message = error instanceof Error ? error.message : "Failed to revoke other clients.";
      setDesktopAccessManagementMutationError(message);
      toastManager.add(
        stackedThreadToast({
          type: "error",
          title: "Could not revoke other clients",
          description: message,
        }),
      );
    } finally {
      setIsRevokingOtherDesktopClients(false);
    }
  }, []);

  const handleAddSavedBackend = useCallback(async () => {
    if (savedBackendMode === "ssh") {
      setIsAddingSavedBackend(true);
      setSavedBackendError(null);
      let target: DesktopSshEnvironmentTarget;
      try {
        target = parseManualDesktopSshTarget({
          host: savedBackendSshHost,
          username: savedBackendSshUsername,
          port: savedBackendSshPort,
        });
      } catch (error) {
        setSavedBackendError(formatDesktopSshConnectionError(error));
        setIsAddingSavedBackend(false);
        return;
      }

      const result = await connectSshEnvironment({ target, label: "" });
      if (result._tag === "Failure") {
        if (!isAtomCommandInterrupted(result)) {
          setSavedBackendError(formatDesktopSshConnectionError(squashAtomCommandFailure(result)));
        }
        setIsAddingSavedBackend(false);
        return;
      }

      setSavedBackendHost("");
      setSavedBackendPairingCode("");
      setSavedBackendSshHost("");
      setSavedBackendSshUsername("");
      setSavedBackendSshPort("");
      setAddBackendDialogOpen(false);
      toastManager.add({
        type: "success",
        title: "Environment connected",
        description: `${target.alias} is ready over an SSH-managed tunnel.`,
      });
      setIsAddingSavedBackend(false);
      return;
    }

    setIsAddingSavedBackend(true);
    setSavedBackendError(null);
    let remotePairingInput: ReturnType<typeof parseRemotePairingFields>;
    try {
      remotePairingInput = parseRemotePairingFields({
        host: savedBackendHost,
        pairingCode: savedBackendPairingCode,
      });
    } catch (error) {
      const message = error instanceof Error ? error.message : "Failed to add backend.";
      setSavedBackendError(message);
      toastManager.add(
        stackedThreadToast({
          type: "error",
          title: "Could not add backend",
          description: message,
        }),
      );
      setIsAddingSavedBackend(false);
      return;
    }

    const result = await connectPairing(remotePairingInput);
    if (result._tag === "Failure") {
      if (!isAtomCommandInterrupted(result)) {
        const error = squashAtomCommandFailure(result);
        const message = error instanceof Error ? error.message : "Failed to add backend.";
        setSavedBackendError(message);
        toastManager.add(
          stackedThreadToast({
            type: "error",
            title: "Could not add backend",
            description: message,
          }),
        );
      }
      setIsAddingSavedBackend(false);
      return;
    }

    setSavedBackendHost("");
    setSavedBackendPairingCode("");
    setSavedBackendSshHost("");
    setSavedBackendSshUsername("");
    setSavedBackendSshPort("");
    setAddBackendDialogOpen(false);
    toastManager.add({
      type: "success",
      title: "Backend added",
      description: "The environment is saved and will reconnect on app startup.",
    });
    setIsAddingSavedBackend(false);
  }, [
    connectPairing,
    connectSshEnvironment,
    savedBackendHost,
    savedBackendMode,
    savedBackendPairingCode,
    savedBackendSshHost,
    savedBackendSshPort,
    savedBackendSshUsername,
  ]);

  const handleConnectSavedBackend = useCallback(
    async (environmentId: EnvironmentId) => {
      setSavedBackendError(null);
      const result = await retryEnvironment(environmentId);
      if (result._tag === "Failure" && !isAtomCommandInterrupted(result)) {
        const error = squashAtomCommandFailure(result);
        const message = error instanceof Error ? error.message : "Failed to connect backend.";
        setSavedBackendError(message);
        toastManager.add(
          stackedThreadToast({
            type: "error",
            title: "Could not connect backend",
            description: message,
          }),
        );
      }
    },
    [retryEnvironment],
  );

  const handleRemoveSavedBackend = useCallback(
    async (environmentId: EnvironmentId) => {
      setRemovingSavedEnvironmentId(environmentId);
      setSavedBackendError(null);
      const result = await removeEnvironment(environmentId);
      setRemovingSavedEnvironmentId(null);
      if (result._tag === "Failure" && !isAtomCommandInterrupted(result)) {
        const error = squashAtomCommandFailure(result);
        const message = error instanceof Error ? error.message : "Failed to remove backend.";
        setSavedBackendError(message);
        toastManager.add(
          stackedThreadToast({
            type: "error",
            title: "Could not remove backend",
            description: message,
          }),
        );
      }
    },
    [removeEnvironment],
  );

  const handleConnectSshHost = useCallback(
    async (target: DesktopSshEnvironmentTarget, label?: string) => {
      setConnectingSshHostAlias(target.alias);
      if (savedBackendMode === "ssh") {
        setSavedBackendError(null);
      } else {
        setSshConnectionError(null);
      }
      const result = await connectSshEnvironment({
        target,
        ...(label === undefined ? {} : { label }),
      });
      setConnectingSshHostAlias(null);
      if (result._tag === "Success") {
        setSavedBackendSshHost("");
        setSavedBackendSshUsername("");
        setSavedBackendSshPort("");
        setAddBackendDialogOpen(false);
        toastManager.add({
          type: "success",
          title: savedDesktopSshEnvironmentsByAlias[target.alias]
            ? "Environment reconnected"
            : "Environment connected",
          description: `${label?.trim() || target.alias} is ready over an SSH-managed tunnel.`,
        });
        return;
      }
      if (!isAtomCommandInterrupted(result)) {
        const error = squashAtomCommandFailure(result);
        const message = formatDesktopSshConnectionError(error);
        if (savedBackendMode === "ssh") {
          setSavedBackendError(message);
        } else {
          setSshConnectionError(message);
        }
      }
    },
    [connectSshEnvironment, savedBackendMode, savedDesktopSshEnvironmentsByAlias],
  );

  const visibleDesktopPairingLinks = desktopPairingLinks;
  const tailscaleHttpsEndpoint = useMemo(
    () => desktopAdvertisedEndpoints.find(isTailscaleHttpsEndpoint) ?? null,
    [desktopAdvertisedEndpoints],
  );
  const visibleDesktopNetworkAdvertisedEndpoints = useMemo(
    () =>
      visibleNetworkAdvertisedEndpoints(
        desktopAdvertisedEndpoints,
        isLocalBackendNetworkAccessible,
      ),
    [desktopAdvertisedEndpoints, isLocalBackendNetworkAccessible],
  );
  const visibleDesktopAdvertisedEndpoints = useMemo(
    () =>
      tailscaleHttpsEndpoint
        ? [...visibleDesktopNetworkAdvertisedEndpoints, tailscaleHttpsEndpoint]
        : visibleDesktopNetworkAdvertisedEndpoints,
    [tailscaleHttpsEndpoint, visibleDesktopNetworkAdvertisedEndpoints],
  );
  const isLocalBackendRemotelyReachable =
    isLocalBackendNetworkAccessible ||
    tailscaleHttpsEndpoint?.status === "available" ||
    externalTunnelSnapshot?.advertisedEndpoint?.status === "available";
  const defaultDesktopNetworkAdvertisedEndpoint = useMemo(
    () =>
      selectPairingEndpoint(visibleDesktopNetworkAdvertisedEndpoints, defaultAdvertisedEndpointKey),
    [defaultAdvertisedEndpointKey, visibleDesktopNetworkAdvertisedEndpoints],
  );
  const defaultDesktopAdvertisedEndpoint = useMemo(
    () =>
      defaultDesktopNetworkAdvertisedEndpoint ??
      selectPairingEndpoint(
        tailscaleHttpsEndpoint ? [tailscaleHttpsEndpoint] : [],
        defaultAdvertisedEndpointKey,
      ),
    [defaultAdvertisedEndpointKey, defaultDesktopNetworkAdvertisedEndpoint, tailscaleHttpsEndpoint],
  );
  const defaultDesktopAdvertisedEndpointKey = defaultDesktopAdvertisedEndpoint
    ? endpointDefaultPreferenceKey(defaultDesktopAdvertisedEndpoint)
    : null;
  const handleSetDefaultAdvertisedEndpoint = useCallback(
    (endpoint: AdvertisedEndpoint) => {
      setDefaultAdvertisedEndpointKey(endpointDefaultPreferenceKey(endpoint));
    },
    [setDefaultAdvertisedEndpointKey],
  );
  const handleSavedBackendHostChange = useCallback((value: string) => {
    const parsedPairingUrl = parsePairingUrlFields(value);
    if (parsedPairingUrl) {
      setSavedBackendHost(parsedPairingUrl.host);
      setSavedBackendPairingCode(parsedPairingUrl.pairingCode);
      return;
    }
    setSavedBackendHost(value);
  }, []);

  const renderConnectionModeCard = (input: {
    readonly mode: "remote" | "ssh";
    readonly title: string;
    readonly description: string;
    readonly icon?: ReactNode;
  }) => {
    const selected = savedBackendMode === input.mode;
    return (
      <button
        type="button"
        aria-pressed={selected}
        className={cn(
          "group flex min-h-24 items-start gap-3 rounded-lg border p-4 text-left",
          selected ? "border-primary/50 bg-primary/5" : "border-border/60 hover:bg-muted/40",
        )}
        disabled={isAddingSavedBackend}
        onClick={() => {
          setSavedBackendMode(input.mode);
        }}
      >
        {input.icon ? (
          <span
            className={cn(
              "mt-0.5 flex size-8 shrink-0 items-center justify-center rounded-md border",
              selected
                ? "border-primary/30 bg-primary/10 text-primary"
                : "border-border/70 bg-background text-muted-foreground group-hover:text-foreground",
            )}
          >
            {input.icon}
          </span>
        ) : null}
        <span className="min-w-0">
          <span className="block text-sm font-medium text-foreground">{input.title}</span>
          <span className="mt-1 block text-xs leading-relaxed text-muted-foreground">
            {input.description}
          </span>
        </span>
      </button>
    );
  };

  const renderRemoteFields = () => (
    <div className="space-y-3">
      <div className="grid gap-3 sm:grid-cols-[minmax(0,1fr)_10rem]">
        <label className="block">
          <span className="mb-1.5 block text-xs font-medium text-foreground">Host</span>
          <Input
            value={savedBackendHost}
            onChange={(event) => handleSavedBackendHostChange(event.target.value)}
            placeholder="backend.example.com"
            disabled={isAddingSavedBackend}
            spellCheck={false}
          />
        </label>
        <label className="block">
          <span className="mb-1.5 block text-xs font-medium text-foreground">Pairing code</span>
          <Input
            value={savedBackendPairingCode}
            onChange={(event) => setSavedBackendPairingCode(event.target.value)}
            placeholder="PAIRCODE"
            disabled={isAddingSavedBackend}
            spellCheck={false}
          />
        </label>
      </div>
      <div>
        <span className="mt-1 block text-[11px] text-muted-foreground">
          Paste a full pairing URL here to fill both fields automatically.
        </span>
      </div>
    </div>
  );
  const renderRemoteModeBody = () => (
    <div className="space-y-4">
      {renderRemoteFields()}
      {savedBackendError ? <p className="text-xs text-destructive">{savedBackendError}</p> : null}
      <Button
        variant="outline"
        className="w-full"
        disabled={isAddingSavedBackend}
        onClick={() => void handleAddSavedBackend()}
      >
        <PlusIcon className="size-3.5" />
        {isAddingSavedBackend ? "Adding…" : "Add environment"}
      </Button>
    </div>
  );
  const renderSshFields = () => (
    <div className="space-y-4">
      <div className="space-y-3">
        <label className="block">
          <span className="mb-1.5 block text-xs font-medium text-foreground">
            SSH host or alias
          </span>
          <Input
            value={savedBackendSshHost}
            onChange={(event) => setSavedBackendSshHost(event.target.value)}
            placeholder="Search hosts or type devbox"
            disabled={isAddingSavedBackend}
            spellCheck={false}
          />
        </label>
        <div className="grid gap-3 sm:grid-cols-[minmax(0,1fr)_7rem]">
          <label className="block">
            <span className="mb-1.5 block text-xs font-medium text-foreground">Username</span>
            <Input
              value={savedBackendSshUsername}
              onChange={(event) => setSavedBackendSshUsername(event.target.value)}
              placeholder="root"
              disabled={isAddingSavedBackend}
              spellCheck={false}
            />
          </label>
          <label className="block">
            <span className="mb-1.5 block text-xs font-medium text-foreground">Port</span>
            <Input
              value={savedBackendSshPort}
              onChange={(event) => setSavedBackendSshPort(event.target.value)}
              placeholder="22"
              inputMode="numeric"
              disabled={isAddingSavedBackend}
              spellCheck={false}
            />
          </label>
        </div>
        {savedBackendError || discoveredSshHostsError ? (
          <div className="rounded-md border border-destructive/30 bg-destructive/5 px-3 py-2 text-xs text-destructive">
            {savedBackendError ?? discoveredSshHostsError}
          </div>
        ) : null}
        <Button
          variant="outline"
          className="w-full"
          disabled={isAddingSavedBackend}
          onClick={() => void handleAddSavedBackend()}
        >
          <PlusIcon className="size-3.5" />
          {isAddingSavedBackend ? "Adding…" : "Add environment"}
        </Button>
      </div>
      <div className="overflow-hidden rounded-lg border border-border/60">
        <div className="flex items-center justify-between gap-3 border-b border-border/60 bg-muted/30 px-3 py-2">
          <div className="min-w-0">
            <p className="text-xs font-medium text-foreground">Suggested hosts</p>
            <p className="text-[11px] text-muted-foreground">From SSH config and known hosts</p>
          </div>
          <Button
            size="xs"
            variant="ghost"
            disabled={isLoadingDiscoveredSshHosts}
            onClick={desktopSshHosts.refresh}
          >
            {isLoadingDiscoveredSshHosts ? (
              <RefreshCwIcon className="size-3 animate-spin" />
            ) : (
              <RefreshCwIcon className="size-3" />
            )}
            Refresh
          </Button>
        </div>
        <ScrollArea scrollFade className="max-h-56">
          <div>
            {unsavedDiscoveredSshHosts.map((target) => (
              <DesktopSshHostRow
                key={`${target.alias}:${target.hostname}:${target.port ?? ""}`}
                target={target}
                connectingHostAlias={connectingSshHostAlias}
                onConnect={(nextTarget) => void handleConnectSshHost(nextTarget)}
              />
            ))}
            {hasLoadedDiscoveredSshHosts &&
            !isLoadingDiscoveredSshHosts &&
            unsavedDiscoveredSshHosts.length === 0 ? (
              <div className={ITEM_ROW_CLASSNAME}>
                <p className="text-xs text-muted-foreground">No new SSH hosts were discovered.</p>
              </div>
            ) : null}
          </div>
        </ScrollArea>
      </div>
    </div>
  );
  const renderNetworkAccessToggle = () => (
    <Switch
      checked={desktopServerExposureState?.mode === "network-accessible"}
      disabled={!desktopServerExposureState || isUpdatingDesktopServerExposure}
      onCheckedChange={(checked) => {
        setPendingDesktopServerExposureMode(checked ? "network-accessible" : "local-only");
        setIsDesktopServerExposureDialogOpen(true);
      }}
      aria-label="Enable network access"
    />
  );
  const renderEndpointRows = (presentation: AccessSectionPresentation) =>
    isAdvertisedEndpointListExpanded
      ? visibleDesktopNetworkAdvertisedEndpoints.map((endpoint) => {
          const endpointKey = endpointDefaultPreferenceKey(endpoint);
          return (
            <AdvertisedEndpointListRow
              key={endpoint.id}
              endpoint={endpoint}
              isDefault={endpointKey === defaultDesktopAdvertisedEndpointKey}
              presentation={presentation}
              onSetDefault={handleSetDefaultAdvertisedEndpoint}
              onSetupTailscaleServe={handleStartTailscaleServeSetup}
              onDisableTailscaleServe={handleStartTailscaleServeDisable}
              isUpdatingTailscaleServe={isUpdatingTailscaleServe}
            />
          );
        })
      : null;
  const renderTailscaleRow = () => (
    <SettingsRow
      title="Tailscale HTTPS"
      description={
        tailscaleHttpsEndpoint
          ? tailscaleHttpsEndpoint.status === "available"
            ? tailscaleHttpsEndpoint.httpBaseUrl
            : "Use Tailscale Serve to expose this backend through a MagicDNS HTTPS URL."
          : "Start Tailscale to set up HTTPS access through MagicDNS."
      }
      control={
        tailscaleHttpsEndpoint ? (
          <Switch
            checked={tailscaleHttpsEndpoint.status === "available"}
            disabled={isUpdatingTailscaleServe}
            onCheckedChange={(checked) => {
              if (checked) {
                handleStartTailscaleServeSetup(tailscaleHttpsEndpoint);
                return;
              }
              handleStartTailscaleServeDisable(tailscaleHttpsEndpoint);
            }}
            aria-label="Enable Tailscale HTTPS"
          />
        ) : null
      }
    />
  );
  const renderAuthorizedClients = (presentation: AccessSectionPresentation) => (
    <>
      {desktopAccessManagementError ? (
        <div className={accessRowClassName(presentation)}>
          <p className="text-xs text-destructive">{desktopAccessManagementError}</p>
        </div>
      ) : null}
      <PairingClientsList
        endpointUrl={desktopServerExposureState?.endpointUrl}
        endpoints={visibleDesktopAdvertisedEndpoints}
        defaultEndpointKey={defaultDesktopAdvertisedEndpointKey}
        presentation={presentation}
        isLoading={isLoadingDesktopAccessManagement}
        pairingLinks={visibleDesktopPairingLinks}
        clientSessions={desktopClientSessions}
        revokingPairingLinkId={revokingDesktopPairingLinkId}
        revokingClientSessionId={revokingDesktopClientSessionId}
        onRevokePairingLink={handleRevokeDesktopPairingLink}
        onRevokeClientSession={handleRevokeDesktopClientSession}
      />
    </>
  );
  const renderNetworkAccessRow = () => (
    <SettingsRow
      title="Network access"
      description={
        isLocalBackendNetworkAccessible ? (
          <NetworkAccessDescription
            endpoint={defaultDesktopNetworkAdvertisedEndpoint}
            hiddenEndpointCount={Math.max(visibleDesktopNetworkAdvertisedEndpoints.length - 1, 0)}
            expanded={isAdvertisedEndpointListExpanded}
            onToggleExpanded={() => setIsAdvertisedEndpointListExpanded((expanded) => !expanded)}
            fallback={
              desktopServerExposureState?.endpointUrl
                ? `Reachable at ${desktopServerExposureState.endpointUrl}`
                : desktopServerExposureState?.advertisedHost
                  ? `Exposed on all interfaces. Pairing links use ${desktopServerExposureState.advertisedHost}.`
                  : "Exposed on all interfaces."
            }
          />
        ) : desktopServerExposureState ? (
          "Limited to this machine."
        ) : (
          "Loading…"
        )
      }
      status={
        desktopServerExposureError ? (
          <span className="block text-destructive">{desktopServerExposureError}</span>
        ) : null
      }
      control={renderNetworkAccessToggle()}
    />
  );
  const renderDisabledNetworkAccessRow = () => (
    <SettingsRow
      title="Network access"
      description={
        currentAuthPolicy === "remote-reachable"
          ? "This backend is already configured for remote access. Network exposure changes must be made where the server is launched."
          : "This backend is only reachable on this machine. Restart it with a non-loopback host to enable remote pairing."
      }
      control={
        <Tooltip>
          <TooltipTrigger
            render={
              <span className="inline-flex">
                <Switch
                  checked={isLocalBackendNetworkAccessible}
                  disabled
                  aria-label="Enable network access"
                />
              </span>
            }
          />
          <TooltipPopup side="top">
            Network exposure changes restart the backend and must be controlled where the server
            process is launched.
          </TooltipPopup>
        </Tooltip>
      }
    />
  );

  return (
    <SettingsPageContainer>
      {canManageLocalBackend ? (
        <>
          <SettingsSection title="This environment">
            {primaryVersionMismatch ? (
              <SettingsRow
                title="Version drift"
                description={
                  <span className="flex items-center gap-1 text-warning">
                    <TriangleAlertIcon className="size-3.5 shrink-0" />
                    Client {primaryVersionMismatch.clientVersion}, server{" "}
                    {primaryVersionMismatch.serverVersion}. Sync them if RPC calls or reconnects
                    fail.
                  </span>
                }
                control={
                  primaryEnvironmentId !== null ? (
                    <ServerUpdateAction
                      environmentId={primaryEnvironmentId}
                      serverLabel={primaryEnvironment?.label ?? "this server"}
                      selfUpdate={resolveServerSelfUpdateCapability(primaryServerConfig)}
                      targetVersion={primaryVersionMismatch.clientVersion}
                    />
                  ) : undefined
                }
              />
            ) : null}
            {canManageNetworkAccess ? (
              <>
                {renderNetworkAccessRow()}
                {renderEndpointRows("endpoint-rail")}
                {/* Tailscale Serve is upstream's, driven through the bridge.
                    laplus starts no `tailscale serve`, so the row is offered
                    only where something can act on it — a tailnet name still
                    works here, through the tunnel list below. */}
                {desktopBridge ? renderTailscaleRow() : null}
              </>
            ) : (
              <>{renderDisabledNetworkAccessRow()}</>
            )}
            {/* **Outside the desktop-bridge branch, on purpose.** What gates
                Cloudflare setup is a scope, not a shell: ADR-0047 gives it to
                `access:read` and `access:write`, and ADR-0048 makes a headless
                `laplus-server` a connector's owner as much as the window is.
                Nested under `canManageNetworkAccess` it was invisible to
                exactly the deployment the feature exists for — a browser
                pointed at a server running under systemd, which is also the
                only shape `tools/ui-driver/cloudflare-tunnel.mjs` can drive. */}
            <CloudflareTunnelSettingsRow canWrite onSnapshot={acceptExternalTunnelSnapshot} />
          </SettingsSection>

          {isLocalBackendRemotelyReachable ? (
            <SettingsSection
              title="Authorized clients"
              headerAction={
                <AuthorizedClientsHeaderAction
                  clientSessions={desktopClientSessions}
                  isRevokingOtherClients={isRevokingOtherDesktopClients}
                  onRevokeOtherClients={handleRevokeOtherDesktopClients}
                />
              }
            >
              <ScrollArea
                scrollFade
                className="max-h-[22.5rem]"
                data-testid="authorized-clients-scroll-area"
              >
                {renderAuthorizedClients("current")}
              </ScrollArea>
            </SettingsSection>
          ) : null}
          <AlertDialog
            open={isDesktopServerExposureDialogOpen}
            onOpenChange={(open) => {
              if (isUpdatingDesktopServerExposure) return;
              setIsDesktopServerExposureDialogOpen(open);
            }}
            onOpenChangeComplete={(open) => {
              if (!open) setPendingDesktopServerExposureMode(null);
            }}
          >
            <AlertDialogPopup>
              <AlertDialogHeader>
                <AlertDialogTitle>
                  {pendingDesktopServerExposureMode === "network-accessible"
                    ? "Enable network access?"
                    : "Disable network access?"}
                </AlertDialogTitle>
                <AlertDialogDescription>
                  {pendingDesktopServerExposureMode === "network-accessible"
                    ? "laplus will restart to expose this environment over the network."
                    : "laplus will restart and limit this environment back to this machine."}
                </AlertDialogDescription>
              </AlertDialogHeader>
              <AlertDialogFooter>
                <AlertDialogClose
                  disabled={isUpdatingDesktopServerExposure}
                  render={<Button variant="outline" disabled={isUpdatingDesktopServerExposure} />}
                >
                  Cancel
                </AlertDialogClose>
                <Button
                  variant={
                    pendingDesktopServerExposureMode === "local-only" ? "destructive" : "default"
                  }
                  onClick={handleConfirmDesktopServerExposureChange}
                  disabled={
                    pendingDesktopServerExposureMode === null || isUpdatingDesktopServerExposure
                  }
                >
                  {isUpdatingDesktopServerExposure ? (
                    <>
                      <Spinner className="size-3.5" />
                      Restarting…
                    </>
                  ) : pendingDesktopServerExposureMode === "network-accessible" ? (
                    "Restart and enable"
                  ) : (
                    "Restart and disable"
                  )}
                </Button>
              </AlertDialogFooter>
            </AlertDialogPopup>
          </AlertDialog>
          <AlertDialog
            open={disableTailscaleServeDialogOpen}
            onOpenChange={(open) => {
              if (isUpdatingTailscaleServe) return;
              setDisableTailscaleServeDialogOpen(open);
            }}
          >
            <AlertDialogPopup>
              <AlertDialogHeader>
                <AlertDialogTitle>Disable Tailscale HTTPS?</AlertDialogTitle>
                <AlertDialogDescription>
                  laplus will restart the local backend without Tailscale Serve.
                </AlertDialogDescription>
              </AlertDialogHeader>
              <AlertDialogFooter>
                <AlertDialogClose
                  disabled={isUpdatingTailscaleServe}
                  render={<Button variant="outline" disabled={isUpdatingTailscaleServe} />}
                >
                  Cancel
                </AlertDialogClose>
                <Button
                  variant="destructive"
                  onClick={() => void handleConfirmTailscaleServeDisable()}
                  disabled={isUpdatingTailscaleServe}
                >
                  {isUpdatingTailscaleServe ? (
                    <>
                      <Spinner className="size-3.5" />
                      Restarting…
                    </>
                  ) : (
                    "Restart and disable"
                  )}
                </Button>
              </AlertDialogFooter>
            </AlertDialogPopup>
          </AlertDialog>
          <Dialog
            open={pendingTailscaleServeEndpoint !== null}
            onOpenChange={(open) => {
              if (isUpdatingTailscaleServe) return;
              if (!open) setPendingTailscaleServeEndpoint(null);
            }}
          >
            <DialogPopup className="max-w-md">
              <DialogHeader>
                <DialogTitle>Set up Tailscale HTTPS?</DialogTitle>
                <DialogDescription>
                  laplus will restart the local backend with Tailscale Serve enabled and ask
                  Tailscale to proxy HTTPS traffic to this backend.
                </DialogDescription>
              </DialogHeader>
              <DialogPanel className="space-y-4">
                <label className="block">
                  <span className="text-sm font-medium text-foreground">HTTPS port</span>
                  <Input
                    className="mt-2"
                    type="number"
                    inputMode="numeric"
                    min={1}
                    max={65_535}
                    step={1}
                    value={tailscaleServePortInput}
                    onChange={(event) => setTailscaleServePortInput(event.target.value)}
                    disabled={isUpdatingTailscaleServe}
                  />
                </label>
                {!isTailscaleServePortValid ? (
                  <p className="mt-2 text-xs text-destructive">Enter a port from 1 to 65535.</p>
                ) : null}
                <div className="rounded-md border border-border/70 bg-muted/20 px-3 py-2">
                  <p className="text-xs font-medium text-muted-foreground">HTTPS endpoint</p>
                  <p
                    className="mt-1 truncate text-sm text-foreground"
                    title={pendingTailscaleServeBaseUrl ?? undefined}
                  >
                    {pendingTailscaleServeBaseUrl ?? "Pending MagicDNS endpoint"}
                  </p>
                </div>
              </DialogPanel>
              <DialogFooter>
                <DialogClose
                  disabled={isUpdatingTailscaleServe}
                  render={<Button variant="outline" disabled={isUpdatingTailscaleServe} />}
                >
                  Cancel
                </DialogClose>
                <Button
                  onClick={() => void handleConfirmTailscaleServeSetup()}
                  disabled={isUpdatingTailscaleServe || !isTailscaleServePortValid}
                >
                  {isUpdatingTailscaleServe ? (
                    <>
                      <Spinner className="size-3.5" />
                      Restarting…
                    </>
                  ) : (
                    "Enable"
                  )}
                </Button>
              </DialogFooter>
            </DialogPopup>
          </Dialog>
        </>
      ) : (
        <SettingsSection title="This environment">
          <SettingsRow
            title="Administrative access"
            description="Pairing links and client-session management require the access:write scope for this backend."
          />
          {canReadLocalBackendAccess ? (
            <CloudflareTunnelSettingsRow
              canWrite={canManageLocalBackend}
              onSnapshot={acceptExternalTunnelSnapshot}
            />
          ) : null}
        </SettingsSection>
      )}

      <SettingsSection
        title="Remote environments"
        headerAction={
          <Dialog
            open={addBackendDialogOpen}
            onOpenChange={(open) => {
              setAddBackendDialogOpen(open);
              if (!open) {
                setSavedBackendError(null);
              }
            }}
          >
            <Tooltip>
              <TooltipTrigger
                render={
                  <DialogTrigger
                    render={
                      <Button
                        size="xs"
                        variant="ghost"
                        className="h-5 gap-1 rounded-sm px-1 text-[11px] font-normal text-muted-foreground/60 hover:text-muted-foreground"
                        aria-label="Add environment"
                      >
                        <PlusIcon className="size-3" />
                        <span>Add environment</span>
                      </Button>
                    }
                  />
                }
              />
              <TooltipPopup side="top">Add environment</TooltipPopup>
            </Tooltip>
            <DialogPopup className="max-h-[80dvh] sm:max-w-3xl">
              <DialogHeader>
                <DialogTitle>Add Environment</DialogTitle>
                <DialogDescription>Pair another environment to this client.</DialogDescription>
              </DialogHeader>
              <DialogPanel>
                <div className="space-y-4">
                  <div className="grid gap-3 sm:grid-cols-2">
                    {renderConnectionModeCard({
                      mode: "remote",
                      title: "Remote link",
                      description: "Enter a backend host and pairing code.",
                      icon: <ChevronsLeftRightEllipsisIcon aria-hidden className="size-4" />,
                    })}
                    {desktopBridge
                      ? renderConnectionModeCard({
                          mode: "ssh",
                          title: "SSH",
                          description: "Use local SSH config, agent, and tunnels for the backend.",
                          icon: <TerminalIcon aria-hidden className="size-4" />,
                        })
                      : null}
                  </div>
                  <AnimatedHeight>
                    {savedBackendMode === "ssh" ? renderSshFields() : renderRemoteModeBody()}
                  </AnimatedHeight>
                </div>
              </DialogPanel>
            </DialogPopup>
          </Dialog>
        }
      >
        {savedEnvironments.map((environment) => (
          <SavedBackendListRow
            key={environment.environmentId}
            environment={environment}
            removingEnvironmentId={removingSavedEnvironmentId}
            onConnect={handleConnectSavedBackend}
            onRemove={handleRemoveSavedBackend}
          />
        ))}
        {savedEnvironments.length === 0 ? <EmptyRemoteEnvironments /> : null}
      </SettingsSection>
    </SettingsPageContainer>
  );
}
