import * as Schema from "effect/Schema";

import { TrimmedNonEmptyString } from "./baseSchemas.ts";

export const AdvertisedEndpointProviderKind = Schema.Literals([
  "core",
  "private-network",
  "tunnel",
  "manual",
]);
export type AdvertisedEndpointProviderKind = typeof AdvertisedEndpointProviderKind.Type;

export const AdvertisedEndpointReachability = Schema.Literals([
  "loopback",
  "lan",
  "private-network",
  "public",
]);
export type AdvertisedEndpointReachability = typeof AdvertisedEndpointReachability.Type;

export const AdvertisedEndpointHostedHttpsCompatibility = Schema.Literals([
  "compatible",
  "mixed-content-blocked",
  "requires-configuration",
  "unknown",
]);
export type AdvertisedEndpointHostedHttpsCompatibility =
  typeof AdvertisedEndpointHostedHttpsCompatibility.Type;

export const AdvertisedEndpointStatus = Schema.Literals(["available", "unavailable", "unknown"]);
export type AdvertisedEndpointStatus = typeof AdvertisedEndpointStatus.Type;

export const AdvertisedEndpointSource = Schema.Literals([
  "desktop-core",
  "desktop-addon",
  "server",
  "user",
]);
export type AdvertisedEndpointSource = typeof AdvertisedEndpointSource.Type;

export const AdvertisedEndpointProvider = Schema.Struct({
  id: TrimmedNonEmptyString,
  label: TrimmedNonEmptyString,
  kind: AdvertisedEndpointProviderKind,
  isAddon: Schema.Boolean,
});
export type AdvertisedEndpointProvider = typeof AdvertisedEndpointProvider.Type;

export const AdvertisedEndpointCompatibility = Schema.Struct({
  hostedHttpsApp: AdvertisedEndpointHostedHttpsCompatibility,
  desktopApp: Schema.Literals(["compatible", "unknown"]),
});
export type AdvertisedEndpointCompatibility = typeof AdvertisedEndpointCompatibility.Type;

export const AdvertisedEndpoint = Schema.Struct({
  id: TrimmedNonEmptyString,
  label: TrimmedNonEmptyString,
  provider: AdvertisedEndpointProvider,
  httpBaseUrl: TrimmedNonEmptyString,
  wsBaseUrl: TrimmedNonEmptyString,
  reachability: AdvertisedEndpointReachability,
  compatibility: AdvertisedEndpointCompatibility,
  source: AdvertisedEndpointSource,
  status: AdvertisedEndpointStatus,
  isDefault: Schema.optional(Schema.Boolean),
  description: Schema.optional(TrimmedNonEmptyString),
});
export type AdvertisedEndpoint = typeof AdvertisedEndpoint.Type;

/**
 * Who owns the Cloudflare tunnel behind an endpoint.
 *
 * **Not the same question as who runs the connector**, and the two used to be
 * one hardcoded string each. `CONTEXT.md`'s "Remote access" section is the
 * vocabulary; `server/docs/adr/0049` is the decision to persist this rather
 * than emit it as a literal, and why it is the axis ticket 07's whole
 * stop/forget/delete matrix is indexed by.
 *
 * - `external` — somebody else's tunnel. laplus verifies and advertises a
 *   hostname and touches nothing. Also the honest answer for a connector-token
 *   tunnel whose connector laplus runs: Cloudflare still owns its configuration
 *   and allocation, so laplus may not delete it.
 * - `adopted` — an inactive existing tunnel explicitly dedicated to this
 *   environment. laplus configures and supervises it; the Cloudflare allocation
 *   and DNS route stay someone else's, so deletion is never offered. Ticket 05.
 * - `laplus-created` — laplus made the allocation and the DNS route, and is the
 *   only owner that may delete either. Ticket 06.
 */
export const TunnelOwnership = Schema.Literals(["external", "adopted", "laplus-created"]);
export type TunnelOwnership = typeof TunnelOwnership.Type;

export const ExternalTunnelVerificationState = Schema.Literals([
  "unconfigured",
  "pending",
  "verified",
  "failed",
]);
export type ExternalTunnelVerificationState = typeof ExternalTunnelVerificationState.Type;

export const ExternalTunnelFailureKind = Schema.Literals([
  "dns",
  "tls",
  "destination",
  "http",
  "identity",
  "wrong-environment",
  "authentication",
  "websocket",
  "cloudflare-access",
  "cloudflare-access-websocket",
]);
export type ExternalTunnelFailureKind = typeof ExternalTunnelFailureKind.Type;

export const ExternalTunnelEndpointSnapshot = Schema.Struct({
  configured: Schema.Boolean,
  httpsOrigin: Schema.NullOr(TrimmedNonEmptyString),
  wssOrigin: Schema.NullOr(TrimmedNonEmptyString),
  ownership: TunnelOwnership,
  health: Schema.Struct({
    /**
     * Who runs the connector in front of this endpoint. Widened from the
     * literal `"external"` with {@link TunnelOwnership}: an endpoint laplus
     * supervises reported `external` here even while laplus was supervising it.
     */
    connector: Schema.Literals(["external", "laplus"]),
    https: Schema.Literals(["unknown", "healthy", "failed"]),
    webSocket: Schema.Literals(["unknown", "healthy", "failed"]),
  }),
  verificationState: ExternalTunnelVerificationState,
  failureKind: Schema.NullOr(ExternalTunnelFailureKind),
  failureMessage: Schema.NullOr(TrimmedNonEmptyString),
  lastAttemptAt: Schema.NullOr(Schema.String),
  lastVerifiedAt: Schema.NullOr(Schema.String),
  advertisedEndpoint: Schema.NullOr(AdvertisedEndpoint),
});
export type ExternalTunnelEndpointSnapshot = typeof ExternalTunnelEndpointSnapshot.Type;

export const RegisterExternalTunnelEndpointInput = Schema.Struct({
  hostname: TrimmedNonEmptyString,
});
export type RegisterExternalTunnelEndpointInput = typeof RegisterExternalTunnelEndpointInput.Type;

export const CloudflaredExecutableCompatibility = Schema.Literals(["compatible", "incompatible"]);
export type CloudflaredExecutableCompatibility = typeof CloudflaredExecutableCompatibility.Type;

export const CloudflaredExecutable = Schema.Struct({
  path: TrimmedNonEmptyString,
  source: Schema.optional(Schema.Literals(["system", "user-selected", "app-managed"])),
  version: Schema.optional(Schema.NullOr(TrimmedNonEmptyString)),
  compatibility: Schema.optional(CloudflaredExecutableCompatibility),
  selected: Schema.Boolean,
  failureMessage: Schema.optional(Schema.NullOr(TrimmedNonEmptyString)),
});
export type CloudflaredExecutable = typeof CloudflaredExecutable.Type;

export const CloudflaredExecutableDiscovery = Schema.Struct({
  executables: Schema.Array(CloudflaredExecutable),
});
export type CloudflaredExecutableDiscovery = typeof CloudflaredExecutableDiscovery.Type;

export const ManagedCloudflareConnectorState = Schema.Literals([
  "unconfigured",
  "starting",
  "ready",
  "degraded",
  "restart-exhausted",
  "stopping",
  "stopped",
  "failed",
]);
export type ManagedCloudflareConnectorState = typeof ManagedCloudflareConnectorState.Type;

export const ManagedCloudflareConnectorSnapshot = Schema.Struct({
  configured: Schema.Boolean,
  /**
   * Who runs the connector process. Always laplus here — an externally managed
   * connector is never represented by this snapshot at all.
   */
  ownership: Schema.Literal("laplus"),
  /**
   * Who owns the tunnel this connector serves. **Persisted, and read back**;
   * it was the string literal `"cloudflare"` until the ownership model landed,
   * which made every laplus-managed connector look alike whatever it was
   * running. Ticket 06's compact row reads this to say "laplus-created".
   */
  tunnelOwnership: TunnelOwnership,
  desiredState: Schema.Literals(["running", "stopped"]),
  connectorState: ManagedCloudflareConnectorState,
  readiness: Schema.NullOr(Schema.Boolean),
  httpsOrigin: Schema.NullOr(TrimmedNonEmptyString),
  loopbackOrigin: Schema.optional(TrimmedNonEmptyString),
  executablePath: Schema.NullOr(TrimmedNonEmptyString),
  detectedVersion: Schema.NullOr(TrimmedNonEmptyString),
  metricsOrigin: Schema.NullOr(TrimmedNonEmptyString),
  failureMessage: Schema.NullOr(TrimmedNonEmptyString),
  restartCount: Schema.Number,
  logs: Schema.Array(TrimmedNonEmptyString),
  verificationState: ExternalTunnelVerificationState,
  failureKind: Schema.NullOr(ExternalTunnelFailureKind),
  publicFailureMessage: Schema.NullOr(TrimmedNonEmptyString),
  lastVerifiedAt: Schema.NullOr(Schema.String),
});
export type ManagedCloudflareConnectorSnapshot = typeof ManagedCloudflareConnectorSnapshot.Type;

/** An app-managed installation is one of four states, and never "probably". */
export const CloudflaredInstallationState = Schema.Literals([
  "not-installed",
  "installing",
  "installed",
  "failed",
]);
export type CloudflaredInstallationState = typeof CloudflaredInstallationState.Type;

/** The identified release a developer approves before anything is downloaded. */
export const CloudflaredRelease = Schema.Struct({
  version: TrimmedNonEmptyString,
  assetName: TrimmedNonEmptyString,
  downloadUrl: TrimmedNonEmptyString,
  checksum: TrimmedNonEmptyString,
});
export type CloudflaredRelease = typeof CloudflaredRelease.Type;

export const CloudflaredInstallationSnapshot = Schema.Struct({
  supported: Schema.Boolean,
  platform: TrimmedNonEmptyString,
  architecture: TrimmedNonEmptyString,
  assetName: Schema.NullOr(TrimmedNonEmptyString),
  ownership: Schema.Literal("app-managed"),
  unsupportedMessage: Schema.NullOr(TrimmedNonEmptyString),
  state: CloudflaredInstallationState,
  installedPath: Schema.NullOr(TrimmedNonEmptyString),
  /** The release laplus fetched and verified. */
  installedVersion: Schema.NullOr(TrimmedNonEmptyString),
  /** What that executable reports now — cloudflared may have replaced itself. */
  detectedVersion: Schema.NullOr(TrimmedNonEmptyString),
  installedAt: Schema.NullOr(Schema.String),
  failureMessage: Schema.NullOr(TrimmedNonEmptyString),
  release: Schema.NullOr(CloudflaredRelease),
  releaseFailureMessage: Schema.NullOr(TrimmedNonEmptyString),
});
export type CloudflaredInstallationSnapshot = typeof CloudflaredInstallationSnapshot.Type;

/**
 * Approval names the exact release. The server re-reads the feed and refuses
 * when it has moved on, so this is consent to one artifact rather than to
 * "whatever is latest at the moment the button is pressed".
 */
export const ApproveCloudflaredReleaseInput = Schema.Struct({
  version: TrimmedNonEmptyString,
  checksum: TrimmedNonEmptyString,
});
export type ApproveCloudflaredReleaseInput = typeof ApproveCloudflaredReleaseInput.Type;

export const ConfigureManagedCloudflareConnectorInput = Schema.Struct({
  hostname: TrimmedNonEmptyString,
  executablePath: TrimmedNonEmptyString,
  connectorToken: TrimmedNonEmptyString,
});
export type ConfigureManagedCloudflareConnectorInput =
  typeof ConfigureManagedCloudflareConnectorInput.Type;

/**
 * Cloudflare browser authorization, as the server can observe it.
 *
 * A sign-in belongs to one running server: `not-started` is also what a restart
 * reports, and the certificate on disk — not this field — is what says the
 * authorization succeeded. The server folds that in before answering, so
 * `complete` after a restart means "a certificate is there", which is what
 * makes an interrupted wizard resumable rather than stuck.
 */
export const CloudflareAccountLoginState = Schema.Literals([
  "not-started",
  "awaiting-browser",
  "complete",
  "cancelled",
  "timed-out",
  "failed",
]);
export type CloudflareAccountLoginState = typeof CloudflareAccountLoginState.Type;

/** Whether cloudflared reported any connection for this tunnel. */
export const CloudflareTunnelActivity = Schema.Literals(["active", "inactive"]);
export type CloudflareTunnelActivity = typeof CloudflareTunnelActivity.Type;

/**
 * What laplus may do with a listed tunnel, and nothing more.
 *
 * `external` — someone else's connector is already serving it, so laplus may
 * verify and advertise the hostname and must not touch the tunnel's lifecycle.
 * `adoptable` — it is inactive, so it *may* be dedicated to laplus, but only
 * after the separate confirmation ADR-0045 requires. Neither value is a claim
 * about who owns the Cloudflare allocation.
 */
export const CloudflareTunnelClassification = Schema.Literals(["external", "adoptable"]);
export type CloudflareTunnelClassification = typeof CloudflareTunnelClassification.Type;

/**
 * One row of `cloudflared tunnel list --output json`, reduced to what it proves.
 *
 * **There is deliberately no hostname and no management mode here.** The
 * listing carries neither, so inferring either would be the wizard inventing
 * the one fact it exists to ask for.
 */
export const CloudflareAccountTunnel = Schema.Struct({
  id: TrimmedNonEmptyString,
  name: TrimmedNonEmptyString,
  createdAt: Schema.NullOr(Schema.String),
  connectionCount: Schema.Number,
  activity: CloudflareTunnelActivity,
  classification: CloudflareTunnelClassification,
});
export type CloudflareAccountTunnel = typeof CloudflareAccountTunnel.Type;

/**
 * The tunnel the developer chose and the hostname they said reaches it.
 *
 * `adoptionConfirmed` is the whole of ADR-0045's inactive-tunnel rule: a chosen
 * adoptable tunnel is a candidate, and stays one until dedication is separately
 * confirmed. Nothing here makes laplus the tunnel's lifecycle owner.
 */
export const CloudflareTunnelSelection = Schema.Struct({
  tunnelId: TrimmedNonEmptyString,
  name: TrimmedNonEmptyString,
  classification: CloudflareTunnelClassification,
  httpsOrigin: TrimmedNonEmptyString,
  adoptionConfirmed: Schema.Boolean,
});
export type CloudflareTunnelSelection = typeof CloudflareTunnelSelection.Type;

/**
 * Which step of the account wizard an interrupted setup resumes at.
 *
 * **Computed by the server from what is durably true** — a certificate on disk,
 * a recorded consent, a recorded selection — rather than remembered by the
 * browser. That is what lets a reopened dialog, a reloaded page and a restarted
 * server agree about how far setup got.
 */
export const CloudflareAccountSetupStep = Schema.Literals([
  "sign-in",
  "consent",
  "choose-tunnel",
  "verify-hostname",
  "confirm-adoption",
]);
export type CloudflareAccountSetupStep = typeof CloudflareAccountSetupStep.Type;

/**
 * Everything the wizard knows about Cloudflare account authorization.
 *
 * **`certificatePath` crosses the wire on purpose.** It is a path and never
 * contents: `cert.pem` can create, list, route and delete every tunnel in the
 * account and stays valid for years, and laplus reads only where cloudflared
 * put it (ADR-0045). It is here because consent has to name the file it is
 * consent to use — a warning about "the account certificate" with no path is
 * consent to an abstraction, and a developer with two Cloudflare accounts on
 * one machine cannot check which one they are about to hand over. Reading this
 * snapshot already requires `access:read`, which ADR-0047 reserves for
 * administrative sessions; an ordinary paired phone never sees it. The value is
 * present even when `certificateDetected` is false, because it is then the
 * location a sign-in would write to.
 */
export const CloudflareAccountSnapshot = Schema.Struct({
  certificateDetected: Schema.Boolean,
  certificatePath: TrimmedNonEmptyString,
  certificateConsentedAt: Schema.NullOr(Schema.String),
  certificateWarning: TrimmedNonEmptyString,
  loginState: CloudflareAccountLoginState,
  authorizationUrl: Schema.NullOr(TrimmedNonEmptyString),
  failureMessage: Schema.NullOr(TrimmedNonEmptyString),
  tunnels: Schema.Array(CloudflareAccountTunnel),
  listedAt: Schema.NullOr(Schema.String),
  selection: Schema.NullOr(CloudflareTunnelSelection),
  step: CloudflareAccountSetupStep,
});
export type CloudflareAccountSnapshot = typeof CloudflareAccountSnapshot.Type;

/**
 * Which cloudflared to run for an account-management action.
 *
 * Named per request rather than remembered by the account module: which
 * executable this environment uses is the wizard's earlier answer, and a server
 * that re-guessed it could sign in with one copy and list with another.
 */
export const CloudflareAccountCommandInput = Schema.Struct({
  executablePath: TrimmedNonEmptyString,
});
export type CloudflareAccountCommandInput = typeof CloudflareAccountCommandInput.Type;

/** Consent to use — or withdrawal of consent to use — the account certificate. */
export const CloudflareCertificateConsentInput = Schema.Struct({
  consented: Schema.Boolean,
});
export type CloudflareCertificateConsentInput = typeof CloudflareCertificateConsentInput.Type;

/**
 * The hostname is the developer's answer, never anything read out of the
 * listing — see {@link CloudflareAccountTunnel}.
 */
export const SelectCloudflareTunnelInput = Schema.Struct({
  tunnelId: TrimmedNonEmptyString,
  hostname: TrimmedNonEmptyString,
});
export type SelectCloudflareTunnelInput = typeof SelectCloudflareTunnelInput.Type;

/**
 * The body of the one-time HTTP challenge laplus answers to itself through the
 * public hostname. Never called by a client: the caller is this server's own
 * verifier, carrying a single-use diagnostic credential rather than a session.
 */
export const ExternalTunnelChallengeResult = Schema.Struct({
  ok: Schema.Literal(true),
});
export type ExternalTunnelChallengeResult = typeof ExternalTunnelChallengeResult.Type;
