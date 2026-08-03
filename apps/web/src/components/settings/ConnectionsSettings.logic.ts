import type {
  AdvertisedEndpoint,
  CloudflareAccountSnapshot,
  CloudflaredExecutable,
  ExternalTunnelEndpointSnapshot,
  ManagedCloudflareConnectorSnapshot,
} from "@t3tools/contracts";

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

/**
 * Which of the three ways to reach this environment the developer is setting up.
 *
 * - `account` — sign in to Cloudflare, list the account's tunnels, choose one.
 * - `connector-token` — paste a tunnel connector token and let laplus run it.
 *   The least-privilege route: the token runs one tunnel and manages no account.
 * - `external` — a hostname somebody else's connector already serves. laplus
 *   verifies and advertises it and touches nothing (ADR-0045).
 */
export type CloudflareWizardPath = "account" | "connector-token" | "external";

/**
 * One screen of the Cloudflare wizard.
 *
 * Five of these — `sign-in`, `consent`, `choose-tunnel`, `verify-hostname` and
 * `confirm-adoption` — are named by the *server*: `cloudflare_account.rs`
 * computes them from what is durably true and answers them in every account
 * snapshot. They are repeated here as a type rather than re-derived, so that a
 * step the server adds is a compile error here rather than a silently missing
 * screen.
 */
export type CloudflareWizardStep =
  | "choose-path"
  | "sign-in"
  | "consent"
  | "choose-tunnel"
  | "verify-hostname"
  | "confirm-adoption"
  | "connector-token"
  | "external-endpoint"
  | "managed-connector";

export interface CloudflareWizardState {
  readonly step: CloudflareWizardStep;
  readonly path: CloudflareWizardPath | null;
  readonly label: string;
  /**
   * Where this step sits in its path, for the wizard's own header. `null` on
   * the path choice and on the two terminal screens, which are a place rather
   * than a position.
   */
  readonly position: { readonly index: number; readonly total: number } | null;
  /**
   * Whether the external-endpoint registration control may render.
   *
   * **The whole of ADR-0045's ownership rule, in one boolean.** A laplus-managed
   * connector owns its hostname, so registering that hostname as an *external*
   * endpoint would claim two owners for one lifecycle. Registration therefore
   * belongs to exactly one step, and never to a configured managed connector.
   */
  readonly offersExternalRegistration: boolean;
  /** Whether laplus supervises the connector behind this endpoint. */
  readonly ownsConnector: boolean;
  /** Whether the developer may go back and set up a different way. */
  readonly canChangePath: boolean;
}

const WIZARD_STEP_LABELS: Record<CloudflareWizardStep, string> = {
  "choose-path": "Choose how to connect",
  "sign-in": "Sign in to Cloudflare",
  consent: "Confirm certificate use",
  "choose-tunnel": "Choose a tunnel",
  "verify-hostname": "Verify the hostname",
  "confirm-adoption": "Dedicate the tunnel",
  "connector-token": "Connector token",
  "external-endpoint": "External hostname",
  "managed-connector": "Laplus-managed connector",
};

export const cloudflareWizardStepLabel = (step: CloudflareWizardStep): string =>
  WIZARD_STEP_LABELS[step];

const ACCOUNT_STEPS: ReadonlyArray<CloudflareWizardStep> = [
  "sign-in",
  "consent",
  "choose-tunnel",
  "verify-hostname",
];

/**
 * Has laplus engaged the Cloudflare account path, or is a certificate merely
 * lying on this machine?
 *
 * **`loginState` cannot answer this on its own.** The server reports `complete`
 * whenever a certificate exists, precisely so a restart resumes rather than
 * restarts — which means `complete` is also what a developer who ran
 * `cloudflared tunnel login` years ago for something else reports. Consent, a
 * listing, a selection, or a sign-in that ended some other way are the four
 * things only laplus's own wizard produces.
 */
const accountEngaged = (account: CloudflareAccountSnapshot): boolean =>
  account.certificateConsentedAt !== null ||
  account.tunnels.length > 0 ||
  account.selection !== null ||
  (account.loginState !== "not-started" && account.loginState !== "complete");

/**
 * Which screen the Cloudflare wizard is on.
 *
 * **Progress is read, never remembered.** Every branch below except the path
 * choice comes from a server snapshot, so a reopened dialog, a reloaded page and
 * a restarted server agree — which is what ticket 01 and ticket 04 both mean by
 * "reopen at the truthful wizard step". `chosenPath` is the one piece of client
 * state, and it is a choice the developer has not yet committed to anything
 * rather than progress: the moment they do commit, a snapshot says so and the
 * branches above it take over.
 *
 * Precedence: a connector laplus supervises, then a path just picked, then a
 * tunnel already chosen, then an account flow under way, then a hostname
 * already registered. **Work in progress beats work already finished** — a
 * developer part-way through the account path who registered an external
 * hostname last month would otherwise reopen on the finished one, and the flow
 * they were in the middle of would silently vanish.
 */
export const cloudflareWizardState = (input: {
  readonly account: CloudflareAccountSnapshot | null;
  readonly managed: ManagedCloudflareConnectorSnapshot | null;
  readonly external: ExternalTunnelEndpointSnapshot | null;
  readonly chosenPath: CloudflareWizardPath | null;
  /**
   * The developer asked to go back to the path choice.
   *
   * **Navigation, not a path.** Clearing `chosenPath` cannot express this,
   * because the inference below would immediately re-derive the step they were
   * trying to leave — which is how the "Change setup path" control managed to
   * be inert for exactly the developers who needed it: anyone whose consent,
   * listing or registration had already been recorded.
   */
  readonly revisitingPathChoice?: boolean;
}): CloudflareWizardState => {
  const { account, managed, external, chosenPath, revisitingPathChoice } = input;
  const accountStep = account?.step ?? "sign-in";

  // A connector laplus supervises is the one thing no navigation may leave:
  // there is a process running, and pretending otherwise would offer setup for
  // an exposure that already exists.
  if (managed?.configured) return wizardState("managed-connector", "connector-token", false);
  // A path picked outranks the request to pick one, so answering the choice is
  // what leaves it — no caller has to remember to clear the flag as well.
  if (chosenPath === "connector-token") return wizardState("connector-token", "connector-token");
  if (chosenPath === "external") return wizardState("external-endpoint", "external");
  if (chosenPath === "account") return wizardState(accountStep, "account");
  if (revisitingPathChoice) return wizardState("choose-path", null, false);
  // `accountEngaged` already covers a recorded selection, which is the furthest
  // the account path gets — so there is no separate branch for one.
  if (account && accountEngaged(account)) return wizardState(accountStep, "account");
  if (external?.configured) return wizardState("external-endpoint", "external");
  return wizardState("choose-path", null);
};

const wizardState = (
  step: CloudflareWizardStep,
  path: CloudflareWizardPath | null,
  canChangePath = step !== "choose-path",
): CloudflareWizardState => ({
  step,
  path,
  label: cloudflareWizardStepLabel(step),
  position: accountPosition(step, path),
  // Registration is an act of *claiming* a hostname laplus does not run, so it
  // belongs to the one step that is about doing exactly that. `verify-hostname`
  // is the account path arriving at an already-registered endpoint — the server
  // registered it when the active tunnel was selected — so it verifies and
  // pairs, and has nothing left to register.
  offersExternalRegistration: step === "external-endpoint",
  ownsConnector: step === "managed-connector",
  canChangePath,
});

const accountPosition = (
  step: CloudflareWizardStep,
  path: CloudflareWizardPath | null,
): { readonly index: number; readonly total: number } | null => {
  if (path !== "account") return null;
  const index = ACCOUNT_STEPS.indexOf(step === "confirm-adoption" ? "verify-hostname" : step);
  return index === -1 ? null : { index: index + 1, total: ACCOUNT_STEPS.length };
};

/**
 * What the compact Connections row says under "Cloudflare Tunnel".
 *
 * An unfinished setup names the step it stopped at, because a row that says
 * only "Not configured" gives a developer who was interrupted no reason to
 * believe reopening the dialog will pick up where they left off.
 */
export const cloudflareRowSummary = (input: {
  readonly state: CloudflareWizardState;
  readonly managed: ManagedCloudflareConnectorSnapshot | null;
  readonly external: ExternalTunnelEndpointSnapshot | null;
  readonly managedStateLabel: (snapshot: ManagedCloudflareConnectorSnapshot) => string;
}): string => {
  const { state, managed, external, managedStateLabel } = input;
  if (managed?.configured) {
    return `${managed.httpsOrigin} · ${managedStateLabel(managed)}`;
  }
  if (external?.httpsOrigin) {
    const verification =
      external.verificationState === "verified" ? "Verified" : "Needs verification";
    return `${external.httpsOrigin} · ${verification}`;
  }
  if (state.step === "choose-path") {
    return "Register an externally managed HTTPS hostname.";
  }
  return `Setup in progress · ${state.label}`;
};

/**
 * The cloudflared executables a developer may pick between.
 *
 * Discovery already ranks them; this only puts a hand-typed path at the end so
 * that selecting it is possible from the same list, rather than only from the
 * text field beside it. An empty or already-listed path adds nothing.
 */
export const selectableCloudflaredExecutables = (
  executables: ReadonlyArray<CloudflaredExecutable>,
  selectedPath: string,
): ReadonlyArray<CloudflaredExecutable> => {
  const trimmed = selectedPath.trim();
  if (trimmed === "" || executables.some((executable) => executable.path === trimmed)) {
    return executables;
  }
  // `selected` is the *server's* answer about which executable it would run, and
  // a path that has only been typed here has no such answer yet.
  return [...executables, { path: trimmed, source: "user-selected", selected: false }];
};
