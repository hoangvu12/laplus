import type {
  AuthBrowserSessionResult,
  AuthClientMetadata,
  AuthEnvironmentScope,
  AuthPairingCredentialResult,
  ServerAuthSessionMethod,
  AuthSessionId,
  AuthSessionState,
} from "@t3tools/contracts";
import {
  EnvironmentHttpCommonError,
  EnvironmentPublicExposureRefusal,
  PRIMARY_LOCAL_ENVIRONMENT_ID,
  type ApproveCloudflaredReleaseInput,
  type CloudflareAccountSnapshot,
  type CloudflaredExecutableDiscovery,
  type CloudflaredInstallationSnapshot,
  type ConfigureManagedCloudflareConnectorInput,
  type ExternalTunnelEndpointSnapshot,
  type ManagedCloudflareConnectorSnapshot,
  type CreateCloudflareTunnelInput,
  type SelectCloudflareTunnelInput,
} from "@t3tools/contracts";
import type { EnvironmentHttpCommonError as EnvironmentHttpCommonErrorType } from "@t3tools/contracts";
import * as DateTime from "effect/DateTime";
import * as Effect from "effect/Effect";
import * as Schema from "effect/Schema";
import { HttpClientError } from "effect/unstable/http";

import {
  getPairingTokenFromUrl,
  stripPairingTokenFromUrl as stripPairingTokenUrl,
} from "../../pairingUrl";

import { PrimaryEnvironmentHttpClient } from "./httpClient";
import { runPrimaryHttp } from "../../lib/runtime";

const PrimaryEnvironmentRequestOperation = Schema.Literals([
  "fetch-session-state",
  "exchange-bootstrap-credential",
  "fetch-environment-descriptor",
  "create-pairing-credential",
  "list-pairing-links",
  "revoke-pairing-link",
  "list-client-sessions",
  "revoke-client-session",
  "revoke-other-client-sessions",
  "read-external-tunnel-endpoint",
  "register-external-tunnel-endpoint",
  "test-external-tunnel-endpoint",
  "forget-external-tunnel-endpoint",
  "discover-cloudflared-executables",
  "read-cloudflared-installation",
  "install-cloudflared-release",
  "read-managed-cloudflare-connector",
  "configure-managed-cloudflare-connector",
  "start-managed-cloudflare-connector",
  "stop-managed-cloudflare-connector",
  "retry-managed-cloudflare-connector",
  "read-cloudflare-account",
  "begin-cloudflare-login",
  "cancel-cloudflare-login",
  "consent-to-cloudflare-certificate",
  "list-cloudflare-tunnels",
  "select-cloudflare-tunnel",
  "adopt-cloudflare-tunnel",
  "create-cloudflare-tunnel",
]);
type PrimaryEnvironmentRequestOperation = typeof PrimaryEnvironmentRequestOperation.Type;

export class PrimaryEnvironmentRequestError extends Schema.TaggedErrorClass<PrimaryEnvironmentRequestError>()(
  "PrimaryEnvironmentRequestError",
  {
    operation: PrimaryEnvironmentRequestOperation,
    status: Schema.Number,
    pairingLinkId: Schema.optional(Schema.String),
    sessionId: Schema.optional(Schema.String),
    cause: Schema.Defect(),
  },
) {
  static fromCause(input: {
    readonly operation: PrimaryEnvironmentRequestOperation;
    readonly cause: unknown;
    readonly pairingLinkId?: string;
    readonly sessionId?: string;
  }): PrimaryEnvironmentRequestError {
    const status = readHttpApiStatus(input.cause) ?? 500;
    return new PrimaryEnvironmentRequestError({
      operation: input.operation,
      status,
      ...(input.pairingLinkId !== undefined ? { pairingLinkId: input.pairingLinkId } : {}),
      ...(input.sessionId !== undefined ? { sessionId: input.sessionId } : {}),
      cause: input.cause,
    });
  }

  override get message(): string {
    return `Primary environment request failed during ${this.operation} (HTTP ${this.status}).`;
  }
}

export const isPrimaryEnvironmentRequestError = Schema.is(PrimaryEnvironmentRequestError);

export class PrimaryEnvironmentPairingCredentialRejectedError extends Schema.TaggedErrorClass<PrimaryEnvironmentPairingCredentialRejectedError>()(
  "PrimaryEnvironmentPairingCredentialRejectedError",
  {
    providedLength: Schema.Number,
    cause: Schema.Defect(),
  },
) {
  override get message(): string {
    return "Invalid pairing token. Check the token and try again.";
  }
}

export const isPrimaryEnvironmentPairingCredentialRejectedError = Schema.is(
  PrimaryEnvironmentPairingCredentialRejectedError,
);

export class PrimaryEnvironmentAuthSessionTimeoutError extends Schema.TaggedErrorClass<PrimaryEnvironmentAuthSessionTimeoutError>()(
  "PrimaryEnvironmentAuthSessionTimeoutError",
  {
    timeoutMs: Schema.Number,
    elapsedMs: Schema.Number,
  },
) {
  override get message(): string {
    return "Timed out waiting for authenticated session after bootstrap.";
  }
}

export const isPrimaryEnvironmentAuthSessionTimeoutError = Schema.is(
  PrimaryEnvironmentAuthSessionTimeoutError,
);

export class PrimaryEnvironmentPairingCredentialRequiredError extends Schema.TaggedErrorClass<PrimaryEnvironmentPairingCredentialRequiredError>()(
  "PrimaryEnvironmentPairingCredentialRequiredError",
  {
    providedLength: Schema.Number,
  },
) {
  override get message(): string {
    return "Enter a pairing token to continue.";
  }
}

export const isPrimaryEnvironmentPairingCredentialRequiredError = Schema.is(
  PrimaryEnvironmentPairingCredentialRequiredError,
);

const isEnvironmentHttpCommonError = Schema.is(EnvironmentHttpCommonError);

/**
 * Whether a decoded error is one of the two public-exposure refusals.
 *
 * Private: callers want {@link publicExposureRefusal}, which also unwraps the
 * transport. A Cloudflare refusal used to arrive as an untagged `{ message }`
 * that decoded as nothing, so the UI could show only the transport's own
 * summary — see {@link PrimaryEnvironmentRequestError}.
 */
const isPublicExposureRefusal = Schema.is(EnvironmentPublicExposureRefusal);

/**
 * The refusal behind a failed public-exposure request, if there was one.
 *
 * The transport wraps whatever the client decoded, so the tagged refusal is the
 * `cause` rather than the error itself.
 */
export function publicExposureRefusal(error: unknown): EnvironmentPublicExposureRefusal | null {
  if (isPublicExposureRefusal(error)) return error;
  if (isPrimaryEnvironmentRequestError(error) && isPublicExposureRefusal(error.cause)) {
    return error.cause;
  }
  return null;
}

export interface ServerPairingLinkRecord {
  readonly id: string;
  readonly credential: string;
  readonly scopes: ReadonlyArray<AuthEnvironmentScope>;
  readonly subject: string;
  readonly label?: string;
  readonly createdAt: string;
  readonly expiresAt: string;
}

export interface ServerClientSessionRecord {
  readonly sessionId: AuthSessionId;
  readonly subject: string;
  readonly scopes: ReadonlyArray<AuthEnvironmentScope>;
  readonly method: ServerAuthSessionMethod;
  readonly client: AuthClientMetadata;
  readonly issuedAt: string;
  readonly expiresAt: string;
  readonly lastConnectedAt: string | null;
  readonly connected: boolean;
  readonly current: boolean;
}

type ServerAuthGateState =
  | { status: "authenticated" }
  | {
      status: "requires-auth";
      auth: AuthSessionState["auth"];
      errorMessage?: string;
    };

let bootstrapPromise: Promise<ServerAuthGateState> | null = null;
let resolvedAuthenticatedGateState: ServerAuthGateState | null = null;
const AUTH_SESSION_ESTABLISH_TIMEOUT_MS = 2_000;
const AUTH_SESSION_ESTABLISH_STEP_MS = 100;

export function peekPairingTokenFromUrl(): string | null {
  return getPairingTokenFromUrl(new URL(window.location.href));
}

export function stripPairingTokenFromUrl() {
  const url = new URL(window.location.href);
  const next = stripPairingTokenUrl(url);
  if (next.toString() === url.toString()) {
    return;
  }
  window.history.replaceState({}, document.title, next.toString());
}

export function takePairingTokenFromUrl(): string | null {
  const token = peekPairingTokenFromUrl();
  if (!token) {
    return null;
  }
  stripPairingTokenFromUrl();
  return token;
}

// Routes that own the pairing fragment themselves. `PairingRouteSurface` reads
// the token out of the URL and auto-submits it, and a phone's pairing code is
// single-use — so the gate reading it first would spend it and leave that
// screen submitting a code the server has already consumed.
const PAIRING_FRAGMENT_OWNING_ROUTES = ["/pair", "/connect"];

function ownsPairingFragmentItself(pathname: string): boolean {
  return PAIRING_FRAGMENT_OWNING_ROUTES.some(
    (route) => pathname === route || pathname.startsWith(`${route}/`),
  );
}

function getBootstrapCredential(): string | null {
  // Both backends share the same bootstrap token (DesktopBackendConfiguration
  // mints one tokenRef and feeds it to both resolvers), so picking the
  // primary entry is fine even when the WSL backend is also registered.
  const bootstraps = window.desktopBridge?.getLocalEnvironmentBootstraps() ?? [];
  const primary = bootstraps.find((entry) => entry.id === PRIMARY_LOCAL_ENVIRONMENT_ID);
  if (typeof primary?.bootstrapToken === "string" && primary.bootstrapToken.length > 0) {
    return primary.bootstrapToken;
  }

  // laplus has no `window.desktopBridge`. Its shell and its server are one
  // process, so there is no Electron preload to hand a token across — the
  // window is opened at `http://127.0.0.1:4773/#token=…` instead, and a URL
  // fragment is never sent to the server, which is what makes it a private
  // channel rather than a credential in a request log. `Server::window_url` in
  // `server/crates/laplus-server/src/server.rs` is the other end.
  //
  // Peeked rather than taken: the boot grant is deliberately re-usable so that
  // reloading the window re-reads it, and stripping it here would spend the
  // address bar's only copy on a session cookie that a cleared cookie jar can
  // outlive.
  if (ownsPairingFragmentItself(window.location.pathname)) {
    return null;
  }
  return peekPairingTokenFromUrl();
}

export async function fetchSessionState(): Promise<AuthSessionState> {
  return retryTransientBootstrap(async () => {
    try {
      return await runPrimaryHttp(
        PrimaryEnvironmentHttpClient.pipe(
          Effect.flatMap((client) => client.auth.session({ headers: {} })),
        ),
      );
    } catch (error) {
      throw PrimaryEnvironmentRequestError.fromCause({
        operation: "fetch-session-state",
        cause: error,
      });
    }
  });
}

function readHttpApiStatus(error: unknown): number | null {
  if (isEnvironmentHttpCommonError(error)) {
    return readEnvironmentHttpErrorStatus(error);
  }
  // A precondition the developer has to satisfy is a 409; a rejection is a 400.
  // Read from the tag rather than from the response, because by the time this
  // runs the client has already decoded the body into one of the two.
  if (isPublicExposureRefusal(error)) {
    return error._tag === "EnvironmentPublicExposurePreconditionError" ? 409 : 400;
  }
  return HttpClientError.isHttpClientError(error) && error.response !== undefined
    ? error.response.status
    : null;
}

function readEnvironmentHttpErrorStatus(error: EnvironmentHttpCommonErrorType): number {
  switch (error._tag) {
    case "EnvironmentRequestInvalidError":
      return 400;
    case "EnvironmentAuthInvalidError":
      return 401;
    case "EnvironmentScopeRequiredError":
    case "EnvironmentOperationForbiddenError":
      return 403;
    case "EnvironmentResourceNotFoundError":
      return 404;
    case "EnvironmentInternalError":
      return 500;
  }
}

async function exchangeBootstrapCredential(credential: string): Promise<AuthBrowserSessionResult> {
  return retryTransientBootstrap(async () => {
    try {
      return await runPrimaryHttp(
        PrimaryEnvironmentHttpClient.pipe(
          Effect.flatMap((client) => client.auth.browserSession({ payload: { credential } })),
        ),
      );
    } catch (error) {
      if (
        isEnvironmentHttpCommonError(error) &&
        error._tag === "EnvironmentAuthInvalidError" &&
        error.reason === "invalid_credential"
      ) {
        throw new PrimaryEnvironmentPairingCredentialRejectedError({
          providedLength: credential.length,
          cause: error,
        });
      }
      throw PrimaryEnvironmentRequestError.fromCause({
        operation: "exchange-bootstrap-credential",
        cause: error,
      });
    }
  });
}

async function waitForAuthenticatedSessionAfterBootstrap(): Promise<AuthSessionState> {
  const startedAt = Date.now();

  while (true) {
    const session = await fetchSessionState();
    if (session.authenticated) {
      return session;
    }

    const elapsedMs = Date.now() - startedAt;
    if (elapsedMs >= AUTH_SESSION_ESTABLISH_TIMEOUT_MS) {
      throw new PrimaryEnvironmentAuthSessionTimeoutError({
        timeoutMs: AUTH_SESSION_ESTABLISH_TIMEOUT_MS,
        elapsedMs,
      });
    }

    await waitForBootstrapRetry(AUTH_SESSION_ESTABLISH_STEP_MS);
  }
}

const TRANSIENT_BOOTSTRAP_STATUS_CODES = new Set([502, 503, 504]);
const BOOTSTRAP_RETRY_TIMEOUT_MS = 15_000;
const BOOTSTRAP_RETRY_STEP_MS = 500;

export async function retryTransientBootstrap<T>(operation: () => Promise<T>): Promise<T> {
  const startedAt = Date.now();
  while (true) {
    try {
      return await operation();
    } catch (error) {
      if (!isTransientBootstrapError(error)) {
        throw error;
      }

      if (Date.now() - startedAt >= BOOTSTRAP_RETRY_TIMEOUT_MS) {
        throw error;
      }

      await waitForBootstrapRetry(BOOTSTRAP_RETRY_STEP_MS);
    }
  }
}

function waitForBootstrapRetry(delayMs: number): Promise<void> {
  return new Promise((resolve) => {
    setTimeout(resolve, delayMs);
  });
}

function isTransientBootstrapError(error: unknown): boolean {
  if (isPrimaryEnvironmentRequestError(error)) {
    return TRANSIENT_BOOTSTRAP_STATUS_CODES.has(error.status);
  }

  if (error instanceof TypeError) {
    return true;
  }

  return error instanceof DOMException && error.name === "AbortError";
}

async function bootstrapServerAuth(): Promise<ServerAuthGateState> {
  const bootstrapCredential = getBootstrapCredential();
  const currentSession = await fetchSessionState();
  if (currentSession.authenticated) {
    return { status: "authenticated" };
  }

  if (!bootstrapCredential) {
    return {
      status: "requires-auth",
      auth: currentSession.auth,
    };
  }

  try {
    await exchangeBootstrapCredential(bootstrapCredential);
    await waitForAuthenticatedSessionAfterBootstrap();
    return { status: "authenticated" };
  } catch (error) {
    return {
      status: "requires-auth",
      auth: currentSession.auth,
      errorMessage: error instanceof Error ? error.message : "Authentication failed.",
    };
  }
}

export async function submitServerAuthCredential(credential: string): Promise<void> {
  const trimmedCredential = credential.trim();
  if (!trimmedCredential) {
    throw new PrimaryEnvironmentPairingCredentialRequiredError({
      providedLength: credential.length,
    });
  }

  resolvedAuthenticatedGateState = null;
  await exchangeBootstrapCredential(trimmedCredential);
  bootstrapPromise = null;
  stripPairingTokenFromUrl();
}

export async function createServerPairingCredential(input?: {
  readonly label?: string;
  readonly scopes?: ReadonlyArray<AuthEnvironmentScope>;
}): Promise<AuthPairingCredentialResult> {
  const trimmedLabel = input?.label?.trim();
  try {
    return await runPrimaryHttp(
      PrimaryEnvironmentHttpClient.pipe(
        Effect.flatMap((client) =>
          client.auth.pairingCredential({
            headers: {},
            payload: {
              ...(trimmedLabel ? { label: trimmedLabel } : {}),
              ...(input?.scopes ? { scopes: input.scopes } : {}),
            },
          }),
        ),
      ),
    );
  } catch (error) {
    throw PrimaryEnvironmentRequestError.fromCause({
      operation: "create-pairing-credential",
      cause: error,
    });
  }
}

export async function readExternalTunnelEndpoint(): Promise<ExternalTunnelEndpointSnapshot> {
  try {
    return await runPrimaryHttp(
      PrimaryEnvironmentHttpClient.pipe(
        Effect.flatMap((client) => client.access.externalTunnel({ headers: {} })),
      ),
    );
  } catch (error) {
    throw PrimaryEnvironmentRequestError.fromCause({
      operation: "read-external-tunnel-endpoint",
      cause: error,
    });
  }
}

export async function registerExternalTunnelEndpoint(
  hostname: string,
): Promise<ExternalTunnelEndpointSnapshot> {
  try {
    return await runPrimaryHttp(
      PrimaryEnvironmentHttpClient.pipe(
        Effect.flatMap((client) =>
          client.access.registerExternalTunnel({ headers: {}, payload: { hostname } }),
        ),
      ),
    );
  } catch (error) {
    throw PrimaryEnvironmentRequestError.fromCause({
      operation: "register-external-tunnel-endpoint",
      cause: error,
    });
  }
}

export async function testExternalTunnelEndpoint(): Promise<ExternalTunnelEndpointSnapshot> {
  try {
    return await runPrimaryHttp(
      PrimaryEnvironmentHttpClient.pipe(
        Effect.flatMap((client) => client.access.testExternalTunnel({ headers: {} })),
      ),
    );
  } catch (error) {
    throw PrimaryEnvironmentRequestError.fromCause({
      operation: "test-external-tunnel-endpoint",
      cause: error,
    });
  }
}

export async function forgetExternalTunnelEndpoint(): Promise<ExternalTunnelEndpointSnapshot> {
  try {
    return await runPrimaryHttp(
      PrimaryEnvironmentHttpClient.pipe(
        Effect.flatMap((client) => client.access.forgetExternalTunnel({ headers: {} })),
      ),
    );
  } catch (error) {
    throw PrimaryEnvironmentRequestError.fromCause({
      operation: "forget-external-tunnel-endpoint",
      cause: error,
    });
  }
}

export async function discoverCloudflaredExecutables(): Promise<CloudflaredExecutableDiscovery> {
  try {
    return await runPrimaryHttp(
      PrimaryEnvironmentHttpClient.pipe(
        Effect.flatMap((client) => client.access.cloudflaredExecutables({ headers: {} })),
      ),
    );
  } catch (error) {
    throw PrimaryEnvironmentRequestError.fromCause({
      operation: "discover-cloudflared-executables",
      cause: error,
    });
  }
}

export async function readCloudflaredInstallation(): Promise<CloudflaredInstallationSnapshot> {
  try {
    return await runPrimaryHttp(
      PrimaryEnvironmentHttpClient.pipe(
        Effect.flatMap((client) => client.access.cloudflaredInstallation({ headers: {} })),
      ),
    );
  } catch (error) {
    throw PrimaryEnvironmentRequestError.fromCause({
      operation: "read-cloudflared-installation",
      cause: error,
    });
  }
}

export async function installCloudflaredRelease(
  payload: ApproveCloudflaredReleaseInput,
): Promise<CloudflaredInstallationSnapshot> {
  try {
    return await runPrimaryHttp(
      PrimaryEnvironmentHttpClient.pipe(
        Effect.flatMap((client) =>
          client.access.installCloudflaredRelease({ headers: {}, payload }),
        ),
      ),
    );
  } catch (error) {
    throw PrimaryEnvironmentRequestError.fromCause({
      operation: "install-cloudflared-release",
      cause: error,
    });
  }
}

export async function readManagedCloudflareConnector(): Promise<ManagedCloudflareConnectorSnapshot> {
  try {
    return await runPrimaryHttp(
      PrimaryEnvironmentHttpClient.pipe(
        Effect.flatMap((client) => client.access.managedCloudflareConnector({ headers: {} })),
      ),
    );
  } catch (error) {
    throw PrimaryEnvironmentRequestError.fromCause({
      operation: "read-managed-cloudflare-connector",
      cause: error,
    });
  }
}

export async function configureManagedCloudflareConnector(
  payload: ConfigureManagedCloudflareConnectorInput,
): Promise<ManagedCloudflareConnectorSnapshot> {
  try {
    return await runPrimaryHttp(
      PrimaryEnvironmentHttpClient.pipe(
        Effect.flatMap((client) =>
          client.access.configureManagedCloudflareConnector({ headers: {}, payload }),
        ),
      ),
    );
  } catch (error) {
    throw PrimaryEnvironmentRequestError.fromCause({
      operation: "configure-managed-cloudflare-connector",
      cause: error,
    });
  }
}

async function mutateManagedCloudflareConnector(
  operation: "start" | "stop" | "retry",
): Promise<ManagedCloudflareConnectorSnapshot> {
  try {
    return await runPrimaryHttp(
      PrimaryEnvironmentHttpClient.pipe(
        Effect.flatMap((client) =>
          operation === "start"
            ? client.access.startManagedCloudflareConnector({ headers: {} })
            : operation === "stop"
              ? client.access.stopManagedCloudflareConnector({ headers: {} })
              : client.access.retryManagedCloudflareConnector({ headers: {} }),
        ),
      ),
    );
  } catch (error) {
    throw PrimaryEnvironmentRequestError.fromCause({
      operation: `${operation}-managed-cloudflare-connector`,
      cause: error,
    });
  }
}

export const startManagedCloudflareConnector = () => mutateManagedCloudflareConnector("start");
export const stopManagedCloudflareConnector = () => mutateManagedCloudflareConnector("stop");
export const retryManagedCloudflareConnector = () => mutateManagedCloudflareConnector("retry");

/**
 * Cloudflare account authorization and the tunnels it can see.
 *
 * Every one of these answers with the whole snapshot, including the step an
 * interrupted setup resumes at, so the wizard never has to remember its own
 * progress — see `CloudflareAccountSnapshot` in the contract.
 */
export async function readCloudflareAccount(): Promise<CloudflareAccountSnapshot> {
  try {
    return await runPrimaryHttp(
      PrimaryEnvironmentHttpClient.pipe(
        Effect.flatMap((client) => client.access.cloudflareAccount({ headers: {} })),
      ),
    );
  } catch (error) {
    throw PrimaryEnvironmentRequestError.fromCause({
      operation: "read-cloudflare-account",
      cause: error,
    });
  }
}

export async function beginCloudflareLogin(
  executablePath: string,
): Promise<CloudflareAccountSnapshot> {
  try {
    return await runPrimaryHttp(
      PrimaryEnvironmentHttpClient.pipe(
        Effect.flatMap((client) =>
          client.access.beginCloudflareLogin({ headers: {}, payload: { executablePath } }),
        ),
      ),
    );
  } catch (error) {
    throw PrimaryEnvironmentRequestError.fromCause({
      operation: "begin-cloudflare-login",
      cause: error,
    });
  }
}

export async function cancelCloudflareLogin(): Promise<CloudflareAccountSnapshot> {
  try {
    return await runPrimaryHttp(
      PrimaryEnvironmentHttpClient.pipe(
        Effect.flatMap((client) => client.access.cancelCloudflareLogin({ headers: {} })),
      ),
    );
  } catch (error) {
    throw PrimaryEnvironmentRequestError.fromCause({
      operation: "cancel-cloudflare-login",
      cause: error,
    });
  }
}

export async function consentToCloudflareCertificate(
  consented: boolean,
): Promise<CloudflareAccountSnapshot> {
  try {
    return await runPrimaryHttp(
      PrimaryEnvironmentHttpClient.pipe(
        Effect.flatMap((client) =>
          client.access.consentToCloudflareCertificate({ headers: {}, payload: { consented } }),
        ),
      ),
    );
  } catch (error) {
    throw PrimaryEnvironmentRequestError.fromCause({
      operation: "consent-to-cloudflare-certificate",
      cause: error,
    });
  }
}

export async function listCloudflareTunnels(
  executablePath: string,
): Promise<CloudflareAccountSnapshot> {
  try {
    return await runPrimaryHttp(
      PrimaryEnvironmentHttpClient.pipe(
        Effect.flatMap((client) =>
          client.access.listCloudflareTunnels({ headers: {}, payload: { executablePath } }),
        ),
      ),
    );
  } catch (error) {
    throw PrimaryEnvironmentRequestError.fromCause({
      operation: "list-cloudflare-tunnels",
      cause: error,
    });
  }
}

export async function selectCloudflareTunnel(
  payload: SelectCloudflareTunnelInput,
): Promise<CloudflareAccountSnapshot> {
  try {
    return await runPrimaryHttp(
      PrimaryEnvironmentHttpClient.pipe(
        Effect.flatMap((client) => client.access.selectCloudflareTunnel({ headers: {}, payload })),
      ),
    );
  } catch (error) {
    throw PrimaryEnvironmentRequestError.fromCause({
      operation: "select-cloudflare-tunnel",
      cause: error,
    });
  }
}

/**
 * Dedicate the selected inactive tunnel to this environment.
 *
 * Sends the executable rather than the tunnel, because which tunnel is being
 * dedicated is the selection the server already recorded — a client that named
 * it again could name a different one, and dedication is the point at which
 * that stops being a harmless disagreement.
 */
export async function adoptCloudflareTunnel(
  executablePath: string,
): Promise<CloudflareAccountSnapshot> {
  try {
    return await runPrimaryHttp(
      PrimaryEnvironmentHttpClient.pipe(
        Effect.flatMap((client) =>
          client.access.adoptCloudflareTunnel({ headers: {}, payload: { executablePath } }),
        ),
      ),
    );
  } catch (error) {
    throw PrimaryEnvironmentRequestError.fromCause({
      operation: "adopt-cloudflare-tunnel",
      cause: error,
    });
  }
}

/**
 * Create a stable tunnel for this environment and route a hostname to it.
 *
 * Sends the name and the hostname because neither exists anywhere yet — unlike
 * dedication, which names nothing and lets the server use the selection it
 * already recorded. The UUID Cloudflare allocates comes back in the answer's
 * selection rather than being asked for, because there is nothing to ask about
 * until Cloudflare has made it.
 */
export async function createCloudflareTunnel(
  payload: CreateCloudflareTunnelInput,
): Promise<CloudflareAccountSnapshot> {
  try {
    return await runPrimaryHttp(
      PrimaryEnvironmentHttpClient.pipe(
        Effect.flatMap((client) => client.access.createCloudflareTunnel({ headers: {}, payload })),
      ),
    );
  } catch (error) {
    throw PrimaryEnvironmentRequestError.fromCause({
      operation: "create-cloudflare-tunnel",
      cause: error,
    });
  }
}

export async function listServerPairingLinks(): Promise<ReadonlyArray<ServerPairingLinkRecord>> {
  try {
    const pairingLinks = await runPrimaryHttp(
      PrimaryEnvironmentHttpClient.pipe(
        Effect.flatMap((client) => client.auth.pairingLinks({ headers: {} })),
      ),
    );
    return pairingLinks.map((pairingLink) => {
      const timestamps = {
        createdAt: DateTime.formatIso(pairingLink.createdAt),
        expiresAt: DateTime.formatIso(pairingLink.expiresAt),
      };
      if (pairingLink.label === undefined) {
        return {
          id: pairingLink.id,
          credential: pairingLink.credential,
          scopes: pairingLink.scopes,
          subject: pairingLink.subject,
          createdAt: timestamps.createdAt,
          expiresAt: timestamps.expiresAt,
        };
      }
      return {
        id: pairingLink.id,
        credential: pairingLink.credential,
        scopes: pairingLink.scopes,
        subject: pairingLink.subject,
        label: pairingLink.label,
        createdAt: timestamps.createdAt,
        expiresAt: timestamps.expiresAt,
      };
    });
  } catch (error) {
    throw PrimaryEnvironmentRequestError.fromCause({
      operation: "list-pairing-links",
      cause: error,
    });
  }
}

export async function revokeServerPairingLink(id: string): Promise<void> {
  try {
    await runPrimaryHttp(
      PrimaryEnvironmentHttpClient.pipe(
        Effect.flatMap((client) => client.auth.revokePairingLink({ headers: {}, payload: { id } })),
      ),
    );
  } catch (error) {
    throw PrimaryEnvironmentRequestError.fromCause({
      operation: "revoke-pairing-link",
      pairingLinkId: id,
      cause: error,
    });
  }
}

export async function listServerClientSessions(): Promise<
  ReadonlyArray<ServerClientSessionRecord>
> {
  try {
    const clientSessions = await runPrimaryHttp(
      PrimaryEnvironmentHttpClient.pipe(
        Effect.flatMap((client) => client.auth.clients({ headers: {} })),
      ),
    );
    return clientSessions.map((clientSession) => ({
      sessionId: clientSession.sessionId,
      subject: clientSession.subject,
      scopes: clientSession.scopes,
      method: clientSession.method,
      client: clientSession.client,
      issuedAt: DateTime.formatIso(clientSession.issuedAt),
      expiresAt: DateTime.formatIso(clientSession.expiresAt),
      lastConnectedAt:
        clientSession.lastConnectedAt === null
          ? null
          : DateTime.formatIso(clientSession.lastConnectedAt),
      connected: clientSession.connected,
      current: clientSession.current,
    }));
  } catch (error) {
    throw PrimaryEnvironmentRequestError.fromCause({
      operation: "list-client-sessions",
      cause: error,
    });
  }
}

export async function revokeServerClientSession(sessionId: AuthSessionId): Promise<void> {
  try {
    await runPrimaryHttp(
      PrimaryEnvironmentHttpClient.pipe(
        Effect.flatMap((client) =>
          client.auth.revokeClient({ headers: {}, payload: { sessionId } }),
        ),
      ),
    );
  } catch (error) {
    throw PrimaryEnvironmentRequestError.fromCause({
      operation: "revoke-client-session",
      sessionId,
      cause: error,
    });
  }
}

export async function revokeOtherServerClientSessions(): Promise<number> {
  try {
    const result = await runPrimaryHttp(
      PrimaryEnvironmentHttpClient.pipe(
        Effect.flatMap((client) => client.auth.revokeOtherClients({ headers: {} })),
      ),
    );
    return result.revokedCount;
  } catch (error) {
    throw PrimaryEnvironmentRequestError.fromCause({
      operation: "revoke-other-client-sessions",
      cause: error,
    });
  }
}

export async function resolveInitialServerAuthGateState(): Promise<ServerAuthGateState> {
  if (resolvedAuthenticatedGateState?.status === "authenticated") {
    return resolvedAuthenticatedGateState;
  }

  if (bootstrapPromise) {
    return bootstrapPromise;
  }

  const nextPromise = bootstrapServerAuth();
  bootstrapPromise = nextPromise;
  return nextPromise
    .then((result) => {
      if (result.status === "authenticated") {
        resolvedAuthenticatedGateState = result;
      }
      return result;
    })
    .finally(() => {
      if (bootstrapPromise === nextPromise) {
        bootstrapPromise = null;
      }
    });
}

// Used by the WSL backend swap: invalidate the cached authenticated state
// (the new backend signs sessions with a different key) and re-bootstrap
// against the desktop bootstrap credential so the next WS reconnect doesn't
// hit 401 and start a reauth loop in the renderer.
export async function reauthenticatePrimaryEnvironment(): Promise<ServerAuthGateState> {
  resolvedAuthenticatedGateState = null;
  bootstrapPromise = null;
  return resolveInitialServerAuthGateState();
}

export function __resetServerAuthBootstrapForTests() {
  bootstrapPromise = null;
  resolvedAuthenticatedGateState = null;
}
