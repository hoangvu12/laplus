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
  ownership: Schema.Literal("external"),
  health: Schema.Struct({
    connector: Schema.Literal("external"),
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
  ownership: Schema.Literal("laplus"),
  remoteOwnership: Schema.optional(Schema.Literal("cloudflare")),
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
