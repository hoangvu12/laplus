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
  type ExternalTunnelEndpointSnapshot,
  type CloudflaredExecutableDiscovery,
  type ConfigureManagedCloudflareConnectorInput,
  type ManagedCloudflareConnectorSnapshot,
  type RegisterExternalTunnelEndpointInput,
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
  readonly managedCloudflareConnector?: () => Effect.Effect<ManagedCloudflareConnectorSnapshot>;
  readonly configureManagedCloudflareConnector?: (
    payload: ConfigureManagedCloudflareConnectorInput,
  ) => Effect.Effect<ManagedCloudflareConnectorSnapshot>;
  readonly startManagedCloudflareConnector?: () => Effect.Effect<ManagedCloudflareConnectorSnapshot>;
  readonly stopManagedCloudflareConnector?: () => Effect.Effect<ManagedCloudflareConnectorSnapshot>;
  readonly retryManagedCloudflareConnector?: () => Effect.Effect<ManagedCloudflareConnectorSnapshot>;
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
  managedCloudflareConnector: number;
  configureManagedCloudflareConnector: Array<ConfigureManagedCloudflareConnectorInput>;
  startManagedCloudflareConnector: number;
  stopManagedCloudflareConnector: number;
  retryManagedCloudflareConnector: number;
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
    managedCloudflareConnector: 0,
    configureManagedCloudflareConnector: [],
    startManagedCloudflareConnector: 0,
    stopManagedCloudflareConnector: 0,
    retryManagedCloudflareConnector: 0,
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
            }),
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
