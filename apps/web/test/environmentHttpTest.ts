import * as NodeHttpServer from "@effect/platform-node/NodeHttpServer";
import {
  AuthSessionId,
  EnvironmentAuthenticatedAuth,
  EnvironmentAuthenticatedPrincipal,
  EnvironmentHttpApi,
  type AuthBrowserSessionRequest,
  type AuthBrowserSessionResult,
  type AuthCreatePairingCredentialInput,
  type AuthEnvironmentScope,
  type AuthPairingCredentialResult,
  type AuthSessionState,
  type ExecutionEnvironmentDescriptor,
  type EnvironmentAuthInvalidError,
  type EnvironmentInternalError,
  type EnvironmentPublicExposureRefusal,
  type EnvironmentScopeRequiredError,
  type ExternalTunnelEndpointSnapshot,
  type ApproveCloudflaredReleaseInput,
  type CloudflareAccountCommandInput,
  type CloudflareAccountSnapshot,
  type CloudflareCertificateConsentInput,
  type CloudflaredExecutableDiscovery,
  type CloudflaredInstallationSnapshot,
  type ConfigureManagedCloudflareConnectorInput,
  type ManagedCloudflareConnectorSnapshot,
  type RegisterExternalTunnelEndpointInput,
  type SelectCloudflareTunnelInput,
} from "@t3tools/contracts";
import * as DateTime from "effect/DateTime";
import type * as Context from "effect/Context";
import * as Effect from "effect/Effect";
import * as Layer from "effect/Layer";
import * as ManagedRuntime from "effect/ManagedRuntime";
import { HttpApiTest } from "effect/unstable/httpapi";
import * as HttpApiBuilder from "effect/unstable/httpapi/HttpApiBuilder";

import { PrimaryEnvironmentHttpClient } from "../src/environments/primary/httpClient";
import { __setPrimaryHttpRunnerForTests } from "../src/lib/runtime";

/**
 * What a scope-gated route may refuse with, so a scenario can drive a refusal.
 *
 * The Cloudflare account routes are the first here that need it: ADR-0047 makes
 * `403` a state the UI has to render, and a handler whose error channel is
 * `never` cannot produce one.
 */
type ScopedOperationFailure = EnvironmentScopeRequiredError | EnvironmentInternalError;

/**
 * What a public-exposure route may refuse with, on top of the scope answer.
 *
 * A `409` or `400` from `/api/access/cloudflare` is a tagged refusal carrying a
 * closed reason and the mutations a partial failure completed and left
 * outstanding — and the wizard renders all three. Cloudflare ticket 05's
 * activation race is the first scenario that needs one; before it, a handler
 * whose error channel stopped at the scope refusal could not produce the shape
 * the component branches on.
 */
type PublicExposureFailure = ScopedOperationFailure | EnvironmentPublicExposureRefusal;

type BrowserSessionHandler = (
  payload: AuthBrowserSessionRequest,
) => Effect.Effect<AuthBrowserSessionResult, EnvironmentAuthInvalidError>;

interface EnvironmentHttpTestScenario {
  readonly descriptor?: () => Effect.Effect<ExecutionEnvironmentDescriptor>;
  readonly session?: () => Effect.Effect<AuthSessionState>;
  readonly browserSession?: BrowserSessionHandler;
  readonly pairingCredential?: (
    payload: AuthCreatePairingCredentialInput,
  ) => Effect.Effect<AuthPairingCredentialResult>;
  readonly externalTunnel?: () => Effect.Effect<ExternalTunnelEndpointSnapshot>;
  readonly registerExternalTunnel?: (
    payload: RegisterExternalTunnelEndpointInput,
  ) => Effect.Effect<ExternalTunnelEndpointSnapshot>;
  readonly testExternalTunnel?: () => Effect.Effect<ExternalTunnelEndpointSnapshot>;
  readonly forgetExternalTunnel?: () => Effect.Effect<ExternalTunnelEndpointSnapshot>;
  readonly cloudflaredExecutables?: () => Effect.Effect<CloudflaredExecutableDiscovery>;
  readonly cloudflaredInstallation?: () => Effect.Effect<CloudflaredInstallationSnapshot>;
  readonly installCloudflaredRelease?: (
    payload: ApproveCloudflaredReleaseInput,
  ) => Effect.Effect<CloudflaredInstallationSnapshot>;
  readonly managedCloudflareConnector?: () => Effect.Effect<ManagedCloudflareConnectorSnapshot>;
  readonly configureManagedCloudflareConnector?: (
    payload: ConfigureManagedCloudflareConnectorInput,
  ) => Effect.Effect<ManagedCloudflareConnectorSnapshot>;
  readonly startManagedCloudflareConnector?: () => Effect.Effect<ManagedCloudflareConnectorSnapshot>;
  readonly stopManagedCloudflareConnector?: () => Effect.Effect<ManagedCloudflareConnectorSnapshot>;
  readonly retryManagedCloudflareConnector?: () => Effect.Effect<ManagedCloudflareConnectorSnapshot>;
  readonly cloudflareAccount?: () => Effect.Effect<
    CloudflareAccountSnapshot,
    ScopedOperationFailure
  >;
  readonly beginCloudflareLogin?: (
    payload: CloudflareAccountCommandInput,
  ) => Effect.Effect<CloudflareAccountSnapshot, ScopedOperationFailure>;
  readonly cancelCloudflareLogin?: () => Effect.Effect<
    CloudflareAccountSnapshot,
    ScopedOperationFailure
  >;
  readonly consentToCloudflareCertificate?: (
    payload: CloudflareCertificateConsentInput,
  ) => Effect.Effect<CloudflareAccountSnapshot, ScopedOperationFailure>;
  readonly listCloudflareTunnels?: (
    payload: CloudflareAccountCommandInput,
  ) => Effect.Effect<CloudflareAccountSnapshot, ScopedOperationFailure>;
  readonly selectCloudflareTunnel?: (
    payload: SelectCloudflareTunnelInput,
  ) => Effect.Effect<CloudflareAccountSnapshot, ScopedOperationFailure>;
  readonly adoptCloudflareTunnel?: (
    payload: CloudflareAccountCommandInput,
  ) => Effect.Effect<CloudflareAccountSnapshot, PublicExposureFailure>;
}

export interface EnvironmentHttpTestCalls {
  descriptor: number;
  session: number;
  browserSession: Array<AuthBrowserSessionRequest>;
  pairingCredential: Array<AuthCreatePairingCredentialInput>;
  externalTunnel: number;
  registerExternalTunnel: Array<RegisterExternalTunnelEndpointInput>;
  testExternalTunnel: number;
  forgetExternalTunnel: number;
  cloudflaredExecutables: number;
  cloudflaredInstallation: number;
  installCloudflaredRelease: Array<ApproveCloudflaredReleaseInput>;
  managedCloudflareConnector: number;
  configureManagedCloudflareConnector: Array<ConfigureManagedCloudflareConnectorInput>;
  startManagedCloudflareConnector: number;
  stopManagedCloudflareConnector: number;
  retryManagedCloudflareConnector: number;
  cloudflareAccount: number;
  beginCloudflareLogin: Array<CloudflareAccountCommandInput>;
  cancelCloudflareLogin: number;
  consentToCloudflareCertificate: Array<CloudflareCertificateConsentInput>;
  listCloudflareTunnels: Array<CloudflareAccountCommandInput>;
  selectCloudflareTunnel: Array<SelectCloudflareTunnelInput>;
  adoptCloudflareTunnel: Array<CloudflareAccountCommandInput>;
}

const unexpectedEndpoint = (endpoint: string) =>
  Effect.die(new Error(`Unexpected environment HTTP endpoint: ${endpoint}`));

const authenticatedAuth: Context.Service.Shape<typeof EnvironmentAuthenticatedAuth> = (
  httpEffect,
) =>
  httpEffect.pipe(
    Effect.provideService(EnvironmentAuthenticatedPrincipal, {
      sessionId: AuthSessionId.make("test-session"),
      subject: "test-client",
      method: "browser-session-cookie",
      scopes: new Set<AuthEnvironmentScope>(),
      expiresAt: DateTime.makeUnsafe("2026-05-01T12:00:00.000Z"),
    }),
  );

export async function installEnvironmentHttpTest(scenario: EnvironmentHttpTestScenario) {
  const calls: EnvironmentHttpTestCalls = {
    descriptor: 0,
    session: 0,
    browserSession: [],
    pairingCredential: [],
    externalTunnel: 0,
    registerExternalTunnel: [],
    testExternalTunnel: 0,
    forgetExternalTunnel: 0,
    cloudflaredExecutables: 0,
    cloudflaredInstallation: 0,
    installCloudflaredRelease: [],
    managedCloudflareConnector: 0,
    configureManagedCloudflareConnector: [],
    startManagedCloudflareConnector: 0,
    stopManagedCloudflareConnector: 0,
    retryManagedCloudflareConnector: 0,
    cloudflareAccount: 0,
    beginCloudflareLogin: [],
    cancelCloudflareLogin: 0,
    consentToCloudflareCertificate: [],
    listCloudflareTunnels: [],
    selectCloudflareTunnel: [],
    adoptCloudflareTunnel: [],
  };

  const client = await Effect.runPromise(
    HttpApiTest.groups(EnvironmentHttpApi, ["metadata", "auth", "access"]).pipe(
      Effect.provide([
        NodeHttpServer.layerHttpServices,
        HttpApiBuilder.group(EnvironmentHttpApi, "metadata", (handlers) =>
          handlers.handle(
            "descriptor",
            Effect.fn("test.environment.metadata.descriptor")(function* () {
              calls.descriptor += 1;
              return yield* scenario.descriptor?.() ?? unexpectedEndpoint("metadata.descriptor");
            }),
          ),
        ),
        HttpApiBuilder.group(EnvironmentHttpApi, "auth", (handlers) =>
          handlers
            .handle(
              "session",
              Effect.fn("test.environment.auth.session")(function* () {
                calls.session += 1;
                return yield* scenario.session?.() ?? unexpectedEndpoint("auth.session");
              }),
            )
            .handle(
              "browserSession",
              Effect.fn("test.environment.auth.browserSession")(function* ({ payload }) {
                calls.browserSession.push(payload);
                return yield* (
                  scenario.browserSession?.(payload) ?? unexpectedEndpoint("auth.browserSession")
                );
              }),
            )
            .handle("token", () => unexpectedEndpoint("auth.token"))
            .handle("webSocketTicket", () => unexpectedEndpoint("auth.webSocketTicket"))
            .handle(
              "pairingCredential",
              Effect.fn("test.environment.auth.pairingCredential")(function* ({ payload }) {
                calls.pairingCredential.push(payload);
                return yield* (
                  scenario.pairingCredential?.(payload) ??
                    unexpectedEndpoint("auth.pairingCredential")
                );
              }),
            )
            .handle("pairingLinks", () => unexpectedEndpoint("auth.pairingLinks"))
            .handle("revokePairingLink", () => unexpectedEndpoint("auth.revokePairingLink"))
            .handle("clients", () => unexpectedEndpoint("auth.clients"))
            .handle("revokeClient", () => unexpectedEndpoint("auth.revokeClient"))
            .handle("revokeOtherClients", () => unexpectedEndpoint("auth.revokeOtherClients")),
        ),
        HttpApiBuilder.group(EnvironmentHttpApi, "access", (handlers) =>
          handlers
            .handle("externalTunnel", () => {
              calls.externalTunnel += 1;
              return scenario.externalTunnel?.() ?? unexpectedEndpoint("access.externalTunnel");
            })
            .handle("registerExternalTunnel", ({ payload }) => {
              calls.registerExternalTunnel.push(payload);
              return (
                scenario.registerExternalTunnel?.(payload) ??
                unexpectedEndpoint("access.registerExternalTunnel")
              );
            })
            .handle("testExternalTunnel", () => {
              calls.testExternalTunnel += 1;
              return (
                scenario.testExternalTunnel?.() ?? unexpectedEndpoint("access.testExternalTunnel")
              );
            })
            .handle("forgetExternalTunnel", () => {
              calls.forgetExternalTunnel += 1;
              return (
                scenario.forgetExternalTunnel?.() ??
                unexpectedEndpoint("access.forgetExternalTunnel")
              );
            })
            .handle("cloudflaredExecutables", () => {
              calls.cloudflaredExecutables += 1;
              return (
                scenario.cloudflaredExecutables?.() ??
                unexpectedEndpoint("access.cloudflaredExecutables")
              );
            })
            .handle("cloudflaredInstallation", () => {
              calls.cloudflaredInstallation += 1;
              return (
                scenario.cloudflaredInstallation?.() ??
                unexpectedEndpoint("access.cloudflaredInstallation")
              );
            })
            .handle("installCloudflaredRelease", ({ payload }) => {
              calls.installCloudflaredRelease.push(payload);
              return (
                scenario.installCloudflaredRelease?.(payload) ??
                unexpectedEndpoint("access.installCloudflaredRelease")
              );
            })
            .handle("managedCloudflareConnector", () => {
              calls.managedCloudflareConnector += 1;
              return (
                scenario.managedCloudflareConnector?.() ??
                unexpectedEndpoint("access.managedCloudflareConnector")
              );
            })
            .handle("configureManagedCloudflareConnector", ({ payload }) => {
              calls.configureManagedCloudflareConnector.push(payload);
              return (
                scenario.configureManagedCloudflareConnector?.(payload) ??
                unexpectedEndpoint("access.configureManagedCloudflareConnector")
              );
            })
            .handle("startManagedCloudflareConnector", () => {
              calls.startManagedCloudflareConnector += 1;
              return (
                scenario.startManagedCloudflareConnector?.() ??
                unexpectedEndpoint("access.startManagedCloudflareConnector")
              );
            })
            .handle("stopManagedCloudflareConnector", () => {
              calls.stopManagedCloudflareConnector += 1;
              return (
                scenario.stopManagedCloudflareConnector?.() ??
                unexpectedEndpoint("access.stopManagedCloudflareConnector")
              );
            })
            .handle("retryManagedCloudflareConnector", () => {
              calls.retryManagedCloudflareConnector += 1;
              return (
                scenario.retryManagedCloudflareConnector?.() ??
                unexpectedEndpoint("access.retryManagedCloudflareConnector")
              );
            })
            .handle("cloudflareAccount", () => {
              calls.cloudflareAccount += 1;
              return (
                scenario.cloudflareAccount?.() ?? unexpectedEndpoint("access.cloudflareAccount")
              );
            })
            .handle("beginCloudflareLogin", ({ payload }) => {
              calls.beginCloudflareLogin.push(payload);
              return (
                scenario.beginCloudflareLogin?.(payload) ??
                unexpectedEndpoint("access.beginCloudflareLogin")
              );
            })
            .handle("cancelCloudflareLogin", () => {
              calls.cancelCloudflareLogin += 1;
              return (
                scenario.cancelCloudflareLogin?.() ??
                unexpectedEndpoint("access.cancelCloudflareLogin")
              );
            })
            .handle("consentToCloudflareCertificate", ({ payload }) => {
              calls.consentToCloudflareCertificate.push(payload);
              return (
                scenario.consentToCloudflareCertificate?.(payload) ??
                unexpectedEndpoint("access.consentToCloudflareCertificate")
              );
            })
            .handle("listCloudflareTunnels", ({ payload }) => {
              calls.listCloudflareTunnels.push(payload);
              return (
                scenario.listCloudflareTunnels?.(payload) ??
                unexpectedEndpoint("access.listCloudflareTunnels")
              );
            })
            .handle("selectCloudflareTunnel", ({ payload }) => {
              calls.selectCloudflareTunnel.push(payload);
              return (
                scenario.selectCloudflareTunnel?.(payload) ??
                unexpectedEndpoint("access.selectCloudflareTunnel")
              );
            })
            .handle("adoptCloudflareTunnel", ({ payload }) => {
              calls.adoptCloudflareTunnel.push(payload);
              return (
                scenario.adoptCloudflareTunnel?.(payload) ??
                unexpectedEndpoint("access.adoptCloudflareTunnel")
              );
            })
            // laplus answering itself through the public hostname, never a
            // client. See the contract's own comment on these two.
            .handle("externalTunnelHttpChallenge", () =>
              unexpectedEndpoint("access.externalTunnelHttpChallenge"),
            )
            .handle("externalTunnelWebSocketChallenge", () =>
              unexpectedEndpoint("access.externalTunnelWebSocketChallenge"),
            ),
        ),
      ]),
      Effect.provideService(EnvironmentAuthenticatedAuth, authenticatedAuth),
      Effect.scoped,
    ),
  );

  const runtime = ManagedRuntime.make(Layer.succeed(PrimaryEnvironmentHttpClient, client));
  __setPrimaryHttpRunnerForTests((effect) => runtime.runPromise(effect));

  return {
    calls,
    async dispose() {
      __setPrimaryHttpRunnerForTests();
      await runtime.dispose();
    },
  };
}
