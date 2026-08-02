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
