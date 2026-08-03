import { describe, expect, it } from "vite-plus/test";

import {
  cloudflareRowSummary,
  cloudflareWizardState,
  formatRemoteBackendHost,
  mergeVerifiedExternalEndpoint,
  registeredExternalTunnelHostname,
  selectableCloudflaredExecutables,
  visibleNetworkAdvertisedEndpoints,
} from "./ConnectionsSettings.logic";

describe("the host a remote environment was paired with", () => {
  /**
   * Ticket 06 of the headless-Linux effort gave every laplus its own
   * environment id, and its drive found that the id is never shown: the saved
   * row renders `environment.label`, which is the machine's hostname, so two
   * data directories on one machine produce two identical rows. This is the
   * text that tells them apart, and the port is the whole reason it is the host
   * rather than the label.
   */
  it("is the host and the port, which is what distinguishes two servers on one machine", () => {
    expect(formatRemoteBackendHost("http://127.0.0.1:5774")).toBe("127.0.0.1:5774");
    expect(formatRemoteBackendHost("http://127.0.0.1:5775")).toBe("127.0.0.1:5775");
    expect(formatRemoteBackendHost("http://192.168.1.42:4773/")).toBe("192.168.1.42:4773");
  });

  /**
   * A default port is not shown, because `laplus.example.com:443` is noise on
   * the one shape where the hostname is already unique — a tunnel, which is how
   * the 2026-07-30 phone drive reached its box.
   */
  it("drops a default port", () => {
    expect(formatRemoteBackendHost("https://laplus.example.com")).toBe("laplus.example.com");
    expect(formatRemoteBackendHost("https://laplus.example.com:443")).toBe("laplus.example.com");
    expect(formatRemoteBackendHost("http://box.example.com:80")).toBe("box.example.com");
  });

  /**
   * **A stored profile is not a value this can validate.** It was written by an
   * older build, or by hand, or by a version that stored something else — and
   * the answer to any of those is to show what is there rather than to render
   * nothing, because nothing is the bug this function exists to fix.
   */
  it("shows what it was given when that is not a URL", () => {
    expect(formatRemoteBackendHost("not a url")).toBe("not a url");
    expect(formatRemoteBackendHost("  spaced.example.com  ")).toBe("spaced.example.com");
  });

  /** Nothing to say rather than an empty line under the label. */
  it("has no answer for an empty value", () => {
    expect(formatRemoteBackendHost("")).toBe(null);
    expect(formatRemoteBackendHost("   ")).toBe(null);
  });
});

describe("verified external tunnel advertisement", () => {
  const endpoint = {
    id: "cloudflare-external:https://laplus.example.com",
    label: "Cloudflare Tunnel",
    provider: { id: "cloudflare", label: "Cloudflare Tunnel", kind: "tunnel", isAddon: true },
    httpBaseUrl: "https://laplus.example.com",
    wsBaseUrl: "wss://laplus.example.com",
    reachability: "public",
    compatibility: { hostedHttpsApp: "compatible", desktopApp: "compatible" },
    source: "user",
    status: "available",
  } as const;

  it("joins the established endpoint rail only after verification", () => {
    const pending = {
      configured: true,
      httpsOrigin: endpoint.httpBaseUrl,
      wssOrigin: endpoint.wsBaseUrl,
      ownership: "external",
      health: { connector: "external", https: "unknown", webSocket: "unknown" },
      verificationState: "pending",
      failureKind: null,
      failureMessage: null,
      lastAttemptAt: null,
      lastVerifiedAt: null,
      advertisedEndpoint: null,
    } as const;
    expect(mergeVerifiedExternalEndpoint([], pending)).toEqual([]);
    expect(registeredExternalTunnelHostname(pending)).toBe("https://laplus.example.com");
    expect(
      mergeVerifiedExternalEndpoint([], {
        ...pending,
        verificationState: "verified",
        advertisedEndpoint: endpoint,
      }),
    ).toEqual([endpoint]);
  });

  it("stays visible while the listener remains loopback-only", () => {
    expect(visibleNetworkAdvertisedEndpoints([endpoint], false)).toEqual([endpoint]);
  });
});

describe("the Cloudflare wizard step machine", () => {
  const account = {
    certificateDetected: false,
    certificatePath: "/home/dev/.cloudflared/cert.pem",
    certificateConsentedAt: null,
    certificateWarning: "The Cloudflare account certificate can create, list, route, and delete.",
    loginState: "not-started",
    authorizationUrl: null,
    failureMessage: null,
    tunnels: [],
    listedAt: null,
    selection: null,
    step: "sign-in",
  } as const;
  const active = {
    id: "11111111-1111-1111-1111-111111111111",
    name: "already-running",
    createdAt: "2026-01-01T00:00:00Z",
    connectionCount: 2,
    activity: "active",
    classification: "external",
  } as const;
  const inactive = {
    id: "22222222-2222-2222-2222-222222222222",
    name: "spare",
    createdAt: "2026-02-02T00:00:00Z",
    connectionCount: 0,
    activity: "inactive",
    classification: "adoptable",
  } as const;
  const managed = {
    configured: true,
    ownership: "laplus",
    desiredState: "running",
    connectorState: "ready",
    readiness: true,
    httpsOrigin: "https://laplus.example.com",
    executablePath: "/usr/bin/cloudflared",
    detectedVersion: "2026.7.3",
    metricsOrigin: "http://127.0.0.1:12345",
    failureMessage: null,
    restartCount: 0,
    logs: [],
    verificationState: "verified",
    failureKind: null,
    publicFailureMessage: null,
    lastVerifiedAt: "2026-08-03T09:00:00.000Z",
  } as const;
  const external = {
    configured: true,
    httpsOrigin: "https://laplus.example.com",
    wssOrigin: "wss://laplus.example.com",
    ownership: "external",
    health: { connector: "external", https: "healthy", webSocket: "healthy" },
    verificationState: "verified",
    failureKind: null,
    failureMessage: null,
    lastAttemptAt: "2026-08-03T09:00:00.000Z",
    lastVerifiedAt: "2026-08-03T09:00:00.000Z",
    advertisedEndpoint: null,
  } as const;
  const at = (input: Partial<Parameters<typeof cloudflareWizardState>[0]>) =>
    cloudflareWizardState({
      account: null,
      managed: null,
      external: null,
      chosenPath: null,
      ...input,
    });

  it("starts at the path choice and offers nothing until one is picked", () => {
    const start = at({ account });
    expect(start.step).toBe("choose-path");
    expect(start.path).toBe(null);
    expect(start.offersExternalRegistration).toBe(false);
    expect(start.canChangePath).toBe(false);
  });

  /**
   * The four account steps, walked the way a developer walks them: each
   * transition is a new *server* snapshot, because that is the only thing the
   * wizard is allowed to believe about its own progress.
   */
  it("walks the account path on the server's own step, and only on it", () => {
    const signIn = at({ account, chosenPath: "account" });
    expect(signIn.step).toBe("sign-in");
    expect(signIn.position).toEqual({ index: 1, total: 4 });

    const detected = {
      ...account,
      certificateDetected: true,
      loginState: "complete",
      step: "consent",
    } as const;
    expect(at({ account: detected, chosenPath: "account" }).step).toBe("consent");
    expect(at({ account: detected, chosenPath: "account" }).position).toEqual({
      index: 2,
      total: 4,
    });

    const consented = {
      ...detected,
      certificateConsentedAt: "2026-08-03T09:00:00.000Z",
      tunnels: [active, inactive],
      listedAt: "2026-08-03T09:00:01.000Z",
      step: "choose-tunnel",
    } as const;
    expect(at({ account: consented }).step).toBe("choose-tunnel");
    expect(at({ account: consented }).position).toEqual({ index: 3, total: 4 });
  });

  /**
   * ADR-0045: an active tunnel is somebody else's, so it lands on the external
   * verification path with no lifecycle action — and with nothing left to
   * register, because selecting it registered the endpoint server-side.
   */
  it("routes a chosen active tunnel to verification and never back to registration", () => {
    const chosenActive = at({
      account: {
        ...account,
        certificateDetected: true,
        certificateConsentedAt: "2026-08-03T09:00:00.000Z",
        loginState: "complete",
        tunnels: [active, inactive],
        listedAt: "2026-08-03T09:00:01.000Z",
        step: "verify-hostname",
        selection: {
          tunnelId: active.id,
          name: active.name,
          classification: "external",
          httpsOrigin: "https://laplus.example.com",
          adoptionConfirmed: false,
        },
      },
      external,
    });

    expect(chosenActive.step).toBe("verify-hostname");
    expect(chosenActive.path).toBe("account");
    expect(chosenActive.offersExternalRegistration).toBe(false);
    expect(chosenActive.ownsConnector).toBe(false);
  });

  /** An inactive tunnel is a candidate for dedication, and nothing more yet. */
  it("routes a chosen inactive tunnel to the dedication offer without managing it", () => {
    const chosenInactive = at({
      account: {
        ...account,
        certificateDetected: true,
        certificateConsentedAt: "2026-08-03T09:00:00.000Z",
        loginState: "complete",
        tunnels: [active, inactive],
        listedAt: "2026-08-03T09:00:01.000Z",
        step: "confirm-adoption",
        selection: {
          tunnelId: inactive.id,
          name: inactive.name,
          classification: "adoptable",
          httpsOrigin: "https://laplus.example.com",
          adoptionConfirmed: false,
        },
      },
    });

    expect(chosenInactive.step).toBe("confirm-adoption");
    expect(chosenInactive.ownsConnector).toBe(false);
    expect(chosenInactive.offersExternalRegistration).toBe(false);
  });

  /**
   * The ownership rule this machine exists to enforce. A laplus-managed
   * connector owns its hostname; registering that same hostname as an external
   * endpoint would give one lifecycle two owners, which ADR-0045 forbids.
   */
  it("never offers external registration once laplus supervises a connector", () => {
    for (const chosenPath of [null, "account", "connector-token", "external"] as const) {
      const owned = at({ account, managed, external, chosenPath });
      expect(owned.step).toBe("managed-connector");
      expect(owned.offersExternalRegistration).toBe(false);
      expect(owned.ownsConnector).toBe(true);
      expect(owned.canChangePath).toBe(false);
    }
  });

  it("offers registration on the external step, and reopens there once registered", () => {
    expect(at({ account, chosenPath: "external" }).offersExternalRegistration).toBe(true);

    const reopened = at({ account, external });
    expect(reopened.step).toBe("external-endpoint");
    expect(reopened.path).toBe("external");
  });

  /**
   * A certificate on the machine is not evidence laplus's account flow was
   * started — the server reports `complete` for any `cert.pem` so that a restart
   * resumes. Opening the dialog must not therefore drop a developer who wanted
   * to paste a hostname onto a consent screen.
   */
  it("treats a stray certificate as a first step, not as progress", () => {
    const stray = {
      ...account,
      certificateDetected: true,
      loginState: "complete",
      step: "consent",
    } as const;

    expect(at({ account: stray }).step).toBe("choose-path");
    expect(at({ account: stray, chosenPath: "account" }).step).toBe("consent");
    // A sign-in laplus itself ran ends somewhere other than "complete" when it
    // fails, and that *is* progress worth resuming.
    expect(at({ account: { ...stray, loginState: "timed-out", step: "sign-in" } }).step).toBe(
      "sign-in",
    );
  });

  /**
   * Clearing the chosen path is not the same as asking to choose again: the
   * inference below it would re-derive the step the developer was trying to
   * leave, which made the "Change setup path" control inert for exactly the
   * people who needed it.
   */
  it("goes back to the path choice even when the server has progress to report", () => {
    const engaged = {
      ...account,
      certificateDetected: true,
      certificateConsentedAt: "2026-08-03T09:00:00.000Z",
      loginState: "complete",
      tunnels: [active, inactive],
      listedAt: "2026-08-03T09:00:01.000Z",
      step: "choose-tunnel",
    } as const;

    expect(at({ account: engaged }).step).toBe("choose-tunnel");
    expect(at({ account: engaged, revisitingPathChoice: true }).step).toBe("choose-path");
    expect(at({ account, external, revisitingPathChoice: true }).step).toBe("choose-path");
    // Picking a path again wins over the revisit, which is how the developer
    // gets out of the choice they just went back to.
    expect(at({ account: engaged, revisitingPathChoice: true, chosenPath: "external" }).step).toBe(
      "external-endpoint",
    );
  });

  /** There is a process running; no navigation may pretend otherwise. */
  it("cannot be navigated away from a connector laplus is supervising", () => {
    const owned = at({ account, managed, external, revisitingPathChoice: true });
    expect(owned.step).toBe("managed-connector");
    expect(owned.canChangePath).toBe(false);
  });

  it("names the step it stopped at in the compact row", () => {
    const managedStateLabel = () => "Publicly verified";
    expect(
      cloudflareRowSummary({
        state: at({ account, chosenPath: "account" }),
        managed: null,
        external: null,
        managedStateLabel,
      }),
    ).toBe("Setup in progress · Sign in to Cloudflare");
    expect(
      cloudflareRowSummary({
        state: at({ account, managed, external }),
        managed,
        external,
        managedStateLabel,
      }),
    ).toBe("https://laplus.example.com · Publicly verified");
    expect(
      cloudflareRowSummary({
        state: at({ account }),
        managed: null,
        external: null,
        managedStateLabel,
      }),
    ).toBe("Register an externally managed HTTPS hostname.");
  });
});

describe("the cloudflared executables a developer can pick between", () => {
  const discovered = [
    { path: "/usr/bin/cloudflared", source: "system", selected: false },
    { path: "/data/laplus/cloudflared", source: "app-managed", selected: true },
  ] as const;

  it("keeps a hand-typed path selectable beside the discovered ones", () => {
    expect(selectableCloudflaredExecutables(discovered, "/opt/cloudflared")).toHaveLength(3);
    expect(selectableCloudflaredExecutables(discovered, "/usr/bin/cloudflared")).toEqual(
      discovered,
    );
    expect(selectableCloudflaredExecutables(discovered, "  ")).toEqual(discovered);
  });
});
