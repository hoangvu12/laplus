// Cloudflare Tunnel setup, driven through the real Connections wizard.
//
// **This one starts its own server**, unlike every other script here, because
// the feature it drives needs a `cloudflared` on the machine and a Cloudflare
// account certificate beside it — and pointing a probe at somebody's real
// laplus would mean pointing it at their real Cloudflare account. So it writes
// a stand-in `cloudflared` and a stand-in certificate into a scratch directory,
// starts an isolated `laplus-server` against them, and takes the boot
// credential out of that server's own startup lines rather than out of anyone's
// SQLite file.
//
//   pnpm --filter @t3tools/web build     # `laplus-server` serves no UI of its own
//   cd server && cargo build -p laplus-server
//   CHROME=/usr/bin/chromium-browser node tools/ui-driver/cloudflare-tunnel.mjs
//
// It spends nothing: no agent turn, no network, no Cloudflare account. Ticket
// 05 is the adoption path; 06 and 07 extend this file with creation and with
// stop/forget/delete.
//
// **What it asserts is server state, never a label.** The wizard reads its
// progress out of the server's own snapshots, so a screen that says "adopted"
// proves only that a string was rendered. Every verdict below comes from
// `GET /api/access/cloudflare` and `GET /api/access/cloudflare/connector`,
// fetched from inside the page so the session applies — the same shape
// `probe-thread-modes.mjs` uses for thread modes, and for the same reason.
//
// Exit code 1 if any verdict fails, 2 if the environment could not be set up
// (no binary, no browser, a server that never announced itself).
//
// To see it fail on purpose — which is the only way to know it can — break the
// server's answer, for instance by making `adopt_cloudflare_tunnel` in
// `server.rs` register `TunnelOwnership::External`. The screen looks identical
// and this exits 1.

import { spawn } from "node:child_process";
import { chmodSync, mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { launch, consoleLog, poll } from "./cdp.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const SERVER =
  process.env.LAPLUS_SERVER ?? join(HERE, "..", "..", "target", "debug", "laplus-server");
const PORT = process.argv[2] ?? "4791";
// `laplus-server` answers calls and serves nothing unless pointed at a bundle —
// only the shell embeds one. The shell needs a window, and this has to run on a
// headless box, so the bundle is handed over with `--ui`.
const BUNDLE = process.env.LAPLUS_UI ?? join(HERE, "..", "..", "..", "apps", "web", "dist");

/** The inactive tunnel in the listing below — the one adoption is offered for. */
const SPARE = "22222222-2222-2222-2222-222222222222";
const HOSTNAME = "spare.example.com";

/**
 * Everything this driver reaches into `ConnectionsSettings.tsx` for.
 *
 * Named in one place because a copy change there breaks this and the break
 * looks like a product bug — the convention `add-remote-environment.mjs`
 * introduced.
 */
const SELECTORS = {
  connectionsRoute: "/settings/connections",
  openWizard: "Set up",
  accountPath: "Sign in to Cloudflare",
  consent: "Use this certificate",
  refreshTunnels: "Refresh tunnel list",
  chooseAnother: "Choose a different tunnel",
  hostnameField: "Tunnel HTTPS hostname",
  useTunnel: "Use this tunnel",
  dedicate: "Dedicate this tunnel",
  stopConnector: "Stop connector",
};

const settle = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

let failures = 0;
const fail = (why) => {
  console.log(`  !! ${why}`);
  failures += 1;
};
const check = (what, actual, expected) => {
  if (actual === expected) {
    console.log(`  ok ${what} = ${JSON.stringify(actual)}`);
  } else {
    fail(`${what} is ${JSON.stringify(actual)}, expected ${JSON.stringify(expected)}`);
  }
};

const giveUp = (why) => {
  console.error(`cannot run: ${why}`);
  process.exit(2);
};

// --- a scratch environment: a stand-in cloudflared and a stand-in certificate ---

const scratch = mkdtempSync(join(tmpdir(), "laplus-cloudflare-"));
// On `PATH`, and named `cloudflared`, because discovery is what the wizard
// offers to choose between — a stand-in somewhere else would need a path typed
// into a field, which is not the flow a developer takes.
const bin = join(scratch, "bin");
mkdirSync(bin, { recursive: true });
const cloudflared = join(bin, "cloudflared");
const certificate = join(scratch, "cert.pem");
const trace = join(scratch, "cloudflared.trace");
const listing = join(scratch, "tunnels.json");
const data = join(scratch, "data");

/**
 * The same verbs the Rust harness's fake answers, in the same argument shapes:
 * `tunnel list --output json`, `tunnel token --cred-file` for an existing
 * tunnel's narrow run credential, and `tunnel --config … run` serving `/ready`.
 *
 * Deliberately not shared with `tests/harness/cloudflare.rs` — that one is a
 * Rust fixture compiled into the test binaries, and importing it here would
 * mean building a Rust crate to run a browser. What has to agree is the
 * *interface* cloudflared presents, and both are written against that.
 */
writeFileSync(
  cloudflared,
  `#!/usr/bin/env python3
import http.server, json, os, signal, sys
ARGS = sys.argv[1:]
TRACE = ${JSON.stringify(trace)}
CERT = ${JSON.stringify(certificate)}
LISTING = ${JSON.stringify(listing)}

if '--version' in ARGS:
    print('cloudflared version 2026.7.3')
    raise SystemExit(0)

with open(TRACE, 'a') as f:
    f.write(json.dumps(ARGS) + '\\n')

def after(flag):
    return ARGS[ARGS.index(flag) + 1] if flag in ARGS else None

if 'list' in ARGS:
    assert ARGS[1] == '--origincert' and ARGS[2] == CERT, ARGS
    # Read per invocation, so the driver can start a connector on the spare
    # tunnel between the offer and the confirmation without restarting anything.
    print(open(LISTING).read())
    raise SystemExit(0)

if 'token' in ARGS:
    assert ARGS[1] == '--origincert' and ARGS[2] == CERT, ARGS
    credentials = after('--cred-file')
    assert credentials is not None, ARGS
    with open(credentials, 'w') as f:
        json.dump({'AccountTag': 'account', 'TunnelID': ARGS[-1],
                   'TunnelSecret': 'FAKE-TUNNEL-CREDENTIAL-SECRET'}, f)
    os.chmod(credentials, 0o600)
    raise SystemExit(0)

if 'run' not in ARGS:
    raise SystemExit(2)

config = after('--config')
assert config is not None, ARGS
lines = [line.strip() for line in open(config).read().splitlines()]
assert [l for l in lines if l.startswith('tunnel:')], lines
held = [l for l in lines if l.startswith('credentials-file:')]
assert held and os.path.exists(held[0].split(':', 1)[1].strip()), lines

class Ready(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200 if self.path == '/ready' else 404)
        self.end_headers()
    def log_message(self, *args): pass

host, port = after('--metrics').rsplit(':', 1)
signal.signal(signal.SIGTERM, lambda *_: sys.exit(0))
http.server.HTTPServer((host, int(port)), Ready).serve_forever()
`,
  { mode: 0o700 },
);
chmodSync(cloudflared, 0o700);
// Detected rather than created, so the wizard opens on the consent step — the
// path ADR-0045 is strictest about, because merely finding a certificate grants
// laplus nothing.
writeFileSync(certificate, "FAKE-ACCOUNT-CERTIFICATE-SECRET");

const tunnels = (spareConnections) =>
  JSON.stringify([
    {
      id: "11111111-1111-1111-1111-111111111111",
      name: "already-running",
      created_at: "2026-01-01T00:00:00Z",
      deleted_at: null,
      connections: [{ id: "c1" }],
    },
    {
      id: SPARE,
      name: "spare",
      created_at: "2026-02-02T00:00:00Z",
      deleted_at: null,
      connections: spareConnections,
    },
  ]);
const listSpareAs = (state) =>
  writeFileSync(listing, tunnels(state === "active" ? [{ id: "somebody-else" }] : []));
listSpareAs("inactive");
mkdirSync(data, { recursive: true });

// --- an isolated server, and its own boot credential ---

const server = spawn(SERVER, ["serve", "--port", PORT, "--ui", BUNDLE], {
  env: {
    ...process.env,
    HOME: join(scratch, "home"),
    USERPROFILE: join(scratch, "home"),
    XDG_DATA_HOME: data,
    XDG_CONFIG_HOME: join(scratch, "config"),
    PATH: `${bin}:${process.env.PATH ?? ""}`,
    LOCALAPPDATA: undefined,
    APPDATA: undefined,
    TUNNEL_ORIGIN_CERT: certificate,
  },
  stdio: ["ignore", "pipe", "pipe"],
});
server.on("error", () => giveUp(`${SERVER} would not run — cargo build -p laplus-server first`));

let announced = "";
server.stdout.on("data", (chunk) => {
  announced += chunk.toString();
});
server.stderr.on("data", (chunk) => {
  announced += chunk.toString();
});

/**
 * The paired URL the server prints, fragment and all.
 *
 * `laplus-server` announces it because a browser on a fresh profile has no
 * credential and would otherwise land on `/pair` with nothing to type — the
 * README's third hour-waster. Taking it from stdout rather than from
 * `auth_pairing_links` means this driver needs no `sqlite3` and no knowledge of
 * where the profile went.
 */
const url = await poll(() => {
  const found = announced.match(/http:\/\/\S*#token=\S+/);
  return found ? found[0].replace(/[)\s]+$/, "") : null;
}, 30000);
if (!url) {
  giveUp(`the server never announced a paired URL. What it said:\n${announced.slice(0, 2000)}`);
}
const origin = new URL(url).origin;
console.log(`server ${origin}`);

const stop = () => {
  server.kill("SIGTERM");
};
process.on("exit", stop);

// --- the browser ---

let session;
try {
  session = await launch({ url });
} catch (cause) {
  stop();
  giveUp(`${cause.message} — set CHROME to a browser this machine has`);
}
const logs = consoleLog(session);

/** The server's own answer, read from inside the page so the session applies. */
const serverState = (path) =>
  session
    .evaluate(
      `
    return (async () => {
      const answer = await fetch(${JSON.stringify(path)}, { credentials: "include" });
      return JSON.stringify({ status: answer.status, body: await answer.json() });
    })();
  `,
    )
    .then((raw) => JSON.parse(raw));

/** Click the first visible control whose label or text starts with `label`. */
const press = (label) =>
  session.evaluate(`
    const found = [...document.querySelectorAll('button, [role="button"]')]
      .filter((element) => element.offsetParent !== null)
      .find((element) =>
        ((element.getAttribute("aria-label") || element.innerText || "").trim())
          .startsWith(${JSON.stringify(label)}));
    if (!found) return "NOT FOUND: " + ${JSON.stringify(label)};
    found.click();
    return "pressed";
  `);

const pressed = async (label) => {
  const answer = await poll(async () => {
    const outcome = await press(label);
    return outcome === "pressed" ? outcome : null;
  }, 15000);
  if (answer !== "pressed") fail(`could not press ${JSON.stringify(label)}`);
  await settle(700);
  return answer;
};

try {
  const loaded = await poll(
    () =>
      session
        .evaluate(`return document.readyState === "complete" ? "yes" : null;`)
        .catch(() => null),
    30000,
  );
  if (!loaded) giveUp("the page never finished loading");
  // Let the boot handshake spend the credential before navigating, or the
  // settings route bounces to pairing.
  await settle(6000);

  await session.evaluate(
    `location.assign(${JSON.stringify(SELECTORS.connectionsRoute)}); return 1;`,
  );
  const arrived = await poll(
    () =>
      session.evaluate(
        // Null-safe: `location.assign` leaves a document without a body for a
        // frame or two, and a poll that throws there never gets to retry.
        `return document.body?.innerText?.includes("Cloudflare Tunnel") ? "yes" : null;`,
      ),
    30000,
  );
  if (!arrived) {
    console.error("never reached the Connections page. What the page showed instead:");
    console.error(
      (await session.evaluate(`return document.body?.innerText ?? "";`))?.slice(0, 1500),
    );
    process.exit(2);
  }

  // --- the wizard, one step at a time ---

  await pressed(SELECTORS.openWizard);
  await pressed(SELECTORS.accountPath);
  // A detected certificate grants nothing until this is pressed, which is the
  // whole of ADR-0045's consent rule.
  await pressed(SELECTORS.consent);
  await pressed(SELECTORS.refreshTunnels);

  const listed = await serverState("/api/access/cloudflare/account");
  check("account step after listing", listed.body.step, "choose-tunnel");
  check("tunnels listed", listed.body.tunnels?.length, 2);
  check(
    "the spare tunnel is adoptable",
    listed.body.tunnels?.find((tunnel) => tunnel.id === SPARE)?.classification,
    "adoptable",
  );

  const chose = await session.evaluate(`
    const radio = [...document.querySelectorAll('input[name="cloudflare-tunnel"]')]
      .find((element) => element.value === ${JSON.stringify(SPARE)});
    if (!radio) return "NO TUNNEL RADIO";
    radio.click();
    const field = document.querySelector('[aria-label=${JSON.stringify(SELECTORS.hostnameField)}]');
    if (!field) return "NO HOSTNAME FIELD";
    // React tracks an input's value on the node, so assigning \`.value\` updates
    // the box and tells the component nothing.
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set;
    setter.call(field, ${JSON.stringify(HOSTNAME)});
    field.dispatchEvent(new Event("input", { bubbles: true }));
    return "chose";
  `);
  if (chose !== "chose") fail(chose);
  await settle(400);
  await pressed(SELECTORS.useTunnel);

  const offered = await serverState("/api/access/cloudflare/account");
  check("account step at the offer", offered.body.step, "confirm-adoption");
  // Chosen is not dedicated: nothing is managed until the confirmation below.
  check("adoption is not confirmed yet", offered.body.selection?.adoptionConfirmed, false);
  check(
    "no endpoint is recorded yet",
    (await serverState("/api/access/cloudflare")).body.configured,
    false,
  );

  // --- the activation race, from the screen that offers the dedication ---
  //
  // Somebody else's connector starts between the listing this offer was drawn
  // from and the button below. ADR-0045 makes the tunnel externally managed, so
  // the confirmation is refused and the hostname is registered as somebody
  // else's instead — and laplus must fetch no credential and configure nothing.
  listSpareAs("active");
  await pressed(SELECTORS.dedicate);

  const raced = await serverState("/api/access/cloudflare");
  check("a tunnel that became active is external", raced.body.ownership, "external");
  check("it is still verified and advertised", raced.body.httpsOrigin, `https://${HOSTNAME}`);
  check("nobody runs a connector for it", raced.body.health?.connector, "external");
  check(
    "no connector was configured",
    (await serverState("/api/access/cloudflare/connector")).body.configured,
    false,
  );
  const traced = readFileSync(trace, "utf8");
  if (traced.includes('"token"')) fail("a credential was retrieved for an active tunnel");

  // Back to inactive, and the wizard re-offers the dedication once the listing
  // is refreshed — the selection is external until something says otherwise.
  listSpareAs("inactive");
  await pressed(SELECTORS.chooseAnother);
  await pressed(SELECTORS.refreshTunnels);
  await session.evaluate(`
    const radio = [...document.querySelectorAll('input[name="cloudflare-tunnel"]')]
      .find((element) => element.value === ${JSON.stringify(SPARE)});
    if (radio) radio.click();
    return 1;
  `);
  await settle(400);
  await pressed(SELECTORS.useTunnel);
  const reoffered = await serverState("/api/access/cloudflare/account");
  check("the offer returns once the tunnel is idle again", reoffered.body.step, "confirm-adoption");

  await pressed(SELECTORS.dedicate);

  // --- the verdicts ---

  const endpoint = await poll(async () => {
    const answer = await serverState("/api/access/cloudflare");
    return answer.body.configured ? answer : null;
  }, 30000);
  if (!endpoint) {
    fail("the server never recorded an endpoint after dedication");
  } else {
    check("endpoint ownership", endpoint.body.ownership, "adopted");
    check("endpoint hostname", endpoint.body.httpsOrigin, `https://${HOSTNAME}`);
    // ADR-0045: laplus configured and runs this tunnel and allocated neither it
    // nor its DNS record, so Delete everywhere is refused rather than hidden.
    check("adopted tunnels are undeletable", endpoint.body.deletableAtCloudflare, false);
    check("laplus runs the connector", endpoint.body.health?.connector, "laplus");
  }

  const connector = await poll(async () => {
    const answer = await serverState("/api/access/cloudflare/connector");
    return answer.body.connectorState === "ready" ? answer : null;
  }, 30000);
  if (!connector) {
    const last = await serverState("/api/access/cloudflare/connector");
    fail(`the connector never became ready: ${JSON.stringify(last.body).slice(0, 400)}`);
  } else {
    check("connector tunnel ownership", connector.body.tunnelOwnership, "adopted");
    check("connector is supervised", connector.body.desiredState, "running");
    check("connector readiness", connector.body.readiness, true);
    check("no deletion is offered", connector.body.deletableAtCloudflare, false);
  }

  const account = await serverState("/api/access/cloudflare/account");
  check("wizard resumes at the dedicated step", account.body.step, "adopting");
  check("adoption is confirmed", account.body.selection?.adoptionConfirmed, true);

  // The secret cloudflared wrote must not have come back through any of them.
  const answered = JSON.stringify([endpoint?.body, connector?.body, account.body]);
  if (answered.includes("FAKE-TUNNEL-CREDENTIAL-SECRET")) {
    fail("a run credential reached a snapshot");
  }
  if (answered.includes("FAKE-ACCOUNT-CERTIFICATE-SECRET")) {
    fail("the account certificate reached a snapshot");
  }

  // Stop stays available for an adopted tunnel, and takes nothing with it.
  await pressed(SELECTORS.stopConnector);
  const stopped = await poll(async () => {
    const answer = await serverState("/api/access/cloudflare/connector");
    return answer.body.desiredState === "stopped" ? answer : null;
  }, 20000);
  if (!stopped) {
    fail("stop did not reach the server");
  } else {
    const after = await serverState("/api/access/cloudflare");
    check("stopping keeps the endpoint", after.body.httpsOrigin, `https://${HOSTNAME}`);
    check("stopping keeps the ownership", after.body.ownership, "adopted");
  }

  const errors = logs.filter((line) => /error|exception/i.test(line) && !/404|401/.test(line));
  if (errors.length) {
    console.log("\n=== CONSOLE ===");
    console.log(errors.slice(0, 6).join("\n").slice(0, 1600));
  }
} finally {
  await session.close();
  stop();
  rmSync(scratch, { recursive: true, force: true });
}

console.log(`\n=== ${failures ? `${failures} FAILURE(S)` : "OK"} ===`);
process.exit(failures ? 1 : 0);
