import { describe, expect, it } from "vite-plus/test";
import {
  EnvironmentPublicExposureRejectedError,
  EnvironmentScopeRequiredError,
} from "@t3tools/contracts";
import type { TunnelOwnership } from "@t3tools/contracts";

import { PrimaryEnvironmentRequestError } from "../../environments/primary";

import {
  cloudflareCreationPreview,
  cloudflareFailureMessage,
  cloudflareUnfinishedCreationSummary,
  cloudflareRefusalSummary,
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
      deletableAtCloudflare: false,
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
    unfinishedCreation: null,
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
    tunnelOwnership: "external",
    deletableAtCloudflare: false,
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
    deletableAtCloudflare: false,
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
      unfinishedCreation: null,
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
      unfinishedCreation: null,
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
        unfinishedCreation: null,
        selection: {
          tunnelId: active.id,
          name: active.name,
          classification: "external",
          httpsOrigin: "https://laplus.example.com",
          adoptionConfirmed: false,
          created: false,
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
        unfinishedCreation: null,
        selection: {
          tunnelId: inactive.id,
          name: inactive.name,
          classification: "adoptable",
          httpsOrigin: "https://laplus.example.com",
          adoptionConfirmed: false,
          created: false,
        },
      },
    });

    expect(chosenInactive.step).toBe("confirm-adoption");
    expect(chosenInactive.ownsConnector).toBe(false);
    expect(chosenInactive.offersExternalRegistration).toBe(false);
    // The two answers to "which tunnel" are different lengths, and saying "4 of
    // 4" here would tell a developer they were finished at the moment they were
    // being asked the only question that changes anything.
    expect(chosenInactive.position).toEqual({ index: 4, total: 5 });
  });

  /**
   * A dedicated tunnel is a connector laplus supervises, and it is *not* the
   * connector-token screen.
   *
   * That panel's controls are a hostname and a connector token, and a dedicated
   * tunnel has neither: Cloudflare does not hold its configuration, laplus does.
   * Which one this is comes from `tunnelOwnership` — the endpoint row's answer —
   * so a reopened dialog and a restarted server cannot disagree about it.
   */
  it("routes a dedicated tunnel's connector to its own screen, not the token one", () => {
    const adopted = {
      ...managed,
      tunnelOwnership: "adopted",
      deletableAtCloudflare: false,
      httpsOrigin: "https://spare.example.com",
    } as const;

    for (const chosenPath of [null, "account", "connector-token", "external"] as const) {
      const dedicated = at({ account, managed: adopted, external, chosenPath });
      expect(dedicated.step).toBe("adopting");
      expect(dedicated.path).toBe("account");
      expect(dedicated.position).toEqual({ index: 5, total: 5 });
      // laplus runs this connector, so the external endpoint's Forget — which
      // would delete the record the connector restores itself from — is not on
      // offer, and neither is registering its hostname as somebody else's.
      expect(dedicated.ownsConnector).toBe(true);
      expect(dedicated.offersExternalRegistration).toBe(false);
      expect(dedicated.canChangePath).toBe(false);
    }

    // A connector-token connector is still the token screen: its tunnel is
    // configured at Cloudflare, which is what `external` ownership means here.
    expect(at({ account, managed, external }).step).toBe("managed-connector");
  });

  /**
   * The way out of an activation race.
   *
   * When a tunnel turns out to be active the server records the hostname as
   * somebody else's and answers `verify-hostname` — truthfully, and with no way
   * back, because every branch below that is derived from a selection which is
   * now external. Changing setup path lands on the same step again. So the
   * request to be asked about tunnels again is client-held, exactly as the
   * request to be asked about paths is.
   */
  it("can be sent back to the tunnel list after a selection turned out to be external", () => {
    const external = {
      ...account,
      certificateDetected: true,
      certificateConsentedAt: "2026-08-03T09:00:00.000Z",
      loginState: "complete",
      tunnels: [active, inactive],
      listedAt: "2026-08-03T09:00:01.000Z",
      step: "verify-hostname",
      unfinishedCreation: null,
      selection: {
        tunnelId: inactive.id,
        name: inactive.name,
        classification: "external",
        httpsOrigin: "https://laplus.example.com",
        adoptionConfirmed: false,
        created: false,
      },
    } as const;

    expect(at({ account: external, chosenPath: "account" }).step).toBe("verify-hostname");
    const revisited = at({
      account: external,
      chosenPath: "account",
      revisitingTunnelChoice: true,
    });
    expect(revisited.step).toBe("choose-tunnel");
    expect(revisited.path).toBe("account");

    // It is navigation, not progress: with nothing chosen there is nothing to
    // be sent back from, and the server's own step still decides.
    expect(
      at({
        account: { ...external, selection: null, step: "choose-tunnel" },
        revisitingTunnelChoice: true,
      }).step,
    ).toBe("choose-tunnel");
    // And it never overrides a connector laplus is already supervising.
    expect(at({ account: external, managed, revisitingTunnelChoice: true }).step).toBe(
      "managed-connector",
    );
  });

  /**
   * The third fork: making a tunnel rather than picking one.
   *
   * **The screen that asks is the client's and the screen that reports is the
   * server's**, and the boundary between them is what has been written down. A
   * name and a hostname that have only been typed are not durable state, so no
   * snapshot could compute `create-tunnel`; the moment creation succeeds the
   * server answers `creating` and the client's flag stops mattering.
   */
  it("offers creation from the tunnel list and leaves it the moment the server records one", () => {
    const consented = {
      ...account,
      certificateDetected: true,
      certificateConsentedAt: "2026-08-03T09:00:00.000Z",
      loginState: "complete",
      tunnels: [active, inactive],
      listedAt: "2026-08-03T09:00:01.000Z",
      step: "choose-tunnel",
      unfinishedCreation: null,
    } as const;

    const creating = at({ account: consented, chosenPath: "account", creatingTunnel: true });
    expect(creating.step).toBe("create-tunnel");
    expect(creating.path).toBe("account");
    // Its own fork, and the same length as adoption's: there is a confirmation
    // to give and a connector to bring up after it.
    expect(creating.position).toEqual({ index: 4, total: 5 });
    expect(creating.ownsConnector).toBe(false);
    expect(creating.offersExternalRegistration).toBe(false);

    // It is navigation, not progress. Asking to create while the server is still
    // on sign-in cannot skip the two screens that establish the authority a
    // creation spends.
    expect(at({ account, chosenPath: "account", creatingTunnel: true }).step).toBe("sign-in");
    // And it never overrides a connector laplus already supervises.
    expect(at({ account: consented, managed, creatingTunnel: true }).step).toBe(
      "managed-connector",
    );
  });

  /**
   * A creation that never finished puts the developer back where they can
   * finish it.
   *
   * The client's own `creatingTunnel` flag is discarded by a reload, and the
   * `completed`/`remaining` in the refusal body live exactly as long as the
   * screen that received them. `unfinishedCreation` is read from a journal a
   * finished creation clears, so it survives both — and without it a restart
   * after a failed DNS route showed a wizard offering to create a tunnel that
   * already existed.
   */
  it("resumes onto the creation screen after a restart, with no client flag set", () => {
    const halfway = {
      ...account,
      certificateDetected: true,
      certificateConsentedAt: "2026-08-03T09:00:00.000Z",
      loginState: "complete",
      step: "choose-tunnel",
      unfinishedCreation: {
        name: "laplus-workstation",
        tunnelId: "44444444-4444-4444-4444-444444444444",
        hostname: null,
        completed: ["tunnel-create"],
        remaining: ["dns-route", "configuration"],
      },
    } as const;

    const resumed = at({ account: halfway, chosenPath: "account" });
    expect(resumed.step).toBe("create-tunnel");
    expect(resumed.position).toEqual({ index: 4, total: 5 });

    // And with no path chosen either, because a half-made Cloudflare tunnel is
    // engagement with the account path however the dialog was reopened.
    expect(at({ account: halfway }).step).toBe("create-tunnel");

    // A connector laplus is already supervising still outranks it: that one has
    // a process behind it.
    expect(at({ account: halfway, managed }).step).toBe("managed-connector");
  });

  /**
   * A laplus-created connector is its own screen, not the adopted one.
   *
   * They share a panel and differ in one sentence — only a laplus-created
   * tunnel's Cloudflare resources are laplus's to delete — and that sentence is
   * `deletableAtCloudflare`, which the server states. Two steps rather than one
   * so the wizard's header and the compact row can say which.
   */
  it("routes a laplus-created connector to the creating step and keeps it there", () => {
    const created = {
      ...managed,
      tunnelOwnership: "laplus-created",
      deletableAtCloudflare: true,
      httpsOrigin: "https://stable.example.com",
    } as const;

    for (const chosenPath of [null, "account", "connector-token", "external"] as const) {
      const owned = at({ account, managed: created, external, chosenPath, creatingTunnel: true });
      expect(owned.step).toBe("creating");
      expect(owned.path).toBe("account");
      expect(owned.position).toEqual({ index: 5, total: 5 });
      // laplus runs this connector, so the external endpoint's Forget is not on
      // offer and neither is registering its hostname as somebody else's.
      expect(owned.ownsConnector).toBe(true);
      expect(owned.offersExternalRegistration).toBe(false);
      expect(owned.canChangePath).toBe(false);
    }

    // Adopted and laplus-created stay two answers, which is the whole point.
    expect(at({ account, managed: { ...managed, tunnelOwnership: "adopted" } }).step).toBe(
      "adopting",
    );
  });

  /**
   * Ticket 06, checkbox 9: the row identifies a laplus-created tunnel, and
   * preserves that across restart.
   *
   * The word comes from `tunnelOwnership` on the connector snapshot, which is
   * read from the persisted endpoint row rather than remembered — so what the
   * row says after a restart is what it said before one.
   */
  it("says a laplus-created tunnel is laplus-created in the compact row", () => {
    const created = {
      ...managed,
      tunnelOwnership: "laplus-created",
      deletableAtCloudflare: true,
      httpsOrigin: "https://stable.example.com",
    } as const;

    expect(
      cloudflareRowSummary({
        state: at({ account, managed: created }),
        managed: created,
        external: null,
        managedStateLabel: () => "Publicly verified",
      }),
    ).toBe("https://stable.example.com · laplus-created · Publicly verified");
  });

  /** The compact row names the ownership, because that is what differs. */
  it("says an adopted tunnel is adopted in the compact row", () => {
    expect(
      cloudflareRowSummary({
        state: at({ account, managed: { ...managed, tunnelOwnership: "adopted" } }),
        managed: {
          ...managed,
          tunnelOwnership: "adopted",
          httpsOrigin: "https://spare.example.com",
        },
        external: null,
        managedStateLabel: () => "Publicly verified",
      }),
    ).toBe("https://spare.example.com · Adopted · Publicly verified");
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
      unfinishedCreation: null,
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
      unfinishedCreation: null,
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
    ).toBe("https://laplus.example.com · Externally owned · Publicly verified");
    expect(
      cloudflareRowSummary({
        state: at({ account }),
        managed: null,
        external: null,
        managedStateLabel,
      }),
    ).toBe("Register an externally managed HTTPS hostname.");
  });

  /**
   * Ticket 06 needs the row to identify a laplus-created tunnel and preserve
   * that across restart, and ticket 07 makes the same word decide whether
   * "Delete everywhere" is offered at all. A row that showed only a hostname
   * and a health word made two endpoints with opposite deletion authority read
   * identically.
   */
  it("says which ownership the endpoint has, not only how healthy it is", () => {
    const managedStateLabel = () => "Publicly verified";
    const rowFor = (ownership: TunnelOwnership) =>
      cloudflareRowSummary({
        state: at({ account, managed, external }),
        managed: { ...managed, tunnelOwnership: ownership },
        external,
        managedStateLabel,
      });

    expect(rowFor("laplus-created")).toBe(
      "https://laplus.example.com · laplus-created · Publicly verified",
    );
    expect(rowFor("adopted")).toBe("https://laplus.example.com · Adopted · Publicly verified");
    expect(rowFor("external")).toBe(
      "https://laplus.example.com · Externally owned · Publicly verified",
    );

    // An endpoint laplus does not run reads its ownership from its own record,
    // which is now three-valued rather than the literal `"external"`.
    expect(
      cloudflareRowSummary({
        state: at({ account, external }),
        managed: null,
        external: { ...external, ownership: "adopted" },
        managedStateLabel,
      }),
    ).toBe("https://laplus.example.com · Adopted · Verified");
  });
});

/**
 * A refusal that changed nothing reads as it always did; one that changed half
 * of something says which half. Tickets 06 and 07 both forbid the wizard from
 * claiming a rollback that did not occur, and this is the sentence that keeps
 * that promise.
 */
describe("what a refused Cloudflare command tells the developer", () => {
  it("carries the server's sentence alone when nothing was mutated", () => {
    expect(
      cloudflareRefusalSummary({
        message: "Sign in to Cloudflare first.",
        completed: [],
        remaining: [],
      }),
    ).toBe("Sign in to Cloudflare first.");
  });

  it("names completed and outstanding work separately", () => {
    expect(
      cloudflareRefusalSummary({
        message: "The DNS route could not be created.",
        completed: ["credential", "tunnel-create"],
        remaining: ["dns-route"],
      }),
    ).toBe(
      "The DNS route could not be created. Already done: the tunnel credential, creating the tunnel. Still outstanding: creating the DNS route.",
    );
  });

  it("names remaining remote cleanup without claiming the rest was undone", () => {
    const summary = cloudflareRefusalSummary({
      message: "The tunnel was deleted but its DNS record was not.",
      completed: ["tunnel-delete"],
      remaining: ["dns-record-delete"],
    });
    expect(summary).toContain("Already done: deleting the tunnel.");
    expect(summary).toContain("Still outstanding: deleting the DNS record.");
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

/**
 * The end of the road for Gap 4: what a refused Cloudflare command actually
 * puts on screen. The server's sentence used to be dropped at the boundary —
 * an untagged `{ message }` decoded as no declared error — so the only thing
 * that reached here was the transport's own summary, which is a sentence for
 * whoever wrote the client rather than whoever is holding the machine.
 */
describe("what a failed Cloudflare request puts on screen", () => {
  const refusal = (over: Partial<EnvironmentPublicExposureRejectedError> = {}) =>
    new EnvironmentPublicExposureRejectedError({
      code: "public_exposure_refused",
      reason: "command-failed",
      message: "cloudflared could not list the account's tunnels.",
      completed: [],
      remaining: [],
      traceId: "trace-1",
      ...over,
    });

  it("shows the server's own sentence rather than the transport's summary", () => {
    const wrapped = PrimaryEnvironmentRequestError.fromCause({
      operation: "list-cloudflare-tunnels",
      cause: refusal(),
    });

    expect(wrapped.message).toContain("Primary environment request failed");
    expect(cloudflareFailureMessage(wrapped, "The Cloudflare request failed.")).toBe(
      "cloudflared could not list the account's tunnels.",
    );
  });

  it("names the work a partial failure finished and left outstanding", () => {
    const wrapped = PrimaryEnvironmentRequestError.fromCause({
      operation: "list-cloudflare-tunnels",
      cause: refusal({
        reason: "cleanup-required",
        message: "The tunnel was deleted but its DNS record was not.",
        completed: ["tunnel-delete"],
        remaining: ["dns-record-delete"],
      }),
    });

    const shown = cloudflareFailureMessage(wrapped, "The Cloudflare request failed.");
    expect(shown).toContain("The tunnel was deleted but its DNS record was not.");
    expect(shown).toContain("Already done: deleting the tunnel.");
    expect(shown).toContain("Still outstanding: deleting the DNS record.");
  });

  /**
   * ADR-0047: a denied client learns that administrator access is required and
   * nothing about the Cloudflare account or configuration behind the refusal.
   */
  it("tells a client without the scope only that it needs one", () => {
    const denied = PrimaryEnvironmentRequestError.fromCause({
      operation: "list-cloudflare-tunnels",
      cause: new EnvironmentScopeRequiredError({
        code: "insufficient_scope",
        requiredScope: "access:write",
        traceId: "trace-2",
      }),
    });

    expect(cloudflareFailureMessage(denied, "The Cloudflare request failed.")).toBe(
      "Administrator access is required to manage Cloudflare setup.",
    );
  });

  it("falls back when the failure carries nothing worth showing", () => {
    expect(cloudflareFailureMessage(new Error("   "), "The Cloudflare request failed.")).toBe(
      "The Cloudflare request failed.",
    );
    expect(cloudflareFailureMessage({}, "The Cloudflare request failed.")).toBe(
      "The Cloudflare request failed.",
    );
  });
});

describe("the preview a tunnel creation is confirmed against", () => {
  const complete = {
    name: "  laplus-workstation  ",
    hostname: "  Laplus.Example.COM  ",
    loopbackOrigin: "http://127.0.0.1:4773",
    credentialPath: "/data/laplus/cloudflare/tunnel.json",
  };

  /**
   * Ticket 06, checkbox 1. Every one of these is something the developer is
   * being asked to agree to, and a confirmation that omits one is a
   * confirmation of an abstraction — the argument ADR-0045 already makes about
   * the account certificate.
   */
  it("names the tunnel, the exact address, the DNS change, the target and the credential", () => {
    expect(cloudflareCreationPreview(complete)).toEqual({
      name: "laplus-workstation",
      httpsOrigin: "https://laplus.example.com",
      dnsChange: "A new CNAME record for laplus.example.com routed to this tunnel",
      routesTo: "http://127.0.0.1:4773",
      credentialPath: "/data/laplus/cloudflare/tunnel.json",
    });
  });

  /**
   * The address shown is the one that will be created, not the one that was
   * typed. `normalize_hostname` on the server is still the authority and still
   * refuses with `hostname-invalid`; this only stops the preview promising a
   * hostname that differs in case from the record laplus would make.
   */
  it("shows the address that will exist rather than the text that was entered", () => {
    expect(
      cloudflareCreationPreview({ ...complete, hostname: "https://Box.example.com/" }),
    ).toEqual(expect.objectContaining({ httpsOrigin: "https://box.example.com" }));
    expect(cloudflareCreationPreview({ ...complete, hostname: "http://box.example.com" })).toEqual(
      expect.objectContaining({ httpsOrigin: "https://box.example.com" }),
    );
  });

  /**
   * One decision about whether a creation may be offered at all, rather than a
   * disabled button beside a half-drawn list.
   */
  it("has no preview, and therefore no offer, until both answers are given", () => {
    expect(cloudflareCreationPreview({ ...complete, name: "   " })).toBe(null);
    expect(cloudflareCreationPreview({ ...complete, hostname: "" })).toBe(null);
  });

  /**
   * **All five lines, or no offer.**
   *
   * The loopback target and the credential location come from the connector
   * snapshot, and they are the two the developer cannot supply themselves.
   * Substituting "somewhere private" for a path laplus has not answered with yet
   * would be precisely the abstraction this preview exists to prevent — so the
   * offer refuses for the moment before the snapshot loads rather than promising
   * something it cannot name.
   */
  it("has no offer while the connector snapshot cannot say where things go", () => {
    expect(cloudflareCreationPreview({ ...complete, loopbackOrigin: null })).toBe(null);
    expect(cloudflareCreationPreview({ ...complete, credentialPath: null })).toBe(null);
  });

  /**
   * What a resumed offer tells a developer, and what it must never imply.
   *
   * laplus removes nothing when a creation stops, so completed work is reported
   * as still done — which is exactly why pressing create again is safe. The
   * vocabulary is shared with the refusal summary, because the sentence read at
   * the moment of failure and the one read after a restart are about the same
   * journal.
   */
  it("says what an unfinished creation did and left, and never claims a rollback", () => {
    expect(
      cloudflareUnfinishedCreationSummary({
        completed: ["tunnel-create"],
        remaining: ["dns-route", "configuration"],
      }),
    ).toBe(
      "Already done: creating the tunnel. Still outstanding: creating the DNS route, writing the connector configuration.",
    );
    expect(
      cloudflareUnfinishedCreationSummary({ completed: [], remaining: ["tunnel-create"] }),
    ).toBe("Still outstanding: creating the tunnel.");
  });
});
