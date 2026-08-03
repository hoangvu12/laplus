import * as Context from "effect/Context";
import type * as DateTime from "effect/DateTime";
import * as Schema from "effect/Schema";
import * as HttpApi from "effect/unstable/httpapi/HttpApi";
import * as HttpApiEndpoint from "effect/unstable/httpapi/HttpApiEndpoint";
import * as HttpApiGroup from "effect/unstable/httpapi/HttpApiGroup";
import * as HttpApiMiddleware from "effect/unstable/httpapi/HttpApiMiddleware";
import * as HttpServerRespondable from "effect/unstable/http/HttpServerRespondable";
import * as HttpServerResponse from "effect/unstable/http/HttpServerResponse";

import {
  AuthAccessTokenResult,
  AuthBrowserSessionRequest,
  AuthBrowserSessionResult,
  AuthClientSession,
  AuthCreatePairingCredentialInput,
  AuthPairingCredentialResult,
  AuthPairingLink,
  AuthRevokeClientSessionInput,
  AuthRevokePairingLinkInput,
  AuthEnvironmentScope,
  AuthTokenExchangeRequest,
  AuthSessionState,
  AuthWebSocketTicketResult,
  ServerAuthSessionMethod,
} from "./auth.ts";
import { AuthSessionId, ThreadId, TrimmedNonEmptyString } from "./baseSchemas.ts";
import { ExecutionEnvironmentDescriptor } from "./environment.ts";
export { PublicExposureMutationStep } from "./remoteAccess.ts";

import {
  ApproveCloudflaredReleaseInput,
  CloudflareAccountCommandInput,
  CloudflareAccountSnapshot,
  CloudflareCertificateConsentInput,
  CloudflareDeletionPlan,
  CloudflaredInstallationSnapshot,
  DeleteCloudflareTunnelInput,
  ExternalTunnelChallengeResult,
  ExternalTunnelEndpointSnapshot,
  CloudflaredExecutableDiscovery,
  PublicExposureMutationStep,
  ConfigureManagedCloudflareConnectorInput,
  CreateCloudflareTunnelInput,
  ManagedCloudflareConnectorSnapshot,
  RegisterExternalTunnelEndpointInput,
  SelectCloudflareTunnelInput,
} from "./remoteAccess.ts";
import {
  ClientOrchestrationCommand,
  DispatchResult,
  OrchestrationReadModel,
  OrchestrationShellSnapshot,
  OrchestrationThreadDetailSnapshot,
} from "./orchestration.ts";

const OptionalBearerHeaders = Schema.Struct({
  authorization: Schema.optionalKey(Schema.String),
  dpop: Schema.optionalKey(Schema.String),
});

const OptionalDpopProofHeaders = Schema.Struct({
  dpop: Schema.optionalKey(Schema.String),
});

export const EnvironmentRequestInvalidReason = Schema.Literals([
  "invalid_scope",
  "scope_not_granted",
  "invalid_command",
]);
export type EnvironmentRequestInvalidReason = typeof EnvironmentRequestInvalidReason.Type;

export const EnvironmentAuthInvalidReason = Schema.Literals([
  "missing_credential",
  "invalid_credential",
]);
export type EnvironmentAuthInvalidReason = typeof EnvironmentAuthInvalidReason.Type;

export const EnvironmentOperationForbiddenReason = Schema.Literals([
  "current_session_revoke_not_allowed",
]);
export type EnvironmentOperationForbiddenReason = typeof EnvironmentOperationForbiddenReason.Type;

export const EnvironmentInternalErrorReason = Schema.Literals([
  "bootstrap_validation_failed",
  "browser_session_issuance_failed",
  "browser_session_cookie_failed",
  "access_token_issuance_failed",
  "websocket_ticket_issuance_failed",
  "pairing_credential_issuance_failed",
  "pairing_links_load_failed",
  "pairing_link_revoke_failed",
  "client_sessions_load_failed",
  "client_session_revoke_failed",
  "orchestration_snapshot_failed",
  "orchestration_thread_snapshot_failed",
  "orchestration_dispatch_failed",
  "internal_error",
]);
export type EnvironmentInternalErrorReason = typeof EnvironmentInternalErrorReason.Type;

export class EnvironmentRequestInvalidError extends Schema.TaggedErrorClass<EnvironmentRequestInvalidError>()(
  "EnvironmentRequestInvalidError",
  {
    code: Schema.Literal("invalid_request"),
    reason: EnvironmentRequestInvalidReason,
    traceId: TrimmedNonEmptyString,
  },
  { httpApiStatus: 400 },
) {
  [HttpServerRespondable.symbol]() {
    return HttpServerResponse.schemaJson(EnvironmentRequestInvalidError)(this, { status: 400 });
  }
}

export class EnvironmentAuthInvalidError extends Schema.TaggedErrorClass<EnvironmentAuthInvalidError>()(
  "EnvironmentAuthInvalidError",
  {
    code: Schema.Literal("auth_invalid"),
    reason: EnvironmentAuthInvalidReason,
    traceId: TrimmedNonEmptyString,
  },
  { httpApiStatus: 401 },
) {
  [HttpServerRespondable.symbol]() {
    return HttpServerResponse.schemaJson(EnvironmentAuthInvalidError)(this, { status: 401 });
  }
}

export class EnvironmentScopeRequiredError extends Schema.TaggedErrorClass<EnvironmentScopeRequiredError>()(
  "EnvironmentScopeRequiredError",
  {
    code: Schema.Literal("insufficient_scope"),
    requiredScope: AuthEnvironmentScope,
    traceId: TrimmedNonEmptyString,
  },
  { httpApiStatus: 403 },
) {
  [HttpServerRespondable.symbol]() {
    return HttpServerResponse.schemaJson(EnvironmentScopeRequiredError)(this, { status: 403 });
  }
}

export class EnvironmentOperationForbiddenError extends Schema.TaggedErrorClass<EnvironmentOperationForbiddenError>()(
  "EnvironmentOperationForbiddenError",
  {
    code: Schema.Literal("operation_forbidden"),
    reason: EnvironmentOperationForbiddenReason,
    traceId: TrimmedNonEmptyString,
  },
  { httpApiStatus: 403 },
) {
  [HttpServerRespondable.symbol]() {
    return HttpServerResponse.schemaJson(EnvironmentOperationForbiddenError)(this, { status: 403 });
  }
}

export class EnvironmentInternalError extends Schema.TaggedErrorClass<EnvironmentInternalError>()(
  "EnvironmentInternalError",
  {
    code: Schema.Literal("internal_error"),
    reason: EnvironmentInternalErrorReason,
    traceId: TrimmedNonEmptyString,
  },
  { httpApiStatus: 500 },
) {
  [HttpServerRespondable.symbol]() {
    return HttpServerResponse.schemaJson(EnvironmentInternalError)(this, { status: 500 });
  }
}

export const EnvironmentResourceNotFoundReason = Schema.Literals(["thread_not_found"]);
export type EnvironmentResourceNotFoundReason = typeof EnvironmentResourceNotFoundReason.Type;

export class EnvironmentResourceNotFoundError extends Schema.TaggedErrorClass<EnvironmentResourceNotFoundError>()(
  "EnvironmentResourceNotFoundError",
  {
    code: Schema.Literal("not_found"),
    reason: EnvironmentResourceNotFoundReason,
    traceId: TrimmedNonEmptyString,
  },
  { httpApiStatus: 404 },
) {
  [HttpServerRespondable.symbol]() {
    return HttpServerResponse.schemaJson(EnvironmentResourceNotFoundError)(this, { status: 404 });
  }
}

/**
 * Why a public-exposure command was refused.
 *
 * **A closed set rather than a sentence, because the sentence never arrived.**
 * Every Cloudflare route answered a refusal as an untagged `{ "message": … }`,
 * which decodes as no declared error at all — so the client had a 409 or a 400
 * and nothing to render, and the reason a developer needed was thrown away at
 * the boundary. That is Gap 4 in `.scratch/contract-parity/ledger.md`.
 *
 * It is a *set* rather than free text because tickets 05, 06 and 07 are all
 * specified in terms of a client deciding what to offer next: an activation
 * race offers a different recovery from a missing consent, and "the tunnel is
 * not laplus's to delete" must be legible to the UI without parsing prose.
 */
export const PublicExposureRefusalReason = Schema.Literals([
  /** Sign in to Cloudflare first. */
  "sign-in-required",
  /** The account certificate may not be used until its authority is accepted. */
  "consent-required",
  /** The chosen tunnel is no longer in the listing; refresh and choose again. */
  "selection-stale",
  /** There is no connector to act on yet. */
  "connector-required",
  /** Nothing is running that could be cancelled. */
  "nothing-running",
  /** laplus already owns this exposure, or another owner already does. */
  "ownership-conflict",
  /** Automatic restarts are spent; an explicit retry is required. */
  "restarts-exhausted",
  /** The named cloudflared cannot be started, or is too old. */
  "executable-unusable",
  /** The hostname is not a bare public HTTPS host. */
  "hostname-invalid",
  /** The approved release is no longer the one the feed offers. */
  "release-moved",
  /** cloudflared ran and said no. */
  "command-failed",
  /**
   * laplus could not write its own private configuration or credential.
   * Distinct from `command-failed` because nothing at Cloudflare went wrong
   * and the retry is local.
   */
  "local-setup-failed",
  /**
   * The tunnel became active between listing and mutation, so it is externally
   * managed after all. Ticket 05's activation race.
   */
  "tunnel-became-active",
  /**
   * Only a laplus-created tunnel may be deleted at Cloudflare. Ticket 07 —
   * a server-side refusal, not merely a hidden button.
   */
  "not-laplus-created",
  /**
   * A previous mutation left Cloudflare or local state half-changed, and that
   * has to be resolved before this command can run. Ticket 07.
   */
  "cleanup-required",
  /**
   * The name for a tunnel laplus would create is not one Cloudflare accepts.
   * Distinct from `hostname-invalid` because creation asks for two different
   * things — what to call the tunnel and where it answers — and a developer
   * given one message for both cannot tell which field to fix. Ticket 06.
   */
  "tunnel-name-invalid",
  /**
   * The destructive confirmation is missing, expired, already spent, or names
   * resources this environment no longer records.
   *
   * This is what ticket 07's "fresh `access:write` authorization" is enforced
   * as: a deletion is authorized by a value the server minted for the exact
   * tunnel and DNS record it will remove, used once and expiring shortly, and
   * checked against the endpoint row as it stands at the moment the command
   * runs. A session scope answers who may ask; it cannot answer what they were
   * shown. `server/docs/adr/0052`.
   */
  "confirmation-required",
  /**
   * laplus has no Cloudflare authority that can remove the recorded DNS record.
   * Distinct from `command-failed` because no command ran and none could:
   * `cloudflared` has no `route dns delete` at all, so the developer's next
   * action is to supply a Cloudflare API token with DNS edit permission for the
   * hostname's zone. Ticket 07.
   */
  "dns-authority-required",
]);
export type PublicExposureRefusalReason = typeof PublicExposureRefusalReason.Type;

const PublicExposureRefusalFields = {
  code: Schema.Literal("public_exposure_refused"),
  reason: PublicExposureRefusalReason,
  /** What to show a developer. Never contains a secret; see the redaction rule. */
  message: Schema.String,
  /**
   * Mutations that did happen and must not be repeated by a retry. Empty for
   * every refusal that changed nothing, which is most of them.
   */
  completed: Schema.Array(PublicExposureMutationStep),
  /**
   * Mutations that were started and never settled — the exact remaining work.
   * A non-empty list is the difference between "nothing happened" and "this is
   * half done", which is the distinction ticket 07 forbids the UI to blur.
   */
  remaining: Schema.Array(PublicExposureMutationStep),
  traceId: TrimmedNonEmptyString,
} as const;

/**
 * The developer has to do something before this command can run.
 *
 * `409`, matching the status the Cloudflare routes have answered since ticket
 * 01 — the shape changed, not the code.
 */
export class EnvironmentPublicExposurePreconditionError extends Schema.TaggedErrorClass<EnvironmentPublicExposurePreconditionError>()(
  "EnvironmentPublicExposurePreconditionError",
  PublicExposureRefusalFields,
  { httpApiStatus: 409 },
) {
  [HttpServerRespondable.symbol]() {
    return HttpServerResponse.schemaJson(EnvironmentPublicExposurePreconditionError)(this, {
      status: 409,
    });
  }
}

/** cloudflared, its output, or the request itself said no. `400`. */
export class EnvironmentPublicExposureRejectedError extends Schema.TaggedErrorClass<EnvironmentPublicExposureRejectedError>()(
  "EnvironmentPublicExposureRejectedError",
  PublicExposureRefusalFields,
  { httpApiStatus: 400 },
) {
  [HttpServerRespondable.symbol]() {
    return HttpServerResponse.schemaJson(EnvironmentPublicExposureRejectedError)(this, {
      status: 400,
    });
  }
}

/**
 * Either refusal a public-exposure command can answer with.
 *
 * **`EnvironmentScopeRequiredError` is deliberately not folded in here.** A
 * client without `access:write` is refused before any of this is evaluated and
 * learns only the scope it needs, which is ADR-0047's rule that a refusal
 * discloses nothing: a reason from this set would tell an unauthorized caller
 * whether a tunnel exists, whether it is laplus-created, and how far setup got.
 */
export const EnvironmentPublicExposureRefusal = Schema.Union([
  EnvironmentPublicExposurePreconditionError,
  EnvironmentPublicExposureRejectedError,
]);
export type EnvironmentPublicExposureRefusal = typeof EnvironmentPublicExposureRefusal.Type;

export const EnvironmentHttpCommonError = Schema.Union([
  EnvironmentRequestInvalidError,
  EnvironmentAuthInvalidError,
  EnvironmentScopeRequiredError,
  EnvironmentOperationForbiddenError,
  EnvironmentResourceNotFoundError,
  EnvironmentInternalError,
]);
export type EnvironmentHttpCommonError = typeof EnvironmentHttpCommonError.Type;

const EnvironmentAuthenticationErrors = [
  EnvironmentAuthInvalidError,
  EnvironmentInternalError,
] as const;

export class EnvironmentHttpBadRequestError extends Schema.TaggedErrorClass<EnvironmentHttpBadRequestError>()(
  "EnvironmentHttpBadRequestError",
  {
    message: Schema.String,
  },
  { httpApiStatus: 400 },
) {
  [HttpServerRespondable.symbol]() {
    return HttpServerResponse.schemaJson(EnvironmentHttpBadRequestError)(this, { status: 400 });
  }
}

export class EnvironmentHttpUnauthorizedError extends Schema.TaggedErrorClass<EnvironmentHttpUnauthorizedError>()(
  "EnvironmentHttpUnauthorizedError",
  {
    message: Schema.String,
  },
  { httpApiStatus: 401 },
) {
  [HttpServerRespondable.symbol]() {
    return HttpServerResponse.schemaJson(EnvironmentHttpUnauthorizedError)(this, { status: 401 });
  }
}

export class EnvironmentHttpForbiddenError extends Schema.TaggedErrorClass<EnvironmentHttpForbiddenError>()(
  "EnvironmentHttpForbiddenError",
  {
    message: Schema.String,
  },
  { httpApiStatus: 403 },
) {
  [HttpServerRespondable.symbol]() {
    return HttpServerResponse.schemaJson(EnvironmentHttpForbiddenError)(this, { status: 403 });
  }
}

export class EnvironmentHttpInternalServerError extends Schema.TaggedErrorClass<EnvironmentHttpInternalServerError>()(
  "EnvironmentHttpInternalServerError",
  {
    message: Schema.String,
  },
  { httpApiStatus: 500 },
) {
  [HttpServerRespondable.symbol]() {
    return HttpServerResponse.schemaJson(EnvironmentHttpInternalServerError)(this, { status: 500 });
  }
}

export class EnvironmentHttpConflictError extends Schema.TaggedErrorClass<EnvironmentHttpConflictError>()(
  "EnvironmentHttpConflictError",
  {
    message: Schema.String,
  },
  { httpApiStatus: 409 },
) {
  [HttpServerRespondable.symbol]() {
    return HttpServerResponse.schemaJson(EnvironmentHttpConflictError)(this, { status: 409 });
  }
}

export class EnvironmentCloudEndpointUnavailableError extends Schema.TaggedErrorClass<EnvironmentCloudEndpointUnavailableError>()(
  "EnvironmentCloudEndpointUnavailableError",
  {
    message: Schema.String,
    endpointRuntimeStatus: Schema.Unknown,
  },
  { httpApiStatus: 503 },
) {
  [HttpServerRespondable.symbol]() {
    return HttpServerResponse.schemaJson(EnvironmentCloudEndpointUnavailableError)(this, {
      status: 503,
    });
  }
}
const EnvironmentSessionCreationErrors = [
  EnvironmentAuthInvalidError,
  EnvironmentInternalError,
] as const;
const EnvironmentTokenExchangeErrors = [
  EnvironmentRequestInvalidError,
  EnvironmentAuthInvalidError,
  EnvironmentInternalError,
] as const;
const EnvironmentScopedOperationErrors = [
  EnvironmentScopeRequiredError,
  EnvironmentInternalError,
] as const;
const EnvironmentPairingCredentialErrors = [
  EnvironmentRequestInvalidError,
  ...EnvironmentScopedOperationErrors,
] as const;
const EnvironmentSessionRevokeErrors = [
  EnvironmentScopeRequiredError,
  EnvironmentOperationForbiddenError,
  EnvironmentInternalError,
] as const;
const EnvironmentOrchestrationSnapshotErrors = [
  EnvironmentScopeRequiredError,
  EnvironmentInternalError,
] as const;
const EnvironmentOrchestrationThreadSnapshotErrors = [
  EnvironmentScopeRequiredError,
  EnvironmentResourceNotFoundError,
  EnvironmentInternalError,
] as const;
const EnvironmentOrchestrationDispatchErrors = [
  EnvironmentRequestInvalidError,
  EnvironmentScopeRequiredError,
  EnvironmentInternalError,
] as const;

export interface EnvironmentSessionPrincipalShape {
  readonly sessionId: AuthSessionId;
  readonly subject: string;
  readonly method: ServerAuthSessionMethod;
  readonly scopes: ReadonlySet<AuthEnvironmentScope>;
  readonly proofKeyThumbprint?: string;
  readonly expiresAt?: DateTime.DateTime;
}

export class EnvironmentAuthenticatedPrincipal extends Context.Service<
  EnvironmentAuthenticatedPrincipal,
  EnvironmentSessionPrincipalShape
>()("@t3tools/contracts/environmentHttp/EnvironmentAuthenticatedPrincipal") {}

export class EnvironmentAuthenticatedAuth extends HttpApiMiddleware.Service<
  EnvironmentAuthenticatedAuth,
  { provides: EnvironmentAuthenticatedPrincipal }
>()("EnvironmentAuthenticatedAuth", {
  error: EnvironmentAuthenticationErrors,
}) {}

export const AuthPairingLinkRevokeResult = Schema.Struct({
  revoked: Schema.Boolean,
});
export type AuthPairingLinkRevokeResult = typeof AuthPairingLinkRevokeResult.Type;

export const AuthClientSessionRevokeResult = Schema.Struct({
  revoked: Schema.Boolean,
});
export type AuthClientSessionRevokeResult = typeof AuthClientSessionRevokeResult.Type;

export const AuthOtherClientSessionsRevokeResult = Schema.Struct({
  revokedCount: Schema.Number,
});
export type AuthOtherClientSessionsRevokeResult = typeof AuthOtherClientSessionsRevokeResult.Type;

export class EnvironmentMetadataHttpApi extends HttpApiGroup.make("metadata").add(
  HttpApiEndpoint.get("descriptor", "/.well-known/t3/environment", {
    success: ExecutionEnvironmentDescriptor,
  }),
) {}

export class EnvironmentAuthHttpApi extends HttpApiGroup.make("auth")
  .add(
    HttpApiEndpoint.get("session", "/api/auth/session", {
      headers: OptionalBearerHeaders,
      success: AuthSessionState,
      error: [EnvironmentInternalError],
    }),
  )
  .add(
    HttpApiEndpoint.post("browserSession", "/api/auth/browser-session", {
      payload: AuthBrowserSessionRequest,
      success: AuthBrowserSessionResult,
      error: EnvironmentSessionCreationErrors,
    }),
  )
  .add(
    HttpApiEndpoint.post("token", "/oauth/token", {
      headers: OptionalDpopProofHeaders,
      payload: AuthTokenExchangeRequest,
      success: AuthAccessTokenResult,
      error: EnvironmentTokenExchangeErrors,
    }),
  )
  .add(
    HttpApiEndpoint.post("webSocketTicket", "/api/auth/websocket-ticket", {
      headers: OptionalBearerHeaders,
      success: AuthWebSocketTicketResult,
      error: [EnvironmentInternalError],
    }).middleware(EnvironmentAuthenticatedAuth),
  )
  .add(
    HttpApiEndpoint.post("pairingCredential", "/api/auth/pairing-token", {
      headers: OptionalBearerHeaders,
      payload: AuthCreatePairingCredentialInput,
      success: AuthPairingCredentialResult,
      error: EnvironmentPairingCredentialErrors,
    }).middleware(EnvironmentAuthenticatedAuth),
  )
  .add(
    HttpApiEndpoint.get("pairingLinks", "/api/auth/pairing-links", {
      headers: OptionalBearerHeaders,
      success: Schema.Array(AuthPairingLink),
      error: EnvironmentScopedOperationErrors,
    }).middleware(EnvironmentAuthenticatedAuth),
  )
  .add(
    HttpApiEndpoint.post("revokePairingLink", "/api/auth/pairing-links/revoke", {
      headers: OptionalBearerHeaders,
      payload: AuthRevokePairingLinkInput,
      success: AuthPairingLinkRevokeResult,
      error: EnvironmentScopedOperationErrors,
    }).middleware(EnvironmentAuthenticatedAuth),
  )
  .add(
    HttpApiEndpoint.get("clients", "/api/auth/clients", {
      headers: OptionalBearerHeaders,
      success: Schema.Array(AuthClientSession),
      error: EnvironmentScopedOperationErrors,
    }).middleware(EnvironmentAuthenticatedAuth),
  )
  .add(
    HttpApiEndpoint.post("revokeClient", "/api/auth/clients/revoke", {
      headers: OptionalBearerHeaders,
      payload: AuthRevokeClientSessionInput,
      success: AuthClientSessionRevokeResult,
      error: EnvironmentSessionRevokeErrors,
    }).middleware(EnvironmentAuthenticatedAuth),
  )
  .add(
    HttpApiEndpoint.post("revokeOtherClients", "/api/auth/clients/revoke-others", {
      headers: OptionalBearerHeaders,
      success: AuthOtherClientSessionsRevokeResult,
      error: EnvironmentScopedOperationErrors,
    }).middleware(EnvironmentAuthenticatedAuth),
  ) {}

/**
 * What a public-exposure *mutation* can answer with.
 *
 * The scope refusal comes first and on its own — see
 * {@link EnvironmentPublicExposureRefusal} for why a client that fails it never
 * sees one of these.
 */
const EnvironmentPublicExposureErrors = [
  EnvironmentScopeRequiredError,
  EnvironmentPublicExposurePreconditionError,
  EnvironmentPublicExposureRejectedError,
  EnvironmentInternalError,
] as const;

export class EnvironmentAccessHttpApi extends HttpApiGroup.make("access")
  .add(
    HttpApiEndpoint.get("externalTunnel", "/api/access/cloudflare", {
      headers: OptionalBearerHeaders,
      success: ExternalTunnelEndpointSnapshot,
      error: EnvironmentScopedOperationErrors,
    }).middleware(EnvironmentAuthenticatedAuth),
  )
  .add(
    HttpApiEndpoint.post("registerExternalTunnel", "/api/access/cloudflare", {
      headers: OptionalBearerHeaders,
      payload: RegisterExternalTunnelEndpointInput,
      success: ExternalTunnelEndpointSnapshot,
      error: EnvironmentPublicExposureErrors,
    }).middleware(EnvironmentAuthenticatedAuth),
  )
  .add(
    HttpApiEndpoint.post("testExternalTunnel", "/api/access/cloudflare/test", {
      headers: OptionalBearerHeaders,
      success: ExternalTunnelEndpointSnapshot,
      error: EnvironmentScopedOperationErrors,
    }).middleware(EnvironmentAuthenticatedAuth),
  )
  .add(
    HttpApiEndpoint.post("forgetExternalTunnel", "/api/access/cloudflare/forget", {
      headers: OptionalBearerHeaders,
      success: ExternalTunnelEndpointSnapshot,
      error: EnvironmentScopedOperationErrors,
    }).middleware(EnvironmentAuthenticatedAuth),
  )
  .add(
    // The two challenge routes below are laplus answering itself through the
    // public hostname, so neither carries `EnvironmentAuthenticatedAuth`: the
    // credential is a single-use diagnostic token this server minted for one
    // probe, not a session, and a paired client has no reason to call either.
    // They are declared so that an audit of `/api/access/cloudflare` finds every
    // path this server serves rather than only the ones a client drives.
    HttpApiEndpoint.get("externalTunnelHttpChallenge", "/api/access/cloudflare/challenge", {
      headers: OptionalBearerHeaders,
      success: ExternalTunnelChallengeResult,
      error: [EnvironmentHttpUnauthorizedError],
    }),
  )
  .add(
    // **A 101, which `HttpApi` has no way to describe.** The success schema is
    // therefore `Void` and describes nothing: what proves this route works is
    // `tests/http_public_exposure.rs` and the production verifier, not a
    // generated client. Calling it through the generated client would open no
    // socket, which is why nothing does.
    HttpApiEndpoint.get("externalTunnelWebSocketChallenge", "/api/access/cloudflare/challenge/ws", {
      headers: OptionalBearerHeaders,
      success: Schema.Void,
      error: [EnvironmentHttpUnauthorizedError],
    }),
  )
  .add(
    HttpApiEndpoint.get("cloudflaredExecutables", "/api/access/cloudflare/executables", {
      headers: OptionalBearerHeaders,
      success: CloudflaredExecutableDiscovery,
      error: EnvironmentScopedOperationErrors,
    }).middleware(EnvironmentAuthenticatedAuth),
  )
  .add(
    HttpApiEndpoint.get("cloudflaredInstallation", "/api/access/cloudflare/install", {
      headers: OptionalBearerHeaders,
      success: CloudflaredInstallationSnapshot,
      error: EnvironmentScopedOperationErrors,
    }).middleware(EnvironmentAuthenticatedAuth),
  )
  .add(
    HttpApiEndpoint.post("installCloudflaredRelease", "/api/access/cloudflare/install", {
      headers: OptionalBearerHeaders,
      payload: ApproveCloudflaredReleaseInput,
      success: CloudflaredInstallationSnapshot,
      error: EnvironmentPublicExposureErrors,
    }).middleware(EnvironmentAuthenticatedAuth),
  )
  .add(
    HttpApiEndpoint.get("cloudflareAccount", "/api/access/cloudflare/account", {
      headers: OptionalBearerHeaders,
      success: CloudflareAccountSnapshot,
      error: EnvironmentScopedOperationErrors,
    }).middleware(EnvironmentAuthenticatedAuth),
  )
  .add(
    // Every account action below answers with the whole snapshot rather than an
    // acknowledgement, so the wizard's next step is read from the same place a
    // reopened dialog reads it and the two cannot disagree.
    //
    // **A refused one answers with a tagged refusal**, which it did not used to.
    // Until this cleanup pass, 409 and 400 both carried an untagged
    // `{ message }` that decoded as no declared error at all, so the reason a
    // developer needed never reached the browser — Gap 4 in
    // `.scratch/contract-parity/ledger.md`. `EnvironmentPublicExposureRefusal`
    // is that shape, and it is on every route that can refuse for a reason
    // other than scope.
    HttpApiEndpoint.post("beginCloudflareLogin", "/api/access/cloudflare/account/login", {
      headers: OptionalBearerHeaders,
      payload: CloudflareAccountCommandInput,
      success: CloudflareAccountSnapshot,
      error: EnvironmentPublicExposureErrors,
    }).middleware(EnvironmentAuthenticatedAuth),
  )
  .add(
    HttpApiEndpoint.post("cancelCloudflareLogin", "/api/access/cloudflare/account/login/cancel", {
      headers: OptionalBearerHeaders,
      success: CloudflareAccountSnapshot,
      error: EnvironmentPublicExposureErrors,
    }).middleware(EnvironmentAuthenticatedAuth),
  )
  .add(
    HttpApiEndpoint.post(
      "consentToCloudflareCertificate",
      "/api/access/cloudflare/account/consent",
      {
        headers: OptionalBearerHeaders,
        payload: CloudflareCertificateConsentInput,
        success: CloudflareAccountSnapshot,
        error: EnvironmentPublicExposureErrors,
      },
    ).middleware(EnvironmentAuthenticatedAuth),
  )
  .add(
    // A POST that mutates nothing at Cloudflare: it runs `tunnel list`, which is
    // a read there and a write here, because it spends the account certificate.
    // Repeating it reconciles what laplus knows, which is what makes an
    // interrupted discovery safe to simply run again.
    HttpApiEndpoint.post("listCloudflareTunnels", "/api/access/cloudflare/account/tunnels", {
      headers: OptionalBearerHeaders,
      payload: CloudflareAccountCommandInput,
      success: CloudflareAccountSnapshot,
      error: EnvironmentPublicExposureErrors,
    }).middleware(EnvironmentAuthenticatedAuth),
  )
  .add(
    HttpApiEndpoint.post("selectCloudflareTunnel", "/api/access/cloudflare/account/select", {
      headers: OptionalBearerHeaders,
      payload: SelectCloudflareTunnelInput,
      success: CloudflareAccountSnapshot,
      error: EnvironmentPublicExposureErrors,
    }).middleware(EnvironmentAuthenticatedAuth),
  )
  .add(
    // Dedicating an inactive existing tunnel to this environment: retrieve its
    // narrow run credential, write laplus's own isolated configuration, and
    // supervise a connector for it. The Cloudflare allocation and DNS route stay
    // owned outside laplus — ADR-0045 — which is what `adopted` means and why it
    // authorizes no deletion.
    //
    // **Repeating it is a reconciliation.** An adoption already recorded answers
    // with what it recorded; an interrupted one resumes from the credential and
    // configuration that are actually there. `tunnel-became-active` is the
    // refusal when a connector started between the offer and this call, and it
    // arrives with the hostname registered as an external tunnel endpoint
    // instead.
    HttpApiEndpoint.post("adoptCloudflareTunnel", "/api/access/cloudflare/account/adopt", {
      headers: OptionalBearerHeaders,
      payload: CloudflareAccountCommandInput,
      success: CloudflareAccountSnapshot,
      error: EnvironmentPublicExposureErrors,
    }).middleware(EnvironmentAuthenticatedAuth),
  )
  .add(
    // Creating a stable tunnel for this environment: allocate it, route a DNS
    // name to it, write laplus's own isolated configuration, and supervise a
    // connector. The one path that ends in `laplus-created` — the only tunnel
    // ownership that authorizes deleting anything at Cloudflare (ADR-0049).
    //
    // **Three mutations, and every one of them can be the last.** A partial
    // creation is refused with the exact steps completed and outstanding, and
    // never claims a rollback: there is no `tunnel delete` in this call.
    // Repeating it reconciles against the credential that is on disk, the DNS
    // record the endpoint row names, and the connector's own configuration, so a
    // resume after a timeout, disconnect or restart duplicates no resource.
    HttpApiEndpoint.post("createCloudflareTunnel", "/api/access/cloudflare/account/create", {
      headers: OptionalBearerHeaders,
      payload: CreateCloudflareTunnelInput,
      success: CloudflareAccountSnapshot,
      error: EnvironmentPublicExposureErrors,
    }).middleware(EnvironmentAuthenticatedAuth),
  )
  .add(
    // What a deletion would remove, and the one-time authorization to remove it.
    //
    // **A POST that deletes nothing**, because it mints something: the offer is
    // the separate destructive confirmation ticket 07 requires, and it is made
    // by the server so that what a developer agrees to is the recorded tunnel
    // and DNS record rather than whatever a client believed. Refused with
    // `not-laplus-created` for every other ownership, from the same value the
    // deletion itself refuses on — so the offer and the refusal cannot come
    // apart. `server/docs/adr/0052`.
    HttpApiEndpoint.post("offerCloudflareDeletion", "/api/access/cloudflare/account/deletion", {
      headers: OptionalBearerHeaders,
      success: CloudflareDeletionPlan,
      error: EnvironmentPublicExposureErrors,
    }).middleware(EnvironmentAuthenticatedAuth),
  )
  .add(
    // Deleting the exact Cloudflare resources laplus created, and then its own
    // local setup — the one command in this API that removes something at
    // Cloudflare.
    //
    // **Four journaled steps at three places**: the DNS record through
    // Cloudflare's DNS API, because `cloudflared` has no `route dns delete`; the
    // tunnel with `cloudflared tunnel delete`; and then laplus's own
    // configuration and credential, which is what `forgetExternalTunnel` does on
    // its own. A partial failure answers with the exact work completed and
    // outstanding and never claims a rollback, and repeating it skips what is
    // already done — a record Cloudflare says is not there is read as done
    // rather than as a new failure.
    //
    // Refused with `not-laplus-created` for an adopted or external tunnel,
    // `confirmation-required` for an authorization that is missing, spent,
    // expired or names something else, and `dns-authority-required` when there
    // is no Cloudflare DNS authority to remove the record with — that last one
    // before anything is attempted, because a deletion that removed the tunnel
    // and left the record would be a weaker operation rather than a recoverable
    // state.
    HttpApiEndpoint.post("deleteCloudflareTunnel", "/api/access/cloudflare/account/delete", {
      headers: OptionalBearerHeaders,
      payload: DeleteCloudflareTunnelInput,
      success: ExternalTunnelEndpointSnapshot,
      error: EnvironmentPublicExposureErrors,
    }).middleware(EnvironmentAuthenticatedAuth),
  )
  .add(
    HttpApiEndpoint.get("managedCloudflareConnector", "/api/access/cloudflare/connector", {
      headers: OptionalBearerHeaders,
      success: ManagedCloudflareConnectorSnapshot,
      error: EnvironmentScopedOperationErrors,
    }).middleware(EnvironmentAuthenticatedAuth),
  )
  .add(
    HttpApiEndpoint.post(
      "configureManagedCloudflareConnector",
      "/api/access/cloudflare/connector/configure",
      {
        headers: OptionalBearerHeaders,
        payload: ConfigureManagedCloudflareConnectorInput,
        success: ManagedCloudflareConnectorSnapshot,
        error: EnvironmentPublicExposureErrors,
      },
    ).middleware(EnvironmentAuthenticatedAuth),
  )
  .add(
    HttpApiEndpoint.post(
      "startManagedCloudflareConnector",
      "/api/access/cloudflare/connector/start",
      {
        headers: OptionalBearerHeaders,
        success: ManagedCloudflareConnectorSnapshot,
        error: EnvironmentPublicExposureErrors,
      },
    ).middleware(EnvironmentAuthenticatedAuth),
  )
  .add(
    HttpApiEndpoint.post(
      "stopManagedCloudflareConnector",
      "/api/access/cloudflare/connector/stop",
      {
        headers: OptionalBearerHeaders,
        success: ManagedCloudflareConnectorSnapshot,
        error: EnvironmentPublicExposureErrors,
      },
    ).middleware(EnvironmentAuthenticatedAuth),
  )
  .add(
    HttpApiEndpoint.post(
      "retryManagedCloudflareConnector",
      "/api/access/cloudflare/connector/retry",
      {
        headers: OptionalBearerHeaders,
        success: ManagedCloudflareConnectorSnapshot,
        error: EnvironmentPublicExposureErrors,
      },
    ).middleware(EnvironmentAuthenticatedAuth),
  ) {}

const EnvironmentOrchestrationThreadSnapshotParams = Schema.Struct({
  threadId: ThreadId,
});

export class EnvironmentOrchestrationHttpApi extends HttpApiGroup.make("orchestration")
  .add(
    HttpApiEndpoint.get("snapshot", "/api/orchestration/snapshot", {
      headers: OptionalBearerHeaders,
      success: OrchestrationReadModel,
      error: EnvironmentOrchestrationSnapshotErrors,
    }).middleware(EnvironmentAuthenticatedAuth),
  )
  .add(
    HttpApiEndpoint.get("shellSnapshot", "/api/orchestration/shell", {
      headers: OptionalBearerHeaders,
      success: OrchestrationShellSnapshot,
      error: EnvironmentOrchestrationSnapshotErrors,
    }).middleware(EnvironmentAuthenticatedAuth),
  )
  .add(
    HttpApiEndpoint.get("threadSnapshot", "/api/orchestration/threads/:threadId", {
      headers: OptionalBearerHeaders,
      params: EnvironmentOrchestrationThreadSnapshotParams,
      success: OrchestrationThreadDetailSnapshot,
      error: EnvironmentOrchestrationThreadSnapshotErrors,
    }).middleware(EnvironmentAuthenticatedAuth),
  )
  .add(
    HttpApiEndpoint.post("dispatch", "/api/orchestration/dispatch", {
      headers: OptionalBearerHeaders,
      payload: ClientOrchestrationCommand,
      success: DispatchResult,
      error: EnvironmentOrchestrationDispatchErrors,
    }).middleware(EnvironmentAuthenticatedAuth),
  ) {}

export class EnvironmentHttpApi extends HttpApi.make("environment")
  .add(EnvironmentMetadataHttpApi)
  .add(EnvironmentAuthHttpApi)
  .add(EnvironmentAccessHttpApi)
  .add(EnvironmentOrchestrationHttpApi) {}
