import {
  EnvironmentAuthInvalidError,
  type AuthBrowserSessionResult,
  type AuthCreatePairingCredentialInput,
  type AuthSessionState,
  type DesktopBridge,
} from "@t3tools/contracts";
import * as DateTime from "effect/DateTime";
import * as Effect from "effect/Effect";
import { HttpClientError, HttpClientRequest, HttpClientResponse } from "effect/unstable/http";
import { afterEach, beforeEach, describe, expect, it, vi } from "vite-plus/test";

import { installEnvironmentHttpTest } from "../test/environmentHttpTest";
import { __setPrimaryHttpRunnerForTests, type PrimaryHttpEffectRunner } from "./lib/runtime";

/**
 * A setup nothing has removed anything from — what these fixtures are unless
 * they say otherwise. Ticket 07 made the cleanup report part of every endpoint
 * snapshot, because it is the one answer that survives the setup it describes.
 */
const INTACT_CLEANUP = {
  state: "intact",
  completed: [],
  remaining: [],
  tunnelId: null,
  dnsRecordName: null,
} as const;

type TestWindow = {
  location: URL;
  history: {
    replaceState: (_data: unknown, _unused: string, url: string) => void;
  };
  desktopBridge?: DesktopBridge;
};

const LOOPBACK_AUTH = {
  policy: "loopback-browser",
  bootstrapMethods: ["one-time-token"],
  sessionMethods: ["browser-session-cookie"],
  sessionCookieName: "t3_session",
} as const;

const DESKTOP_AUTH = {
  policy: "desktop-managed-local",
  bootstrapMethods: ["desktop-bootstrap"],
  sessionMethods: ["browser-session-cookie"],
  sessionCookieName: "t3_session",
} as const;

const SESSION_EXPIRES_AT = DateTime.makeUnsafe("2026-04-05T00:00:00.000Z");
const unauthenticatedSession = (auth: AuthSessionState["auth"]): AuthSessionState => ({
  authenticated: false,
  auth,
});

const authenticatedSession = (auth: AuthSessionState["auth"]): AuthSessionState => ({
  authenticated: true,
  auth,
  sessionMethod: "browser-session-cookie",
  expiresAt: SESSION_EXPIRES_AT,
});

const browserSession = (scopes: AuthBrowserSessionResult["scopes"]): AuthBrowserSessionResult => ({
  authenticated: true,
  scopes,
  sessionMethod: "browser-session-cookie",
  expiresAt: SESSION_EXPIRES_AT,
});

function installTestBrowser(url: string) {
  const testWindow: TestWindow = {
    location: new URL(url),
    history: {
      replaceState: (_data, _unused, nextUrl) => {
        testWindow.location = new URL(nextUrl, testWindow.location.href);
      },
    },
  };

  vi.stubGlobal("window", testWindow);
  vi.stubGlobal("document", { title: "T3 Code" });

  return testWindow;
}

function installDesktopBootstrap() {
  const testWindow = installTestBrowser("http://localhost/");
  testWindow.desktopBridge = {
    getLocalEnvironmentBootstraps: () => [
      {
        id: "primary",
        label: "Local environment",
        httpBaseUrl: "http://localhost:3773",
        wsBaseUrl: "ws://localhost:3773",
        bootstrapToken: "desktop-bootstrap-token",
      },
    ],
  } as unknown as DesktopBridge;
}

function sequence<A>(...values: ReadonlyArray<A>) {
  let index = 0;
  return () => values[Math.min(index++, values.length - 1)]!;
}

let disposeHttpTest: (() => Promise<void>) | undefined;

async function installAuthApi(input: {
  readonly session?: () => AuthSessionState;
  readonly browserSession?: (
    credential: string,
  ) => Effect.Effect<AuthBrowserSessionResult, EnvironmentAuthInvalidError>;
  readonly pairingCredential?: (payload: AuthCreatePairingCredentialInput) => Effect.Effect<{
    readonly id: string;
    readonly credential: string;
    readonly label?: string;
    readonly expiresAt: DateTime.Utc;
  }>;
}) {
  const testApi = await installEnvironmentHttpTest({
    ...(input.session ? { session: () => Effect.succeed(input.session!()) } : {}),
    ...(input.browserSession
      ? { browserSession: (payload) => input.browserSession!(payload.credential) }
      : {}),
    ...(input.pairingCredential
      ? { pairingCredential: (payload) => input.pairingCredential!(payload) }
      : {}),
  });
  disposeHttpTest = testApi.dispose;
  return testApi;
}

describe("resolveInitialServerAuthGateState", () => {
  beforeEach(() => {
    vi.restoreAllMocks();
    vi.useRealTimers();
    installTestBrowser("http://localhost/");
  });

  afterEach(async () => {
    await disposeHttpTest?.();
    disposeHttpTest = undefined;
    const { __resetServerAuthBootstrapForTests } = await import("./environments/primary");
    __resetServerAuthBootstrapForTests();
    __setPrimaryHttpRunnerForTests();
    vi.unstubAllEnvs();
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it("reuses an in-flight silent bootstrap attempt", async () => {
    const nextSession = sequence(
      unauthenticatedSession(DESKTOP_AUTH),
      authenticatedSession(DESKTOP_AUTH),
    );
    const testApi = await installAuthApi({
      session: nextSession,
      browserSession: () => Effect.succeed(browserSession(["orchestration:read", "access:write"])),
    });

    const testWindow = installTestBrowser("http://localhost/");
    testWindow.desktopBridge = {
      getLocalEnvironmentBootstraps: () => [
        {
          id: "primary",
          label: "Windows",
          httpBaseUrl: "http://localhost:3773",
          wsBaseUrl: "ws://localhost:3773",
          bootstrapToken: "desktop-bootstrap-token",
        },
      ],
    } as unknown as DesktopBridge;

    const { resolveInitialServerAuthGateState } = await import("./environments/primary");

    await Promise.all([resolveInitialServerAuthGateState(), resolveInitialServerAuthGateState()]);

    expect(testApi.calls.session).toBe(2);
    expect(testApi.calls.browserSession).toEqual([{ credential: "desktop-bootstrap-token" }]);
  });

  it("uses https urls when the primary environment uses wss", async () => {
    await installAuthApi({ session: () => unauthenticatedSession(LOOPBACK_AUTH) });
    vi.stubEnv("VITE_HTTP_URL", "https://remote.example.com");
    vi.stubEnv("VITE_WS_URL", "wss://remote.example.com");

    const { resolveInitialServerAuthGateState, resolvePrimaryEnvironmentHttpUrl } =
      await import("./environments/primary");

    await expect(resolveInitialServerAuthGateState()).resolves.toEqual({
      status: "requires-auth",
      auth: LOOPBACK_AUTH,
    });
    expect(resolvePrimaryEnvironmentHttpUrl("/api/auth/session")).toBe(
      "https://remote.example.com/api/auth/session",
    );
  });

  it("uses the current origin as an auth proxy base for local dev environments", async () => {
    await installAuthApi({ session: () => unauthenticatedSession(LOOPBACK_AUTH) });
    installTestBrowser("http://localhost:5735/");

    const { resolveInitialServerAuthGateState, resolvePrimaryEnvironmentHttpUrl } =
      await import("./environments/primary");

    await expect(resolveInitialServerAuthGateState()).resolves.toEqual({
      status: "requires-auth",
      auth: LOOPBACK_AUTH,
    });
    expect(resolvePrimaryEnvironmentHttpUrl("/api/auth/session")).toBe(
      "http://localhost:5735/api/auth/session",
    );
  });

  it("uses the vite proxy for desktop-managed loopback auth requests during local dev", async () => {
    await installAuthApi({ session: () => unauthenticatedSession(DESKTOP_AUTH) });
    vi.stubEnv("VITE_DEV_SERVER_URL", "http://127.0.0.1:5733");

    const testWindow = installTestBrowser("http://127.0.0.1:5733/");
    testWindow.desktopBridge = {
      getLocalEnvironmentBootstraps: () => [
        {
          id: "primary",
          label: "Windows",
          httpBaseUrl: "http://127.0.0.1:3773",
          wsBaseUrl: "ws://127.0.0.1:3773",
        },
      ],
    } as unknown as DesktopBridge;

    const { resolveInitialServerAuthGateState, resolvePrimaryEnvironmentHttpUrl } =
      await import("./environments/primary");

    await expect(resolveInitialServerAuthGateState()).resolves.toEqual({
      status: "requires-auth",
      auth: DESKTOP_AUTH,
    });
    expect(resolvePrimaryEnvironmentHttpUrl("/api/auth/session")).toBe(
      "http://127.0.0.1:5733/api/auth/session",
    );
  });

  // laplus has no `window.desktopBridge` — its shell and its server are one
  // process, so there is no preload to hand a token across and the window is
  // opened at `/#token=…` instead. Nothing read that fragment outside the
  // `/pair` route, so the gate found no credential, never opened a session, and
  // every socket upgrade was refused for presenting nothing.
  it("bootstraps from the url fragment when there is no desktop bridge", async () => {
    const testApi = await installAuthApi({
      session: sequence(unauthenticatedSession(LOOPBACK_AUTH), authenticatedSession(LOOPBACK_AUTH)),
      browserSession: () => Effect.succeed(browserSession(["orchestration:read", "access:write"])),
    });

    const testWindow = installTestBrowser("http://127.0.0.1:4773/#token=BOOT2345WXYZ");

    const { resolveInitialServerAuthGateState } = await import("./environments/primary");

    await expect(resolveInitialServerAuthGateState()).resolves.toEqual({
      status: "authenticated",
    });
    expect(testApi.calls.browserSession).toEqual([{ credential: "BOOT2345WXYZ" }]);
    // Peeked, not taken: the boot grant is re-usable precisely so that a reload
    // can read it again, and spending the address bar's only copy would make F5
    // the thing that locks the window out.
    expect(testWindow.location.hash).toBe("#token=BOOT2345WXYZ");
  });

  // `PairingRouteSurface` reads the same fragment and auto-submits it, and a
  // phone's pairing code is single-use — so a gate that spent it first would
  // leave that screen submitting a code the server had already consumed.
  it("leaves the fragment to the pairing route when that is the route being opened", async () => {
    const testApi = await installAuthApi({
      session: () => unauthenticatedSession(LOOPBACK_AUTH),
    });

    installTestBrowser("http://127.0.0.1:4773/pair#token=PHONE2345WXY");

    const { resolveInitialServerAuthGateState } = await import("./environments/primary");

    await expect(resolveInitialServerAuthGateState()).resolves.toEqual({
      status: "requires-auth",
      auth: LOOPBACK_AUTH,
    });
    expect(testApi.calls.browserSession).toEqual([]);
  });

  it("returns a requires-auth state instead of throwing when no bootstrap credential exists", async () => {
    await installAuthApi({ session: () => unauthenticatedSession(LOOPBACK_AUTH) });
    const { resolveInitialServerAuthGateState } = await import("./environments/primary");

    await expect(resolveInitialServerAuthGateState()).resolves.toEqual({
      status: "requires-auth",
      auth: LOOPBACK_AUTH,
    });
  });

  it("retries transient auth session bootstrap failures after restart", async () => {
    vi.useFakeTimers();
    let attempts = 0;
    const request = HttpClientRequest.get("http://localhost/api/auth/session");
    const response = HttpClientResponse.fromWeb(
      request,
      new Response("Bad Gateway", { status: 502 }),
    );
    const runner: PrimaryHttpEffectRunner = async <A>() => {
      attempts += 1;
      if (attempts < 4) {
        throw new HttpClientError.HttpClientError({
          reason: new HttpClientError.StatusCodeError({ request, response }),
        });
      }
      return unauthenticatedSession(LOOPBACK_AUTH) as A;
    };
    __setPrimaryHttpRunnerForTests(runner);

    const { resolveInitialServerAuthGateState } = await import("./environments/primary");

    const gateStatePromise = resolveInitialServerAuthGateState();
    await vi.advanceTimersByTimeAsync(2_000);

    await expect(gateStatePromise).resolves.toEqual({
      status: "requires-auth",
      auth: LOOPBACK_AUTH,
    });
    expect(attempts).toBe(4);
  });

  it("takes a pairing token from the location hash and strips it immediately", async () => {
    const testWindow = installTestBrowser("http://localhost/#token=pairing-token");
    const { takePairingTokenFromUrl } = await import("./environments/primary");

    expect(takePairingTokenFromUrl()).toBe("pairing-token");
    expect(testWindow.location.hash).toBe("");
    expect(testWindow.location.searchParams.get("token")).toBeNull();
  });

  it("accepts query-string pairing tokens as a backward-compatible fallback", async () => {
    const testWindow = installTestBrowser("http://localhost/?token=pairing-token");
    const { takePairingTokenFromUrl } = await import("./environments/primary");

    expect(takePairingTokenFromUrl()).toBe("pairing-token");
    expect(testWindow.location.searchParams.get("token")).toBeNull();
  });

  it("allows manual token submission after the initial auth check requires pairing", async () => {
    const nextSession = sequence(
      unauthenticatedSession(LOOPBACK_AUTH),
      authenticatedSession(LOOPBACK_AUTH),
    );
    const testApi = await installAuthApi({
      session: nextSession,
      browserSession: () => Effect.succeed(browserSession(["orchestration:read"])),
    });
    const { resolveInitialServerAuthGateState, submitServerAuthCredential } =
      await import("./environments/primary");

    await expect(resolveInitialServerAuthGateState()).resolves.toEqual({
      status: "requires-auth",
      auth: LOOPBACK_AUTH,
    });
    await expect(submitServerAuthCredential("retry-token")).resolves.toBeUndefined();
    await expect(resolveInitialServerAuthGateState()).resolves.toEqual({
      status: "authenticated",
    });
    expect(testApi.calls.browserSession).toEqual([{ credential: "retry-token" }]);
    expect(testApi.calls.session).toBe(2);
  });

  it("rejects a blank pairing token with a structured validation error", async () => {
    const { PrimaryEnvironmentPairingCredentialRequiredError, submitServerAuthCredential } =
      await import("./environments/primary/auth");

    const error = await submitServerAuthCredential("   ").then(
      () => null,
      (failure: unknown) => failure,
    );

    expect(error).toBeInstanceOf(PrimaryEnvironmentPairingCredentialRequiredError);
    expect(error).toMatchObject({
      _tag: "PrimaryEnvironmentPairingCredentialRequiredError",
      providedLength: 3,
      message: "Enter a pairing token to continue.",
    });
  });

  it("surfaces a friendly error message when an invalid pairing token is submitted", async () => {
    const cause = new EnvironmentAuthInvalidError({
      code: "auth_invalid",
      reason: "invalid_credential",
      traceId: "trace-invalid-credential",
    });
    const testApi = await installAuthApi({
      browserSession: () => Effect.fail(cause),
    });

    const { isPrimaryEnvironmentPairingCredentialRejectedError, submitServerAuthCredential } =
      await import("./environments/primary");

    const error = await submitServerAuthCredential("bad-token").then(
      () => null,
      (failure: unknown) => failure,
    );
    expect(error).toMatchObject({
      _tag: "PrimaryEnvironmentPairingCredentialRejectedError",
      providedLength: 9,
      message: "Invalid pairing token. Check the token and try again.",
    });
    expect(isPrimaryEnvironmentPairingCredentialRejectedError(error)).toBe(true);
    if (!isPrimaryEnvironmentPairingCredentialRejectedError(error)) {
      throw new Error("Expected a structured rejected pairing credential error.");
    }
    expect(error.cause).toMatchObject({
      _tag: "EnvironmentAuthInvalidError",
      code: "auth_invalid",
      reason: "invalid_credential",
      traceId: "trace-invalid-credential",
    });
    expect(testApi.calls.browserSession).toEqual([{ credential: "bad-token" }]);
  });

  it("derives primary request messages from structural request context", async () => {
    const cause = new Error("private transport detail");
    const { PrimaryEnvironmentRequestError } = await import("./environments/primary");
    const error = PrimaryEnvironmentRequestError.fromCause({
      operation: "list-pairing-links",
      cause,
    });

    expect(error.status).toBe(500);
    expect(error.cause).toBe(cause);
    expect(error.message).toBe(
      "Primary environment request failed during list-pairing-links (HTTP 500).",
    );
    expect(error.message).not.toContain(cause.message);
  });

  it("waits for the authenticated session to become observable after silent desktop bootstrap", async () => {
    vi.useFakeTimers();
    const nextSession = sequence(
      unauthenticatedSession(DESKTOP_AUTH),
      unauthenticatedSession(DESKTOP_AUTH),
      authenticatedSession(DESKTOP_AUTH),
    );
    const testApi = await installAuthApi({
      session: nextSession,
      browserSession: () => Effect.succeed(browserSession(["orchestration:read", "access:write"])),
    });

    const testWindow = installTestBrowser("http://localhost/");
    testWindow.desktopBridge = {
      getLocalEnvironmentBootstraps: () => [
        {
          id: "primary",
          label: "Windows",
          httpBaseUrl: "http://localhost:3773",
          wsBaseUrl: "ws://localhost:3773",
          bootstrapToken: "desktop-bootstrap-token",
        },
      ],
    } as unknown as DesktopBridge;

    const { resolveInitialServerAuthGateState } = await import("./environments/primary");

    const gateStatePromise = resolveInitialServerAuthGateState();
    await vi.advanceTimersByTimeAsync(100);

    await expect(gateStatePromise).resolves.toEqual({ status: "authenticated" });
    expect(testApi.calls.session).toBe(3);
  });

  it("preserves the timeout message when a bootstrapped session never becomes observable", async () => {
    vi.useFakeTimers();
    const testApi = await installAuthApi({
      session: () => unauthenticatedSession(DESKTOP_AUTH),
      browserSession: () => Effect.succeed(browserSession(["orchestration:read", "access:write"])),
    });

    installDesktopBootstrap();

    const { resolveInitialServerAuthGateState } = await import("./environments/primary");

    const gateStatePromise = resolveInitialServerAuthGateState();
    await vi.advanceTimersByTimeAsync(2_000);

    await expect(gateStatePromise).resolves.toEqual({
      status: "requires-auth",
      auth: DESKTOP_AUTH,
      errorMessage: "Timed out waiting for authenticated session after bootstrap.",
    });
    expect(testApi.calls.browserSession).toEqual([{ credential: "desktop-bootstrap-token" }]);
  });

  it("memoizes the authenticated gate state after the first successful read", async () => {
    const testApi = await installAuthApi({
      session: sequence(authenticatedSession(LOOPBACK_AUTH), unauthenticatedSession(LOOPBACK_AUTH)),
    });
    const { resolveInitialServerAuthGateState } = await import("./environments/primary");

    await expect(resolveInitialServerAuthGateState()).resolves.toEqual({
      status: "authenticated",
    });
    await expect(resolveInitialServerAuthGateState()).resolves.toEqual({
      status: "authenticated",
    });
    expect(testApi.calls.session).toBe(1);
  });

  it("creates a pairing credential from the authenticated auth endpoint", async () => {
    const testApi = await installAuthApi({
      pairingCredential: (payload) =>
        Effect.succeed({
          id: "pairing-link-1",
          credential: "pairing-token",
          ...(payload.label === undefined ? {} : { label: payload.label }),
          expiresAt: SESSION_EXPIRES_AT,
        }),
    });
    const { createServerPairingCredential } = await import("./environments/primary");

    const credential = await createServerPairingCredential({
      label: "Julius iPhone",
      scopes: ["orchestration:read"],
    });
    expect(credential).toMatchObject({
      id: "pairing-link-1",
      credential: "pairing-token",
      label: "Julius iPhone",
    });
    expect(DateTime.formatIso(credential.expiresAt)).toBe("2026-04-05T00:00:00.000Z");
    expect(testApi.calls.pairingCredential).toEqual([
      { label: "Julius iPhone", scopes: ["orchestration:read"] },
    ]);
  });

  it("routes external tunnel administration through the selected primary environment client", async () => {
    const snapshot = {
      configured: false,
      httpsOrigin: null,
      wssOrigin: null,
      ownership: "external",
      deletableAtCloudflare: false,
      cleanup: INTACT_CLEANUP,
      health: { connector: "external", https: "unknown", webSocket: "unknown" },
      verificationState: "unconfigured",
      failureKind: null,
      failureMessage: null,
      lastAttemptAt: null,
      lastVerifiedAt: null,
      advertisedEndpoint: null,
    } as const;
    const testApi = await installEnvironmentHttpTest({
      externalTunnel: () => Effect.succeed(snapshot),
      registerExternalTunnel: () =>
        Effect.succeed({
          ...snapshot,
          configured: true,
          httpsOrigin: "https://laplus.example.com",
          wssOrigin: "wss://laplus.example.com",
          verificationState: "pending",
        }),
      testExternalTunnel: () => Effect.succeed(snapshot),
      forgetExternalTunnel: () => Effect.succeed(snapshot),
    });
    disposeHttpTest = testApi.dispose;
    const {
      forgetExternalTunnelEndpoint,
      readExternalTunnelEndpoint,
      registerExternalTunnelEndpoint,
      testExternalTunnelEndpoint,
    } = await import("./environments/primary");

    await readExternalTunnelEndpoint();
    await registerExternalTunnelEndpoint("laplus.example.com");
    await testExternalTunnelEndpoint();
    await forgetExternalTunnelEndpoint();

    expect(testApi.calls.externalTunnel).toBe(1);
    expect(testApi.calls.registerExternalTunnel).toEqual([{ hostname: "laplus.example.com" }]);
    expect(testApi.calls.testExternalTunnel).toBe(1);
    expect(testApi.calls.forgetExternalTunnel).toBe(1);
  });

  it("routes managed connector discovery and lifecycle commands through the access client", async () => {
    const snapshot = {
      configured: false,
      ownership: "laplus",
      tunnelOwnership: "external",
      deletableAtCloudflare: false,
      desiredState: "stopped",
      connectorState: "stopped",
      readiness: null,
      httpsOrigin: null,
      executablePath: null,
      detectedVersion: null,
      metricsOrigin: null,
      failureMessage: null,
      restartCount: 0,
      logs: [],
      verificationState: "unconfigured",
      failureKind: null,
      publicFailureMessage: null,
      lastVerifiedAt: null,
    } as const;
    const testApi = await installEnvironmentHttpTest({
      cloudflaredExecutables: () => Effect.succeed({ executables: [] }),
      managedCloudflareConnector: () => Effect.succeed(snapshot),
      configureManagedCloudflareConnector: () => Effect.succeed(snapshot),
      startManagedCloudflareConnector: () => Effect.succeed(snapshot),
      stopManagedCloudflareConnector: () => Effect.succeed(snapshot),
      retryManagedCloudflareConnector: () => Effect.succeed(snapshot),
    });
    disposeHttpTest = testApi.dispose;
    const {
      configureManagedCloudflareConnector,
      discoverCloudflaredExecutables,
      readManagedCloudflareConnector,
      retryManagedCloudflareConnector,
      startManagedCloudflareConnector,
      stopManagedCloudflareConnector,
    } = await import("./environments/primary");

    await discoverCloudflaredExecutables();
    await readManagedCloudflareConnector();
    await configureManagedCloudflareConnector({
      hostname: "laplus.example.com",
      executablePath: "/usr/bin/cloudflared",
      connectorToken: "private-token",
    });
    await startManagedCloudflareConnector();
    await stopManagedCloudflareConnector();
    await retryManagedCloudflareConnector();

    expect(testApi.calls.cloudflaredExecutables).toBe(1);
    expect(testApi.calls.configureManagedCloudflareConnector).toEqual([
      {
        hostname: "laplus.example.com",
        executablePath: "/usr/bin/cloudflared",
        connectorToken: "private-token",
      },
    ]);
    expect(testApi.calls.startManagedCloudflareConnector).toBe(1);
    expect(testApi.calls.stopManagedCloudflareConnector).toBe(1);
    expect(testApi.calls.retryManagedCloudflareConnector).toBe(1);
  });
});
