// @vitest-environment happy-dom
/**
 * The Cloudflare wizard, driven rather than rendered.
 *
 * **These fire handlers.** The eleven Cloudflare tests that came before this
 * file are `renderToStaticMarkup` plus `toContain`, so every checkbox that said
 * "the wizard does X" was backed by a test that proved a string appeared. What
 * is asserted here is what the component *did*: which request it made, with
 * which payload, and which screen the server's answer moved it to. Requests go
 * through the real `PrimaryEnvironmentHttpClient` against the contract's own
 * handlers (`test/environmentHttpTest.ts`), so a payload the contract would
 * reject fails here rather than in a browser.
 */
import {
  EnvironmentPublicExposurePreconditionError,
  EnvironmentScopeRequiredError,
} from "@t3tools/contracts";
import type {
  CloudflareAccountSnapshot,
  CloudflaredExecutableDiscovery,
  ExternalTunnelEndpointSnapshot,
  ManagedCloudflareConnectorSnapshot,
} from "@t3tools/contracts";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import * as DateTime from "effect/DateTime";
import * as Effect from "effect/Effect";
import { afterEach, describe, expect, it } from "vite-plus/test";

import { installEnvironmentHttpTest } from "../../../test/environmentHttpTest";

// happy-dom has no Web Animations API, and `@base-ui/react`'s scroll area asks
// every viewport for its animations on a timer. Stubbing it keeps the dialog
// under test rather than the DOM implementation it renders into.
if (typeof Element.prototype.getAnimations !== "function") {
  Element.prototype.getAnimations = () => [];
}

import { CloudflareTunnelSettingsRow } from "./ConnectionsSettings";

const CERTIFICATE_WARNING =
  "The Cloudflare account certificate can create, list, route, and delete every tunnel in your account, and stays valid for years. laplus uses it where cloudflared put it and never copies, moves, replaces, or deletes it.";

const unconfiguredExternal: ExternalTunnelEndpointSnapshot = {
  configured: false,
  httpsOrigin: null,
  wssOrigin: null,
  ownership: "external",
  deletableAtCloudflare: false,
  health: { connector: "external", https: "unknown", webSocket: "unknown" },
  verificationState: "unconfigured",
  failureKind: null,
  failureMessage: null,
  lastAttemptAt: null,
  lastVerifiedAt: null,
  advertisedEndpoint: null,
};

const verifiedExternal: ExternalTunnelEndpointSnapshot = {
  ...unconfiguredExternal,
  configured: true,
  httpsOrigin: "https://laplus.example.com",
  wssOrigin: "wss://laplus.example.com",
  health: { connector: "external", https: "healthy", webSocket: "healthy" },
  verificationState: "verified",
  lastAttemptAt: "2026-08-03T09:00:00.000Z",
  lastVerifiedAt: "2026-08-03T09:00:00.000Z",
};

const unconfiguredConnector: ManagedCloudflareConnectorSnapshot = {
  configured: false,
  ownership: "laplus",
  tunnelOwnership: "external",
  deletableAtCloudflare: false,
  desiredState: "stopped",
  connectorState: "stopped",
  readiness: null,
  httpsOrigin: null,
  // The server answers both of these before anything is configured, because a
  // dedication confirmation has to show where the public hostname would go and
  // a creation confirmation has to show where the run credential will be kept.
  loopbackOrigin: "http://127.0.0.1:4773",
  credentialPath: "/data/laplus/cloudflare/tunnel.json",
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
};

const readyConnector: ManagedCloudflareConnectorSnapshot = {
  ...unconfiguredConnector,
  configured: true,
  desiredState: "running",
  connectorState: "ready",
  readiness: true,
  httpsOrigin: "https://laplus.example.com",
  loopbackOrigin: "http://127.0.0.1:4773",
  executablePath: "/usr/bin/cloudflared",
  detectedVersion: "2026.7.3",
  metricsOrigin: "http://127.0.0.1:12345",
  verificationState: "verified",
  lastVerifiedAt: "2026-08-03T09:00:00.000Z",
};

const discovery: CloudflaredExecutableDiscovery = {
  executables: [
    {
      path: "/usr/bin/cloudflared",
      source: "system",
      version: "2026.7.3",
      compatibility: "compatible",
      selected: true,
      failureMessage: null,
    },
    {
      path: "/data/laplus/cloudflare/tools/cloudflared-2026.7.4",
      source: "app-managed",
      version: "2026.7.4",
      compatibility: "compatible",
      selected: false,
      failureMessage: null,
    },
  ],
};

const signedOut: CloudflareAccountSnapshot = {
  certificateDetected: false,
  certificatePath: "/home/dev/.cloudflared/cert.pem",
  certificateConsentedAt: null,
  certificateWarning: CERTIFICATE_WARNING,
  loginState: "not-started",
  authorizationUrl: null,
  failureMessage: null,
  tunnels: [],
  listedAt: null,
  selection: null,
  step: "sign-in",
  unfinishedCreation: null,
};

const activeTunnel = {
  id: "11111111-1111-1111-1111-111111111111",
  name: "already-running",
  createdAt: "2026-01-01T00:00:00Z",
  connectionCount: 2,
  activity: "active",
  classification: "external",
} as const;

const inactiveTunnel = {
  id: "22222222-2222-2222-2222-222222222222",
  name: "spare",
  createdAt: "2026-02-02T00:00:00Z",
  connectionCount: 0,
  activity: "inactive",
  classification: "adoptable",
} as const;

const listed: CloudflareAccountSnapshot = {
  ...signedOut,
  certificateDetected: true,
  certificateConsentedAt: "2026-08-03T09:00:00.000Z",
  loginState: "complete",
  tunnels: [activeTunnel, inactiveTunnel],
  listedAt: "2026-08-03T09:00:01.000Z",
  step: "choose-tunnel",
  unfinishedCreation: null,
};

type Scenario = Parameters<typeof installEnvironmentHttpTest>[0];

/** Everything the row reads on mount, so a test only states what it changes. */
async function mount(
  overrides: Scenario & {
    readonly external?: ExternalTunnelEndpointSnapshot;
    readonly connector?: ManagedCloudflareConnectorSnapshot;
    readonly account?: CloudflareAccountSnapshot;
    readonly canWrite?: boolean;
  } = {},
) {
  const { external, connector, account, canWrite, ...scenario } = overrides;
  const testApi = await installEnvironmentHttpTest({
    externalTunnel: () => Effect.succeed(external ?? unconfiguredExternal),
    managedCloudflareConnector: () => Effect.succeed(connector ?? unconfiguredConnector),
    cloudflaredExecutables: () => Effect.succeed(discovery),
    cloudflareAccount: () => Effect.succeed(account ?? signedOut),
    ...scenario,
  });
  render(<CloudflareTunnelSettingsRow canWrite={canWrite ?? true} />);
  return testApi;
}

/** The dialog is a modal; every step below starts by opening it. */
async function openWizard(user: ReturnType<typeof userEvent.setup>) {
  await user.click(await screen.findByRole("button", { name: /Set up|Manage/ }));
}

afterEach(() => {
  cleanup();
});

describe("ownership never leaks between the wizard's paths", () => {
  /**
   * The defect this asserts against: the external block was hidden for a
   * configured managed connector but its Register button was not, and both read
   * one hostname field. Pressing Register then registered the *laplus-managed*
   * hostname as an externally managed endpoint — one lifecycle, two owners,
   * which is exactly what ADR-0045 exists to prevent.
   */
  it("offers no external registration once laplus supervises a connector", async () => {
    const user = userEvent.setup();
    const testApi = await mount({ connector: readyConnector, external: verifiedExternal });
    try {
      await openWizard(user);

      expect(screen.queryByRole("button", { name: "Register" })).toBe(null);
      expect(screen.queryByRole("button", { name: "Update" })).toBe(null);
      expect(screen.queryByLabelText("Externally managed hostname")).toBe(null);
      // And the connector it does own is on screen instead.
      expect(screen.getByLabelText("Laplus-managed Cloudflare connector")).toBeTruthy();
      expect(testApi.calls.registerExternalTunnel).toEqual([]);
    } finally {
      await testApi.dispose();
    }
  });

  /**
   * The second defect: the QR block lived inside the `!managed?.configured`
   * branch while the button that fills it did not, so pairing from a
   * laplus-managed connector produced a credential nothing rendered.
   */
  it("shows the pairing link it just minted, on a managed connector too", async () => {
    const user = userEvent.setup();
    const testApi = await mount({
      connector: readyConnector,
      external: verifiedExternal,
      pairingCredential: () =>
        Effect.succeed({
          id: "pairing-link",
          credential: "pairing-credential",
          expiresAt: DateTime.makeUnsafe("2026-08-03T09:05:00.000Z"),
        }),
    });
    try {
      await openWizard(user);
      await user.click(await screen.findByRole("button", { name: "Pair device" }));

      const pairing = await screen.findByLabelText("Cloudflare pairing URL");
      expect((pairing as HTMLTextAreaElement).value).toContain("https://laplus.example.com");
      expect(testApi.calls.pairingCredential).toHaveLength(1);
    } finally {
      await testApi.dispose();
    }
  });

  /** An external endpoint keeps its own hostname field and its own Register. */
  it("registers only the hostname typed into the external step", async () => {
    const user = userEvent.setup();
    const testApi = await mount({
      registerExternalTunnel: () => Effect.succeed(verifiedExternal),
    });
    try {
      await openWizard(user);
      await user.click(await screen.findByText("Register a hostname someone else runs"));
      await user.type(screen.getByLabelText("External HTTPS hostname"), "laplus.example.com");
      await user.click(screen.getByRole("button", { name: "Register" }));

      await waitFor(() =>
        expect(testApi.calls.registerExternalTunnel).toEqual([{ hostname: "laplus.example.com" }]),
      );
    } finally {
      await testApi.dispose();
    }
  });
});

describe("the Cloudflare account path", () => {
  it("starts a browser sign-in with the executable the developer picked", async () => {
    const user = userEvent.setup();
    const awaiting: CloudflareAccountSnapshot = {
      ...signedOut,
      loginState: "awaiting-browser",
      authorizationUrl: "https://dash.cloudflare.com/argotunnel?callback=test",
    };
    const testApi = await mount({ beginCloudflareLogin: () => Effect.succeed(awaiting) });
    try {
      await openWizard(user);
      await user.click(await screen.findByText("Sign in to Cloudflare"));
      // The picker offers what discovery found; choose the app-managed copy.
      await user.click(screen.getByRole("radio", { name: /cloudflared-2026\.7\.4/ }));
      await user.click(screen.getByRole("button", { name: "Sign in to Cloudflare" }));

      await waitFor(() =>
        expect(testApi.calls.beginCloudflareLogin).toEqual([
          { executablePath: "/data/laplus/cloudflare/tools/cloudflared-2026.7.4" },
        ]),
      );
      // Shown rather than opened: this dialog can be a phone looking at a
      // server somewhere else.
      const url = await screen.findByLabelText("Cloudflare authorization URL");
      expect((url as HTMLTextAreaElement).value).toBe(awaiting.authorizationUrl);
    } finally {
      await testApi.dispose();
    }
  });

  it("lets a sign-in be cancelled, and stays resumable afterwards", async () => {
    const user = userEvent.setup();
    const awaiting: CloudflareAccountSnapshot = {
      ...signedOut,
      loginState: "awaiting-browser",
      authorizationUrl: "https://dash.cloudflare.com/argotunnel?callback=test",
    };
    const testApi = await mount({
      account: awaiting,
      cancelCloudflareLogin: () =>
        Effect.succeed({
          ...signedOut,
          loginState: "cancelled",
          failureMessage: "Cloudflare authorization was cancelled.",
        }),
    });
    try {
      await openWizard(user);
      await user.click(await screen.findByRole("button", { name: "Cancel sign-in" }));

      await waitFor(() => expect(testApi.calls.cancelCloudflareLogin).toBe(1));
      expect(
        await screen.findAllByText("Cloudflare authorization was cancelled."),
      ).not.toHaveLength(0);
      // Resumable: the step that failed offers another attempt rather than a
      // dead end.
      expect(screen.getByRole("button", { name: "Try again" })).toBeTruthy();
    } finally {
      await testApi.dispose();
    }
  });

  /**
   * ADR-0045's consent. The warning string has existed in
   * `cloudflare_account.rs` since ticket 04's server half and had never reached
   * a browser; this is the test that says it does.
   */
  it("states the certificate's authority and its path before using it", async () => {
    const user = userEvent.setup();
    const detected: CloudflareAccountSnapshot = {
      ...signedOut,
      certificateDetected: true,
      loginState: "complete",
      step: "consent",
      unfinishedCreation: null,
    };
    const testApi = await mount({
      account: detected,
      consentToCloudflareCertificate: () =>
        Effect.succeed({
          ...detected,
          certificateConsentedAt: "2026-08-03T09:00:00.000Z",
          step: "choose-tunnel",
          unfinishedCreation: null,
        }),
    });
    try {
      await openWizard(user);
      await user.click(await screen.findByText("Sign in to Cloudflare"));

      expect(await screen.findByText(CERTIFICATE_WARNING)).toBeTruthy();
      expect(screen.getByText("/home/dev/.cloudflared/cert.pem")).toBeTruthy();

      await user.click(screen.getByRole("button", { name: "Use this certificate" }));
      await waitFor(() =>
        expect(testApi.calls.consentToCloudflareCertificate).toEqual([{ consented: true }]),
      );
      // Consent moves the wizard on, because the server said so.
      expect(await screen.findByLabelText("Choose a tunnel")).toBeTruthy();
    } finally {
      await testApi.dispose();
    }
  });

  it("can refuse the certificate, and refusing does not use it", async () => {
    const user = userEvent.setup();
    const detected: CloudflareAccountSnapshot = {
      ...signedOut,
      certificateDetected: true,
      loginState: "complete",
      step: "consent",
      unfinishedCreation: null,
    };
    const testApi = await mount({
      account: detected,
      consentToCloudflareCertificate: () => Effect.succeed(detected),
    });
    try {
      await openWizard(user);
      await user.click(await screen.findByText("Sign in to Cloudflare"));
      await user.click(await screen.findByRole("button", { name: "Don’t use it" }));

      await waitFor(() =>
        expect(testApi.calls.consentToCloudflareCertificate).toEqual([{ consented: false }]),
      );
      expect(testApi.calls.listCloudflareTunnels).toEqual([]);
    } finally {
      await testApi.dispose();
    }
  });

  /**
   * The listing carries ids, names, timestamps and connections — and no
   * hostname. So activity is shown, the hostname is asked for, and neither is
   * inferred from the other.
   */
  it("shows structured tunnel identity and asks for the hostname it cannot know", async () => {
    const user = userEvent.setup();
    const testApi = await mount({ account: listed });
    try {
      await openWizard(user);

      expect(await screen.findByText("already-running")).toBeTruthy();
      expect(screen.getByText(activeTunnel.id)).toBeTruthy();
      expect(screen.getByText(/Active · 2 connections · externally managed/)).toBeTruthy();
      expect(screen.getByText(/Inactive · can be dedicated to laplus/)).toBeTruthy();
      // Nothing on this screen claims to know where either tunnel is reachable.
      expect(screen.queryByDisplayValue(/example\.com/)).toBe(null);
    } finally {
      await testApi.dispose();
    }
  });

  it("routes a chosen active tunnel to verification with no lifecycle action", async () => {
    const user = userEvent.setup();
    const chosen: CloudflareAccountSnapshot = {
      ...listed,
      step: "verify-hostname",
      unfinishedCreation: null,
      selection: {
        tunnelId: activeTunnel.id,
        name: activeTunnel.name,
        classification: "external",
        httpsOrigin: "https://laplus.example.com",
        adoptionConfirmed: false,
        created: false,
      },
    };
    const testApi = await mount({
      account: listed,
      selectCloudflareTunnel: () => Effect.succeed(chosen),
      externalTunnel: () => Effect.succeed(verifiedExternal),
    });
    try {
      await openWizard(user);
      await user.click(await screen.findByRole("radio", { name: /already-running/ }));
      await user.type(screen.getByLabelText("Tunnel HTTPS hostname"), "laplus.example.com");
      await user.click(screen.getByRole("button", { name: "Use this tunnel" }));

      await waitFor(() =>
        expect(testApi.calls.selectCloudflareTunnel).toEqual([
          { tunnelId: activeTunnel.id, hostname: "laplus.example.com" },
        ]),
      );
      expect(await screen.findByLabelText("Verify the tunnel hostname")).toBeTruthy();
      // No connector lifecycle control appears for a tunnel laplus does not run.
      expect(screen.queryByRole("button", { name: "Start connector" })).toBe(null);
      expect(screen.queryByRole("button", { name: "Save connector" })).toBe(null);
      expect(testApi.calls.configureManagedCloudflareConnector).toEqual([]);
    } finally {
      await testApi.dispose();
    }
  });

  it("offers an inactive tunnel for dedication without managing it yet", async () => {
    const user = userEvent.setup();
    const chosen: CloudflareAccountSnapshot = {
      ...listed,
      step: "confirm-adoption",
      unfinishedCreation: null,
      selection: {
        tunnelId: inactiveTunnel.id,
        name: inactiveTunnel.name,
        classification: "adoptable",
        httpsOrigin: "https://spare.example.com",
        adoptionConfirmed: false,
        created: false,
      },
    };
    const testApi = await mount({ account: chosen });
    try {
      await openWizard(user);

      const offer = await screen.findByLabelText("Dedicate the tunnel");
      expect(offer.textContent).toContain(inactiveTunnel.id);
      expect(offer.textContent).toContain("https://spare.example.com");
      expect(offer.textContent).toContain("Not dedicated yet");
      expect(offer.textContent).toContain("stay owned outside laplus");
      // Not laplus-managed, so no connector controls and no external claim.
      expect(screen.queryByRole("button", { name: "Save connector" })).toBe(null);
      expect(screen.queryByRole("button", { name: "Register" })).toBe(null);
    } finally {
      await testApi.dispose();
    }
  });

  /**
   * Ticket 01 and ticket 04 both promise this and neither delivered it: the
   * dialog opens where the *server* says setup stopped, not at the beginning.
   */
  it("reopens at the step the server says setup reached", async () => {
    const user = userEvent.setup();
    const testApi = await mount({ account: listed });
    try {
      await openWizard(user);

      expect(await screen.findByLabelText("Choose a tunnel")).toBeTruthy();
      expect(screen.getByText("Step 3 of 4 · Choose a tunnel")).toBeTruthy();
      expect(screen.queryByLabelText("Choose how to connect")).toBe(null);
    } finally {
      await testApi.dispose();
    }
  });

  /**
   * ADR-0047: "a denied client learns only that administrator access is
   * required and receives no Cloudflare account or configuration state." What
   * would otherwise reach the browser is the transport's own summary, naming
   * the operation and the status code.
   */
  it("tells a refused administrator what they may know, and nothing else", async () => {
    const user = userEvent.setup();
    const testApi = await mount({
      account: listed,
      listCloudflareTunnels: () =>
        Effect.fail(
          new EnvironmentScopeRequiredError({
            code: "insufficient_scope",
            requiredScope: "access:write",
            traceId: "trace-id",
          }),
        ),
    });
    try {
      await openWizard(user);
      await user.click(await screen.findByRole("button", { name: "Refresh tunnel list" }));

      await waitFor(() => expect(testApi.calls.listCloudflareTunnels).toHaveLength(1));
      expect(
        await screen.findByText("Administrator access is required to manage Cloudflare setup."),
      ).toBeTruthy();
      expect(document.body.textContent).not.toContain("403");
      expect(document.body.textContent).not.toContain("list-cloudflare-tunnels");
      // And the step it was on is still there to try again from.
      expect(screen.getByLabelText("Choose a tunnel")).toBeTruthy();
    } finally {
      await testApi.dispose();
    }
  });

  it("relists tunnels on demand, which mutates nothing at Cloudflare", async () => {
    const user = userEvent.setup();
    const testApi = await mount({
      account: listed,
      listCloudflareTunnels: () => Effect.succeed(listed),
    });
    try {
      await openWizard(user);
      await user.click(await screen.findByRole("button", { name: "Refresh tunnel list" }));
      await user.click(screen.getByRole("button", { name: "Refresh tunnel list" }));

      await waitFor(() => expect(testApi.calls.listCloudflareTunnels).toHaveLength(2));
      expect(testApi.calls.selectCloudflareTunnel).toEqual([]);
    } finally {
      await testApi.dispose();
    }
  });
});

describe("navigating between the paths", () => {
  /**
   * The control was inert for exactly the developers who needed it: with a
   * consent or a listing recorded, clearing the chosen path re-derived the same
   * step and the button did nothing.
   */
  it("goes back to the path choice from a step the server derived", async () => {
    const user = userEvent.setup();
    const testApi = await mount({ account: listed });
    try {
      await openWizard(user);
      expect(await screen.findByLabelText("Choose a tunnel")).toBeTruthy();

      await user.click(screen.getByRole("button", { name: "Change setup path" }));

      expect(await screen.findByLabelText("Choose how to connect")).toBeTruthy();
      expect(screen.queryByLabelText("Choose a tunnel")).toBe(null);

      // And answering the choice leaves it again.
      await user.click(screen.getByText("Register a hostname someone else runs"));
      expect(await screen.findByLabelText("Externally managed hostname")).toBeTruthy();
    } finally {
      await testApi.dispose();
    }
  });

  /** Nothing to sign in *with* is a reason not to offer it. */
  it("will not start a sign-in with no cloudflared to run it", async () => {
    const user = userEvent.setup();
    const testApi = await mount({
      cloudflaredExecutables: () => Effect.succeed({ executables: [] }),
    });
    try {
      await openWizard(user);
      await user.click(await screen.findByText("Sign in to Cloudflare"));

      const begin = await screen.findByRole("button", { name: "Sign in to Cloudflare" });
      expect((begin as HTMLButtonElement).disabled).toBe(true);
      await user.click(begin);
      expect(testApi.calls.beginCloudflareLogin).toEqual([]);
    } finally {
      await testApi.dispose();
    }
  });
});

describe("an administrator who cannot write", () => {
  it("sees Cloudflare state and is offered no action that would change it", async () => {
    const user = userEvent.setup();
    const testApi = await mount({ account: listed, canWrite: false });
    try {
      await openWizard(user);

      expect(await screen.findByText("already-running")).toBeTruthy();
      expect(screen.queryByRole("button", { name: "Use this tunnel" })).toBe(null);
      expect(screen.queryByRole("button", { name: "Refresh tunnel list" })).toBe(null);
      expect(screen.queryByRole("button", { name: "Register" })).toBe(null);
    } finally {
      await testApi.dispose();
    }
  });
});

describe("dedicating an inactive tunnel", () => {
  const offered: CloudflareAccountSnapshot = {
    ...listed,
    step: "confirm-adoption",
    unfinishedCreation: null,
    selection: {
      tunnelId: inactiveTunnel.id,
      name: inactiveTunnel.name,
      classification: "adoptable",
      httpsOrigin: "https://spare.example.com",
      adoptionConfirmed: false,
      created: false,
    },
  };

  const dedicated: CloudflareAccountSnapshot = {
    ...offered,
    step: "adopting",
    unfinishedCreation: null,
    selection: { ...offered.selection!, adoptionConfirmed: true },
  };

  const adoptedConnector: ManagedCloudflareConnectorSnapshot = {
    ...readyConnector,
    tunnelOwnership: "adopted",
    deletableAtCloudflare: false,
    httpsOrigin: "https://spare.example.com",
  };

  /**
   * ADR-0045 makes dedication a separate, explicit confirmation, so what it
   * confirms has to be on the screen that asks for it — including where the
   * public hostname would be routed to, which nothing else on the wire says.
   */
  it("shows the tunnel, hostname, loopback target and consequences, then dedicates", async () => {
    const user = userEvent.setup();
    const testApi = await mount({
      account: offered,
      adoptCloudflareTunnel: () => Effect.succeed(dedicated),
      managedCloudflareConnector: () => Effect.succeed(unconfiguredConnector),
    });
    try {
      await openWizard(user);

      const offer = await screen.findByLabelText("Dedicate the tunnel");
      expect(offer.textContent).toContain(inactiveTunnel.id);
      expect(offer.textContent).toContain("https://spare.example.com");
      expect(offer.textContent).toContain("http://127.0.0.1:4773");
      expect(offer.textContent).toContain("Inactive");
      expect(offer.textContent).toContain("can never be deleted from here");

      await user.click(screen.getByRole("button", { name: "Dedicate this tunnel" }));

      await waitFor(() =>
        expect(testApi.calls.adoptCloudflareTunnel).toEqual([
          { executablePath: "/usr/bin/cloudflared" },
        ]),
      );
    } finally {
      await testApi.dispose();
    }
  });

  /**
   * The activation race, as the developer sees it. The server refuses and
   * registers the hostname as somebody else's instead; the screen has to say
   * both that nothing was done and what is therefore still outstanding, rather
   * than claiming a rollback that never happened.
   */
  it("reports a tunnel that became active without claiming a rollback", async () => {
    const user = userEvent.setup();
    const testApi = await mount({
      account: offered,
      adoptCloudflareTunnel: () =>
        Effect.fail(
          new EnvironmentPublicExposurePreconditionError({
            code: "public_exposure_refused",
            reason: "tunnel-became-active",
            message:
              "A connector started serving that tunnel, so it is externally managed. laplus registered the hostname as an external tunnel endpoint instead.",
            completed: [],
            remaining: ["credential", "configuration"],
            traceId: "trace",
          }),
        ),
    });
    try {
      await openWizard(user);
      await user.click(await screen.findByRole("button", { name: "Dedicate this tunnel" }));

      const refusal = await screen.findByText(/externally managed/);
      expect(refusal.textContent).toContain("external tunnel endpoint instead");
      expect(refusal.textContent).toContain(
        "Still outstanding: the tunnel credential, writing the connector configuration.",
      );
      expect(refusal.textContent).not.toContain("Already done");
    } finally {
      await testApi.dispose();
    }
  });

  /**
   * The whole of ticket 05's last acceptance line that this half owns: an
   * adopted tunnel keeps Stop, and is never offered a Cloudflare deletion —
   * because the *server* says it is not deletable, not because a control was
   * left out of a layout.
   */
  it("keeps stop available and never offers to delete an adopted tunnel", async () => {
    const user = userEvent.setup();
    const testApi = await mount({
      account: dedicated,
      connector: adoptedConnector,
      external: {
        ...verifiedExternal,
        ownership: "adopted",
        httpsOrigin: "https://spare.example.com",
      },
      pairingCredential: () =>
        Effect.succeed({
          id: "pairing-link",
          credential: "pairing-credential",
          expiresAt: DateTime.makeUnsafe("2026-08-03T09:05:00.000Z"),
        }),
    });
    try {
      await openWizard(user);

      const panel = await screen.findByLabelText("Dedicated Cloudflare tunnel");
      expect(panel.textContent).toContain("Adopted");
      expect(panel.textContent).toContain("can never delete either");
      expect(screen.getByRole("button", { name: "Stop connector" })).toBeTruthy();
      // Neither the connector-token panel's controls nor any deletion.
      expect(screen.queryByLabelText("Connector token")).toBe(null);
      expect(screen.queryByRole("button", { name: /Delete/ })).toBe(null);
      // Nor the *external* endpoint's Forget, which removes the row and stops
      // nothing — the same treatment a laplus-run connector-token connector has
      // always had. The forget a supervised connector needs stops it and
      // removes laplus's own credential and configuration first, and that is
      // ticket 07's; this ticket's last checkbox is met only in part because of
      // it.
      expect(screen.queryByRole("button", { name: "Forget" })).toBe(null);

      // A verified adopted endpoint pairs like any other: the pairing link
      // belongs to whichever endpoint was verified, not to a setup path.
      await user.click(screen.getByRole("button", { name: "Pair device" }));
      const pairing = await screen.findByLabelText("Cloudflare pairing URL");
      expect((pairing as HTMLTextAreaElement).value).toContain("https://spare.example.com");
      expect(testApi.calls.pairingCredential).toHaveLength(1);
    } finally {
      await testApi.dispose();
    }
  });
});

describe("creating a stable tunnel", () => {
  const created: CloudflareAccountSnapshot = {
    ...listed,
    step: "creating",
    unfinishedCreation: null,
    selection: {
      tunnelId: "44444444-4444-4444-4444-444444444444",
      name: "laplus-workstation",
      classification: "adoptable",
      httpsOrigin: "https://stable.example.com",
      adoptionConfirmed: false,
      created: true,
    },
  };

  const createdConnector: ManagedCloudflareConnectorSnapshot = {
    ...readyConnector,
    tunnelOwnership: "laplus-created",
    deletableAtCloudflare: true,
    httpsOrigin: "https://stable.example.com",
  };

  /**
   * Ticket 06, checkbox 1, driven rather than rendered.
   *
   * Every line of the preview is something the developer is agreeing to, and
   * nothing may be confirmed until all of them are there — so this types both
   * answers and checks the offer before and after.
   */
  it("previews the tunnel, address, DNS change, target, credential and warning first", async () => {
    const user = userEvent.setup();
    const testApi = await mount({
      account: listed,
      createCloudflareTunnel: () => Effect.succeed(created),
      managedCloudflareConnector: () => Effect.succeed(unconfiguredConnector),
    });
    try {
      await openWizard(user);
      await user.click(await screen.findByRole("button", { name: "Create a new tunnel" }));

      const offer = await screen.findByLabelText("Create a tunnel");
      // Locally managed on this computer, said in as many words: Cloudflare's
      // own recommendation is the other kind of tunnel.
      expect(offer.textContent).toContain("locally managed on this computer");
      // Nothing may be confirmed before both answers exist. The control is
      // shown and refuses rather than hidden, so a developer can see what the
      // screen is for before they have finished filling it in.
      const before = await screen.findByRole("button", { name: "Create this tunnel" });
      expect((before as HTMLButtonElement).disabled).toBe(true);

      await user.type(screen.getByLabelText("New tunnel name"), "laplus-workstation");
      await user.type(screen.getByLabelText("New tunnel HTTPS hostname"), "Stable.Example.com");

      const previewed = await screen.findByLabelText("Create a tunnel");
      expect(previewed.textContent).toContain("laplus-workstation");
      // The address that will exist, not the text that was typed.
      expect(previewed.textContent).toContain("https://stable.example.com");
      expect(previewed.textContent).toContain("A new CNAME record for stable.example.com");
      expect(previewed.textContent).toContain("http://127.0.0.1:4773");
      expect(previewed.textContent).toContain("/data/laplus/cloudflare/tunnel.json");
      expect(previewed.textContent).toContain("reachable from the public Internet");
      expect(previewed.textContent).toContain("laplus authentication remains required");

      await user.click(screen.getByRole("button", { name: "Create this tunnel" }));

      await waitFor(() =>
        expect(testApi.calls.createCloudflareTunnel).toEqual([
          {
            executablePath: "/usr/bin/cloudflared",
            name: "laplus-workstation",
            hostname: "Stable.Example.com",
          },
        ]),
      );
    } finally {
      await testApi.dispose();
    }
  });

  /**
   * Ticket 06, checkbox 7, as the developer sees it.
   *
   * A creation that allocated a tunnel and could not route it has to say both
   * halves. The screen must not imply the tunnel was cleaned up: nothing here
   * deletes anything, and claiming otherwise is precisely what the acceptance
   * criterion forbids.
   */
  it("reports a half-finished creation as what happened and what is left", async () => {
    const user = userEvent.setup();
    const testApi = await mount({
      account: listed,
      createCloudflareTunnel: () =>
        Effect.fail(
          new EnvironmentPublicExposurePreconditionError({
            code: "public_exposure_refused",
            reason: "command-failed",
            message: "cloudflared could not route that hostname to the tunnel.",
            completed: ["tunnel-create"],
            remaining: ["dns-route", "configuration"],
            traceId: "trace",
          }),
        ),
    });
    try {
      await openWizard(user);
      await user.click(await screen.findByRole("button", { name: "Create a new tunnel" }));
      await user.type(screen.getByLabelText("New tunnel name"), "laplus-workstation");
      await user.type(screen.getByLabelText("New tunnel HTTPS hostname"), "stable.example.com");
      await user.click(screen.getByRole("button", { name: "Create this tunnel" }));

      const refusal = await screen.findByText(/could not route that hostname/);
      expect(refusal.textContent).toContain("Already done: creating the tunnel.");
      expect(refusal.textContent).toContain(
        "Still outstanding: creating the DNS route, writing the connector configuration.",
      );
      // Never a rollback that did not occur.
      expect(refusal.textContent).not.toContain("undone");
      expect(refusal.textContent).not.toContain("rolled back");
    } finally {
      await testApi.dispose();
    }
  });

  /**
   * Ticket 06, checkbox 9: the endpoint is identified as laplus-created, and
   * *only* this ownership is told it may delete anything at Cloudflare.
   *
   * The sentence is `deletableAtCloudflare` — the server's own verdict and the
   * same value ticket 07's deletion command will refuse on — so the offer and
   * the refusal cannot come apart.
   */
  it("identifies a laplus-created tunnel and is the one ownership offered a deletion", async () => {
    const user = userEvent.setup();
    const testApi = await mount({
      account: created,
      connector: createdConnector,
      external: {
        ...verifiedExternal,
        ownership: "laplus-created",
        deletableAtCloudflare: true,
        httpsOrigin: "https://stable.example.com",
      },
      pairingCredential: () =>
        Effect.succeed({
          id: "pairing-link",
          credential: "pairing-credential",
          expiresAt: DateTime.makeUnsafe("2026-08-03T09:05:00.000Z"),
        }),
    });
    try {
      await openWizard(user);

      const panel = await screen.findByLabelText("Dedicated Cloudflare tunnel");
      expect(panel.textContent).toContain("laplus-created");
      expect(panel.textContent).toContain("so it can also delete them");
      // The supervision, stop and pairing behaviour every connector has.
      expect(screen.getByRole("button", { name: "Stop connector" })).toBeTruthy();
      expect(screen.queryByLabelText("Connector token")).toBe(null);
      // Ticket 07 owns the command itself; nothing here draws one.
      expect(screen.queryByRole("button", { name: /Delete/ })).toBe(null);
      expect(screen.queryByRole("button", { name: "Forget" })).toBe(null);

      await user.click(screen.getByRole("button", { name: "Pair device" }));
      const pairing = await screen.findByLabelText("Cloudflare pairing URL");
      expect((pairing as HTMLTextAreaElement).value).toContain("https://stable.example.com");
    } finally {
      await testApi.dispose();
    }
  });
});
