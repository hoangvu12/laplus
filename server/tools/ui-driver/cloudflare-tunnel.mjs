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
// 05 is the adoption path, ticket 06 the creation one, ticket 07 the three ways
// out of both — stop, forget, and delete everywhere — and ticket 02 the
// connector-token path, its supervision and its persistence.
//
// **A stand-in Cloudflare DNS API too, not only a stand-in cloudflared.**
// `cloudflared` has no `route dns delete`, so removing the record a creation made
// is a REST call with DNS authority of its own — modelling it as another CLI verb
// would let the delete path pass against a command that does not exist. The
// server is pointed at it with `LAPLUS_CLOUDFLARE_API`, which it honours only
// towards loopback.
//
// **Verification and pairing are not reachable from here**, and deliberately not
// faked: both need a hostname that genuinely resolves and a public HTTPS path
// back to this machine, which is exactly what a scratch world does not have.
// `tests/http_cloudflare_{adoption,creation}.rs` cover them against the hermetic
// verifier.
//
// **Three isolated servers, one browser.** Adoption, creation and the
// connector-token path cannot share a server: each ends with a connector laplus
// supervises, and the wizard rightly refuses to offer setup for an exposure that
// already exists. So each walkthrough gets its own scratch directory, its own
// stand-in `cloudflared`, its own port and its own boot credential, and the page
// is navigated between them the same way it was booted the first time.
//
// **The third world is where the driver stops being polite.** Ticket 02's
// checkboxes are about a connector that is *not* being asked nicely: it names
// restarts, a spent budget and a configuration that survives its own server. So
// that world writes no account certificate at all, kills the connector out from
// under the supervisor `MAX_RESTARTS` times, and stops and replaces its whole
// server against the same data directory. Stop and Start are the operator's
// buttons and reach `set_desired`; nothing a developer can press reaches
// `child_failed`, which is the only place `restartCount` moves.
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
// server's answer, for instance by making `adopt_cloudflare_tunnel` or
// `create_cloudflare_tunnel` in `server.rs` register `TunnelOwnership::External`.
// The screen looks identical and this exits 1.

import { spawn } from "node:child_process";
import { createServer } from "node:http";
import {
  chmodSync,
  mkdtempSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
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
/** What `tunnel create` allocates: a UUID, and never the name it was asked for. */
const CREATED = "44444444-4444-4444-4444-444444444444";
const NEW_NAME = "laplus-desk";
const NEW_HOSTNAME = "stable.example.com";
/** Ticket 01: a hostname laplus verifies and advertises and never operates. */
const EXTERNAL_HOSTNAME = "somebody-elses.example.com";
/**
 * Ticket 02's own path: a tunnel-specific connector token, and no account.
 *
 * A distinct literal from every other secret here, so a leak can be attributed
 * to the thing that leaked it rather than to "a secret".
 */
const CONNECTOR_TOKEN = "FAKE-CONNECTOR-TOKEN-SECRET";
const TOKEN_HOSTNAME = "connector-token.example.com";
/**
 * `MAX_RESTARTS` in `cloudflare_connector.rs`. Restated rather than imported,
 * because the driver's claim is about the number a running server enforces —
 * a copy that drifts is this driver failing, which is the correct outcome.
 */
const MAX_RESTARTS = 3;

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
  createInstead: "Create a new tunnel",
  newNameField: "New tunnel name",
  newHostnameField: "New tunnel HTTPS hostname",
  create: "Create this tunnel",
  startConnector: "Start connector",
  // Two labels, one ellipsis apart, for the same reason the delete pair below
  // is two: `3cf96ae` gave Forget a confirmation of its own, and a driver that
  // pressed the trigger and waited would wait forever. `press` matches by
  // prefix, and the trigger is gone by the time the second is looked for.
  forget: "Forget local setup…",
  confirmForget: "Forget local setup",
  // Ticket 01's path: a hostname somebody else's connector already serves.
  externalPath: "Register a hostname someone else runs",
  externalHostnameField: "External HTTPS hostname",
  register: "Register",
  changePath: "Change setup path",
  // Ticket 02's own panel: an existing cloudflared plus a tunnel-specific
  // connector token, reached without signing in to anything.
  tokenPath: "Use a tunnel connector token",
  managedHostnameField: "Managed connector hostname",
  connectorTokenField: "Connector token",
  saveConnector: "Save connector",
  retryConnector: "Retry connector",
  // Two labels, one word apart on purpose: the first opens the destructive
  // confirmation and the second is inside it. `press` matches by prefix, and the
  // trigger is gone by the time the second is looked for.
  offerDelete: "Delete everywhere\u2026",
  confirmDelete: "Delete everywhere",
  dnsTokenField: "Cloudflare DNS API token",
};

/** What the fake Cloudflare DNS API insists on being handed. */
const DNS_API_TOKEN = "FAKE-CLOUDFLARE-DNS-API-TOKEN";
const ZONE_ID = "zone-example-com";
const RECORD_ID = "record-stable";

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

/**
 * One isolated Cloudflare world: a stand-in `cloudflared` on its own `PATH`, a
 * stand-in account certificate beside it, and a data directory for the server
 * that will use them.
 *
 * A function rather than a block because adoption and creation each need their
 * own — a server that is already supervising a connector cannot be asked to set
 * one up, which is the wizard behaving correctly rather than a limitation.
 *
 * The verbs are the ones the Rust harness's fake answers, in the same argument
 * shapes: `tunnel list --output json`, `tunnel token --cred-file` for an
 * existing tunnel's narrow run credential, `tunnel create --credentials-file
 * --output json` for one laplus allocates, `tunnel route dns`, and
 * `tunnel --config … run` serving `/ready`. Deliberately not shared with
 * `tests/harness/cloudflare.rs` — that one is a Rust fixture compiled into the
 * test binaries, and importing it here would mean building a Rust crate to run
 * a browser. What has to agree is the *interface* cloudflared presents, and both
 * are written against that.
 *
 * `certificate: false` writes no account certificate at all, which is what
 * makes ticket 02's connector-token walkthrough mean anything: a world where a
 * certificate is merely unused proves nothing about a path whose whole claim is
 * that it never needs one.
 */
function writeScratch({ certificate = true } = {}) {
  const scratch = mkdtempSync(join(tmpdir(), "laplus-cloudflare-"));
  // On `PATH`, and named `cloudflared`, because discovery is what the wizard
  // offers to choose between — a stand-in somewhere else would need a path typed
  // into a field, which is not the flow a developer takes.
  const bin = join(scratch, "bin");
  mkdirSync(bin, { recursive: true });
  const world = {
    scratch,
    bin,
    cloudflared: join(bin, "cloudflared"),
    certificate: join(scratch, "cert.pem"),
    hasCertificate: certificate,
    trace: join(scratch, "cloudflared.trace"),
    listing: join(scratch, "tunnels.json"),
    mode: join(scratch, "cloudflared.mode"),
    // Which connector is running right now, so the supervision walkthrough can
    // kill *this* child rather than guess at a pid or match on a name.
    pidfile: join(scratch, "connector.pid"),
    // How long a relaunched connector takes to answer `/ready`. `degraded` is
    // the window between a restart and the replacement being ready, and a
    // stand-in that binds its port instantly makes that window unobservable.
    slow: join(scratch, "cloudflared.slow"),
    data: join(scratch, "data"),
  };

  writeFileSync(
    world.cloudflared,
    `#!/usr/bin/env python3
import http.server, json, os, signal, sys, time
ARGS = sys.argv[1:]
TRACE = ${JSON.stringify(world.trace)}
CERT = ${JSON.stringify(world.certificate)}
LISTING = ${JSON.stringify(world.listing)}
MODE = ${JSON.stringify(world.mode)}
PIDFILE = ${JSON.stringify(world.pidfile)}
SLOW = ${JSON.stringify(world.slow)}
CREATED = ${JSON.stringify(CREATED)}

if '--version' in ARGS:
    print('cloudflared version 2026.7.3')
    raise SystemExit(0)

with open(TRACE, 'a') as f:
    f.write(json.dumps(ARGS) + '\\n')

mode = open(MODE).read().strip() if os.path.exists(MODE) else 'ok'

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

if 'create' in ARGS:
    # \`--credentials-file\` keeps the narrow run credential out of cloudflared's
    # default location and inside laplus's private directory, and the UUID it
    # allocates is deliberately not the name it was asked for.
    assert ARGS[1] == '--origincert' and ARGS[2] == CERT, ARGS
    credentials = after('--credentials-file')
    assert credentials is not None, ARGS
    with open(credentials, 'w') as f:
        json.dump({'AccountTag': 'account', 'TunnelID': CREATED,
                   'TunnelSecret': 'FAKE-TUNNEL-CREDENTIAL-SECRET'}, f)
    os.chmod(credentials, 0o600)
    print(json.dumps({'id': CREATED, 'name': ARGS[-1],
                      'created_at': '2026-08-03T00:00:00Z'}))
    raise SystemExit(0)

if 'route' in ARGS:
    assert ARGS[1] == '--origincert' and ARGS[2] == CERT, ARGS
    if mode == 'route-fails':
        print('failed to add route', file=sys.stderr)
        raise SystemExit(1)
    print('Added CNAME %s which will route to this tunnel' % ARGS[-1])
    raise SystemExit(0)

if 'delete' in ARGS:
    # The only Cloudflare mutation a deletion can make with the account
    # certificate. The DNS record is not removable from here at all — there is no
    # \`route dns delete\` — which is why the driver also stands up a fake DNS API.
    assert ARGS[1] == '--origincert' and ARGS[2] == CERT, ARGS
    print('Deleted tunnel %s' % ARGS[-1])
    raise SystemExit(0)

if 'run' not in ARGS:
    raise SystemExit(2)

# Written first, and overwritten on every launch, so "the connector running now"
# is a fact the driver can read rather than infer.
with open(PIDFILE, 'w') as f:
    f.write(str(os.getpid()))

config = after('--config')
assert config is not None, ARGS
token_file = after('--token-file')
if token_file is not None:
    # **Ticket 02's own credential.** A connector token carries its tunnel with
    # it, so laplus's configuration names neither a tunnel nor a credentials
    # file — what it must name is a *file* holding the token, never the token.
    # Asserted here, so a build that passed \`--token <value>\` would break the
    # stand-in rather than quietly pass the driver.
    assert '--token' not in ARGS, ARGS
    secret = open(token_file).read().strip()
    assert secret, token_file
    said = 'connector starting with token=%s' % secret
else:
    lines = [line.strip() for line in open(config).read().splitlines()]
    assert [l for l in lines if l.startswith('tunnel:')], lines
    held = [l for l in lines if l.startswith('credentials-file:')]
    assert held and os.path.exists(held[0].split(':', 1)[1].strip()), lines
    held = json.load(open(held[0].split(':', 1)[1].strip()))
    said = 'connector starting with TunnelSecret=%s' % held['TunnelSecret']

class Ready(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200 if self.path == '/ready' else 404)
        self.end_headers()
    def log_message(self, *args): pass

# Say the quiet part on stderr, the way a real cloudflared complaining about its
# credential would. laplus captures this as the connector's log, so it is what
# the redaction verdict reads — and a connector's output is drained when the
# child exits, which is exactly when a cleanup may already have removed the file
# the secret would have been recognised from.
print(said, file=sys.stderr)
sys.stderr.flush()

def stopped(*_):
    # The proof of a *graceful* stop: a SIGKILL, or a laplus that dropped its
    # child without asking it to go, cannot write this line.
    with open(TRACE, 'a') as f:
        f.write('stopped\\n')
    sys.exit(0)

host, port = after('--metrics').rsplit(':', 1)
# Installed before the slow start below, so a stop that arrives while this
# connector is still coming up is still a graceful one.
signal.signal(signal.SIGTERM, stopped)
signal.signal(signal.SIGINT, stopped)
if os.path.exists(SLOW):
    time.sleep(float(open(SLOW).read().strip() or '0'))
http.server.HTTPServer((host, int(port)), Ready).serve_forever()
`,
    { mode: 0o700 },
  );
  chmodSync(world.cloudflared, 0o700);
  // Detected rather than created, so the wizard opens on the consent step — the
  // path ADR-0045 is strictest about, because merely finding a certificate
  // grants laplus nothing.
  if (certificate) writeFileSync(world.certificate, "FAKE-ACCOUNT-CERTIFICATE-SECRET");
  mkdirSync(world.data, { recursive: true });
  return world;
}

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

// --- the Cloudflare DNS API, which is the one thing cloudflared cannot do ---

/**
 * A stand-in for Cloudflare's DNS API, on loopback.
 *
 * **Not a shortcut round the CLI — a different surface.** `cloudflared tunnel
 * route dns` creates a CNAME and there is no `route dns delete`, so "Delete
 * everywhere" removes the record through Cloudflare's REST API with DNS
 * authority of its own. Modelling it as another `cloudflared` verb would let
 * this driver pass against a command that does not exist.
 *
 * It lists the zones a token can see, lists the records in one, and deletes by
 * id — because the endpoint row records the record by *name* (ADR-0051) and the
 * deletion has to resolve it first. An unauthorized caller is answered `403`,
 * the way Cloudflare answers one.
 */
function startDnsApi(recordName) {
  const state = { records: [{ id: RECORD_ID, zone_id: ZONE_ID, name: recordName, type: "CNAME" }] };
  const seen = [];
  const answer = (response, status, body) => {
    response.writeHead(status, { "content-type": "application/json" });
    response.end(JSON.stringify(body));
  };
  const server = createServer((request, response) => {
    const url = new URL(request.url, "http://127.0.0.1");
    seen.push(`${request.method} ${url.pathname}`);
    if (request.headers.authorization !== `Bearer ${DNS_API_TOKEN}`) {
      return answer(response, 403, {
        success: false,
        errors: [{ code: 9109, message: "Invalid access token." }],
      });
    }
    if (url.pathname === "/client/v4/zones") {
      // Filtered by name, because that is what laplus asks: a record's zone is
      // one of its own suffixes, looked up one at a time. Answering with
      // everything regardless would let a build that pages a listing badly pass
      // here — Cloudflare caps this endpoint at fifty zones per page.
      const wanted = url.searchParams.get("name");
      return answer(response, 200, {
        success: true,
        errors: [],
        result: [{ id: ZONE_ID, name: "example.com" }].filter(
          (zone) => wanted === null || zone.name === wanted,
        ),
      });
    }
    const listed = url.pathname.match(/^\/client\/v4\/zones\/([^/]+)\/dns_records$/);
    if (listed) {
      const wanted = url.searchParams.get("name");
      return answer(response, 200, {
        success: true,
        errors: [],
        result: state.records.filter(
          (held) => held.zone_id === listed[1] && (wanted === null || held.name === wanted),
        ),
      });
    }
    const one = url.pathname.match(/^\/client\/v4\/zones\/([^/]+)\/dns_records\/([^/]+)$/);
    if (one && request.method === "DELETE") {
      const before = state.records.length;
      state.records = state.records.filter(
        (held) => !(held.id === one[2] && held.zone_id === one[1]),
      );
      return state.records.length === before
        ? // Cloudflare's own shape for "that record is not here", which an
          // idempotent retry has to read as already-done.
          answer(response, 404, {
            success: false,
            errors: [{ code: 81044, message: "Record does not exist." }],
          })
        : answer(response, 200, { success: true, errors: [], result: { id: one[2] } });
    }
    answer(response, 404, { success: false, errors: [{ code: 7000, message: "No route." }] });
  });
  return new Promise((resolve) => {
    server.listen(0, "127.0.0.1", () => {
      resolve({
        origin: `http://127.0.0.1:${server.address().port}`,
        records: () => state.records,
        requests: () => seen,
        stop: () => server.close(),
      });
    });
  });
}

// --- isolated servers, each with its own boot credential ---

const running = [];
const stopAll = () => {
  for (const server of running.splice(0)) server.kill("SIGTERM");
};
process.on("exit", stopAll);

/**
 * Start a `laplus-server` against one scratch world and return the paired URL it
 * announces.
 *
 * The credential comes out of the server's own startup lines rather than out of
 * anyone's SQLite file, so this needs no `sqlite3` and no knowledge of where the
 * profile went — and the token is in the URL **fragment**, without which every
 * `/api` call answers 401. See the README.
 */
async function startServer(world, port, dnsApiOrigin) {
  const server = spawn(SERVER, ["serve", "--port", port, "--ui", BUNDLE], {
    env: {
      ...process.env,
      // Only ever overridden towards loopback, and only by a build that checks
      // that — the request carries a Cloudflare API token in a header.
      ...(dnsApiOrigin ? { LAPLUS_CLOUDFLARE_API: dnsApiOrigin } : {}),
      HOME: join(world.scratch, "home"),
      USERPROFILE: join(world.scratch, "home"),
      XDG_DATA_HOME: world.data,
      XDG_CONFIG_HOME: join(world.scratch, "config"),
      PATH: `${world.bin}:${process.env.PATH ?? ""}`,
      LOCALAPPDATA: undefined,
      APPDATA: undefined,
      // Absent, rather than pointed at a file that is not there, for the
      // connector-token world: the claim is that the path needs no account.
      ...(world.hasCertificate ? { TUNNEL_ORIGIN_CERT: world.certificate } : {}),
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  server.on("error", () => giveUp(`${SERVER} would not run — cargo build -p laplus-server first`));
  running.push(server);
  const exited = new Promise((resolve) => server.once("exit", resolve));

  let announced = "";
  server.stdout.on("data", (chunk) => {
    announced += chunk.toString();
  });
  server.stderr.on("data", (chunk) => {
    announced += chunk.toString();
  });
  const url = await poll(() => {
    const found = announced.match(/http:\/\/\S*#token=\S+/);
    return found ? found[0].replace(/[)\s]+$/, "") : null;
  }, 30000);
  if (!url) {
    giveUp(`the server never announced a paired URL. What it said:\n${announced.slice(0, 2000)}`);
  }
  console.log(`server ${new URL(url).origin}`);
  return { url, process: server, exited };
}

/**
 * Stop one server and wait until it is actually gone.
 *
 * **Waiting is the point.** The persistence walkthrough starts a replacement on
 * the same port against the same data directory, and a start that races the
 * previous process's exit fails at `bind` — which would read as a server that
 * lost its configuration.
 */
const stopServer = async (started) => {
  const index = running.indexOf(started.process);
  if (index >= 0) running.splice(index, 1);
  started.process.kill("SIGTERM");
  await started.exited;
};

const adoption = writeScratch();
const listSpareAs = (state) =>
  writeFileSync(adoption.listing, tunnels(state === "active" ? [{ id: "somebody-else" }] : []));
listSpareAs("inactive");
const creation = writeScratch();
writeFileSync(creation.listing, tunnels([]));
// No certificate at all: ticket 02's path is the one that never signs in.
const connectorToken = writeScratch({ certificate: false });
const worlds = [adoption, creation, connectorToken];
const dnsApi = await startDnsApi(NEW_HOSTNAME);

const { url } = await startServer(adoption, PORT);
const trace = adoption.trace;

// --- the browser ---

let session;
try {
  session = await launch({ url });
} catch (cause) {
  stopAll();
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

/**
 * Take the page to one server's Connections view, credential and all.
 *
 * Called once per walkthrough. The second call is a *different origin*, so it
 * is booted exactly the way the first was — the boot credential rides in the URL
 * fragment and nothing from the first server carries over, which is the point of
 * two servers rather than two data directories.
 */
const arriveAtConnections = async (paired) => {
  await session.evaluate(`location.assign(${JSON.stringify(paired)}); return 1;`);
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
  return arrived;
};

/** Put text into a React-controlled field, the only way React notices. */
const type = (label, value) =>
  session.evaluate(`
    const field = document.querySelector('[aria-label=${JSON.stringify(label)}]');
    if (!field) return "NO FIELD: " + ${JSON.stringify(label)};
    // React tracks an input's value on the node, so assigning \`.value\` updates
    // the box and tells the component nothing.
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set;
    setter.call(field, ${JSON.stringify(value)});
    field.dispatchEvent(new Event("input", { bubbles: true }));
    return "typed";
  `);

/**
 * The private files laplus wrote for this exposure, wherever the data directory
 * turned out to be.
 *
 * Walked rather than assumed, because the point of the assertion is that forget
 * removed *laplus's own* configuration and credential — and a path spelled by
 * hand that happened to be wrong would pass by finding nothing.
 */
function privateFiles(world) {
  const found = [];
  const walk = (directory) => {
    for (const entry of readdirSync(directory, { withFileTypes: true })) {
      const path = join(directory, entry.name);
      if (entry.isDirectory()) walk(path);
      else if (
        ["connector.yml", "connector.json", "tunnel.json", "connector.token"].includes(entry.name)
      ) {
        found.push(path);
      }
    }
  };
  walk(world.data);
  return found;
}

/** How many recorded invocations of the stand-in mention `verb`. */
const invocations = (world, verb) =>
  readFileSync(world.trace, "utf8")
    .split("\n")
    .filter((line) => line.includes(`"${verb}"`)).length;

/**
 * Every file anywhere under this world that contains `secret`, contents-first.
 *
 * The box's claim is about *non-secret* persistence, so the answer this is
 * asked for is "the private credential file, and nothing else" — a database, a
 * settings file, an ingress configuration or a command trace holding the same
 * bytes is the failure. Walked from the scratch root rather than from a path
 * spelled by hand, because a wrong path would pass by finding nothing.
 */
function filesHolding(world, secret) {
  const held = [];
  const walk = (directory) => {
    for (const entry of readdirSync(directory, { withFileTypes: true })) {
      const path = join(directory, entry.name);
      if (entry.isDirectory()) walk(path);
      else if (readFileSync(path).includes(secret)) held.push(path);
    }
  };
  walk(world.scratch);
  return held;
}

/** The stand-in connector this world is running right now. */
const connectorPid = (world) => Number(readFileSync(world.pidfile, "utf8").trim());

/**
 * Kill the connector out from under the server, and answer which pid died.
 *
 * **A kill is not the Stop button**, and that distinction is the whole of this
 * walkthrough. Stop is the operator asking, and it reaches `set_desired`;
 * nothing a developer can press reaches `child_failed`, which is the only
 * place `restartCount` moves and `degraded` and `restart-exhausted` are
 * written. `SIGKILL` so the stand-in's own signal handler cannot run: a child
 * that exits gracefully on its own is still a child that went away.
 */
const killConnector = (world) => {
  const doomed = connectorPid(world);
  process.kill(doomed, "SIGKILL");
  return doomed;
};

/**
 * Nothing this driver started may outlive it, including a connector whose
 * server was killed while it was up.
 *
 * The pid is confirmed against its own command line before anything is
 * signalled — a pid file outlives its process and pids are reused, so a
 * teardown that trusted the number could kill somebody else's work.
 */
const reapConnectors = () => {
  for (const world of worlds) {
    try {
      const pid = connectorPid(world);
      if (readFileSync(`/proc/${pid}/cmdline`, "utf8").includes(world.scratch)) {
        process.kill(pid, "SIGKILL");
      }
    } catch {
      // No connector ever ran in this world, or it is already gone.
    }
  }
};

try {
  const arrived = await arriveAtConnections(url);
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

  // --- ticket 07: stop, restart, and forget, on the adopted tunnel ---
  //
  // Stop changes whether the connector runs and nothing else; forget removes
  // laplus's own configuration and credential *after* stopping it, and touches
  // nothing at Cloudflare — an adopted tunnel's allocation and DNS route are
  // somebody else's throughout.

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
    check("stopping is reported as stopped", after.body.cleanup?.state, "stopped");
    check("stopping keeps everything restartable", after.body.cleanup?.remaining?.length, 0);
    check("stopping deletes nothing at Cloudflare", invocations(adoption, "delete"), 0);

    // **Gracefully, which is a different claim from "no longer running".** The
    // stand-in writes this line from its `SIGTERM` handler, so a laplus that
    // hard-killed its child — or dropped it without asking it to go — cannot
    // produce it. The same assertion the Rust harness spells
    // `FakeCloudflared::stopped_gracefully()`, made here through the button a
    // developer actually presses.
    const graceful = await poll(
      async () => (readFileSync(adoption.trace, "utf8").includes("stopped") ? "yes" : null),
      20000,
    );
    check("the connector was asked to stop rather than killed", graceful, "yes");

    // **Ticket 02, checkbox 6: the logs a developer is shown are redacted.** The
    // stand-in prints its own `TunnelSecret` on stderr the way a real cloudflared
    // complaining about its credential would, and a connector's output is only
    // drained when the child exits — so this is the first moment there is
    // anything to read, and the last moment before a cleanup could remove the
    // file the secret would have been recognised from.
    const logged = await poll(async () => {
      const answer = await serverState("/api/access/cloudflare/connector");
      return (answer.body.logs ?? []).length > 0 ? answer : null;
    }, 20000);
    if (!logged) {
      fail("the connector never reported the output the stand-in wrote");
    } else {
      const spoken = (logged.body.logs ?? []).join("\n");
      if (spoken.includes("FAKE-TUNNEL-CREDENTIAL-SECRET")) {
        fail(`the connector's logs quoted its run credential: ${spoken}`);
      } else {
        check("connector logs are redacted", true, true);
      }
      // Redacted rather than dropped: the actionable half has to survive, or the
      // rule would be satisfied by showing nothing.
      check("connector logs stay actionable", spoken.includes("[REDACTED]"), true);
      check("connector logs still name what happened", spoken.includes("connector starting"), true);
    }
  }

  // Restart: one action, and no second credential retrieval — the whole point of
  // stop preserving the setup rather than unwinding it.
  await pressed(SELECTORS.startConnector);
  const restarted = await poll(async () => {
    const answer = await serverState("/api/access/cloudflare/connector");
    return answer.body.connectorState === "ready" ? answer : null;
  }, 30000);
  if (!restarted) {
    fail("the connector did not come back after being stopped");
  } else {
    check("restarting needs no new credential", invocations(adoption, "token"), 1);
    check("restarting keeps the ownership", restarted.body.tunnelOwnership, "adopted");
  }

  // Forget. Ticket 05 left this unbuilt on purpose: the route removed the
  // endpoint row and stopped nothing, so forgetting an adopted tunnel left a
  // connector serving a public hostname nothing recorded.
  await pressed(SELECTORS.forget);
  // **Offered is not done.** The confirmation names what is removed and what is
  // untouched, and nothing has happened until the second press.
  const forgetOffer = await session.evaluate(
    `return document.querySelector('[aria-label="Forget local setup"]')?.innerText ?? null;`,
  );
  if (!forgetOffer) {
    fail("the forget confirmation never appeared");
  } else {
    for (const [what, shown] of [
      ["what it removes", "connector configuration"],
      ["that Cloudflare is untouched", "the Cloudflare tunnel and its DNS record"],
    ]) {
      if (forgetOffer.includes(shown)) {
        console.log(`  ok the forget confirmation names ${what}`);
      } else {
        fail(`the forget confirmation does not name ${what} (${JSON.stringify(shown)})`);
      }
    }
  }
  check("nothing is forgotten by being offered", privateFiles(adoption).length > 0, true);
  await pressed(SELECTORS.confirmForget);
  const forgotten = await poll(async () => {
    const answer = await serverState("/api/access/cloudflare");
    return answer.body.configured === false ? answer : null;
  }, 20000);
  if (!forgotten) {
    fail("forget did not reach the server");
  } else {
    check("forget reports what it removed", forgotten.body.cleanup?.state, "forgotten");
    check("forget leaves nothing outstanding", forgotten.body.cleanup?.remaining?.length, 0);
    const bare = await serverState("/api/access/cloudflare/connector");
    check("forget stops supervising the connector", bare.body.configured, false);
    // **Nothing at Cloudflare, for an adopted tunnel or any other.**
    check("forget deletes no tunnel", invocations(adoption, "delete"), 0);
    check("forget touches no DNS record", invocations(adoption, "route"), 0);
    // The account certificate is cloudflared's, and is exactly as it was.
    check(
      "the account certificate is untouched",
      readFileSync(adoption.certificate, "utf8"),
      "FAKE-ACCOUNT-CERTIFICATE-SECRET",
    );
    check("forget starts no new sign-in", invocations(adoption, "login"), 0);
    // What laplus owned is gone, which is what releases a later creation.
    const held = privateFiles(adoption);
    if (held.length) fail(`forget left laplus's own files behind: ${held.join(", ")}`);
    const account = await serverState("/api/access/cloudflare/account");
    check("the wizard can set up again", account.body.step, "choose-tunnel");
    check("the forgotten tunnel is no longer selected", account.body.selection, null);
  }

  // --- ticket 01: registering a hostname somebody else runs ---
  //
  // **Driven here because this is the one moment it is reachable.** Registering
  // an external endpoint is refused while laplus supervises a connector — one
  // lifecycle, one owner (ADR-0045) — and the forget above is what released
  // this environment. So ticket 01's path gets the world ticket 07 left behind
  // rather than a third server.
  //
  // **Verification and pairing are still not reachable, and still not faked.**
  // A verified endpoint needs a hostname that genuinely resolves in public DNS
  // and an HTTPS path back to this machine, which a scratch world has neither
  // of; pairing is only offered once verification succeeded. Both are covered
  // against the hermetic verifier in `tests/http_public_exposure.rs`. What a
  // browser *can* prove is everything up to the probe: that the wizard reaches
  // the step, that the hostname is normalized and recorded as somebody else's,
  // that laplus starts no process for it, and that an unverified endpoint is not
  // advertised for pairing.
  await pressed(SELECTORS.changePath);
  await pressed(SELECTORS.externalPath);
  const launchesBefore = invocations(adoption, "run");
  const typed = await type(
    SELECTORS.externalHostnameField,
    `  ${EXTERNAL_HOSTNAME.toUpperCase()}. `,
  );
  if (typed !== "typed") {
    fail(`the external hostname field was not reachable: ${typed}`);
  } else {
    await settle(300);
    await pressed(SELECTORS.register);
    const registered = await poll(async () => {
      const answer = await serverState("/api/access/cloudflare");
      return answer.body.configured === true ? answer : null;
    }, 20000);
    if (!registered) {
      fail("registering an external hostname did not reach the server");
    } else {
      // Normalized by the server, from a hostname typed with the padding,
      // casing and trailing dot a developer actually produces.
      check(
        "the external hostname is normalized",
        registered.body.httpsOrigin,
        `https://${EXTERNAL_HOSTNAME}`,
      );
      check(
        "the endpoint derives its own wss origin",
        registered.body.wssOrigin,
        `wss://${EXTERNAL_HOSTNAME}`,
      );
      // The whole of what this path claims: laplus verifies and advertises, and
      // owns nothing.
      check(
        "an external endpoint is somebody else's tunnel",
        registered.body.ownership,
        "external",
      );
      check("and is never laplus's to delete", registered.body.deletableAtCloudflare, false);
      check("its connector is external", registered.body.health?.connector, "external");
      // No process, no Cloudflare mutation. This path runs no cloudflared at all.
      check("registering launches no connector", invocations(adoption, "run"), launchesBefore);
      check("registering allocates no tunnel", invocations(adoption, "create"), 0);
      check("registering routes no DNS record", invocations(adoption, "route"), 0);
      const bare = await serverState("/api/access/cloudflare/connector");
      check("laplus supervises nothing for it", bare.body.configured, false);
      // **Only a verified endpoint is advertised.** Verification cannot succeed
      // here, which is precisely what makes this the assertion worth having: an
      // endpoint that has never been proven must not be offered for pairing.
      check("an unverified endpoint is not advertised", registered.body.advertisedEndpoint, null);
      if (registered.body.verificationState === "verified") {
        fail("a scratch hostname reported itself verified, which it cannot be");
      } else {
        check("verification is attempted rather than assumed", true, true);
      }
    }
  }

  // --- ticket 06: creating a stable tunnel, on a server of its own ---
  //
  // A second laplus, because the first is now supervising a connector and the
  // wizard rightly will not offer setup for an exposure that already exists.

  console.log("\n--- creating a tunnel ---");
  const { url: createdUrl } = await startServer(creation, String(Number(PORT) + 1), dnsApi.origin);
  const reached = await arriveAtConnections(createdUrl);
  if (!reached) giveUp("never reached the Connections page on the creation server");

  await pressed(SELECTORS.openWizard);
  await pressed(SELECTORS.accountPath);
  await pressed(SELECTORS.consent);
  await pressed(SELECTORS.refreshTunnels);
  await pressed(SELECTORS.createInstead);

  // **The preview is the confirmation.** Everything the developer is agreeing to
  // has to be on the screen before the button does anything: the tunnel's name,
  // the exact HTTPS address, the DNS change, the loopback target, where the run
  // credential will be kept, and that the hostname will be public.
  const namedIt = await type(SELECTORS.newNameField, NEW_NAME);
  if (namedIt !== "typed") fail(namedIt);
  const addressedIt = await type(SELECTORS.newHostnameField, NEW_HOSTNAME);
  if (addressedIt !== "typed") fail(addressedIt);
  await settle(400);

  const preview = await session.evaluate(
    `return document.querySelector('[aria-label="Create a tunnel"]')?.innerText ?? "";`,
  );
  for (const shown of [
    ["the tunnel name", NEW_NAME],
    ["the exact HTTPS address", `https://${NEW_HOSTNAME}`],
    ["the DNS change", `A new CNAME record for ${NEW_HOSTNAME}`],
    ["the loopback target", "http://127.0.0.1:"],
    ["the credential location", "tunnel.json"],
    ["that it is locally managed", "locally managed on this computer"],
    ["the public-exposure warning", "reachable from the public Internet"],
    ["that laplus auth still applies", "laplus authentication remains required"],
  ]) {
    if (preview.includes(shown[1])) {
      console.log(`  ok the preview shows ${shown[0]}`);
    } else {
      fail(`the preview does not show ${shown[0]} (${JSON.stringify(shown[1])})`);
    }
  }

  // --- the partial creation, which is what this ticket is mostly about ---
  //
  // The tunnel is allocated and the DNS route refuses. laplus has really made a
  // tunnel at this point, so the screen must say so rather than imply a
  // rollback — there is no `tunnel delete` in the creation path at all.
  writeFileSync(creation.mode, "route-fails");
  await pressed(SELECTORS.create);

  const halfway = await serverState("/api/access/cloudflare");
  check("a half-finished creation records no endpoint", halfway.body.configured, false);
  check("the tunnel was allocated once", invocations(creation, "create"), 1);
  const refusal = await session.evaluate(
    `return document.body?.innerText?.includes("Already done: creating the tunnel.") ? "said" : (document.body?.innerText ?? "").slice(0, 400);`,
  );
  check("the screen names the work already done", refusal, "said");
  const outstanding = await session.evaluate(
    `return document.body?.innerText?.includes("Still outstanding: creating the DNS route") ? "said" : "missing";`,
  );
  check("the screen names the work still outstanding", outstanding, "said");

  // Retry from exactly where it stopped: the credential on disk is the
  // allocation having happened, so a second attempt must route and configure
  // without allocating a second tunnel.
  rmSync(creation.mode, { force: true });
  await pressed(SELECTORS.create);

  const created = await poll(async () => {
    const answer = await serverState("/api/access/cloudflare");
    return answer.body.configured ? answer : null;
  }, 30000);
  if (!created) {
    fail("the server never recorded an endpoint after creation");
  } else {
    check("endpoint ownership", created.body.ownership, "laplus-created");
    check("endpoint hostname", created.body.httpsOrigin, `https://${NEW_HOSTNAME}`);
    // The only ownership that authorizes a Cloudflare deletion. Ticket 07 owns
    // the command; what is proven here is that the server states the verdict.
    check("a laplus-created tunnel is deletable", created.body.deletableAtCloudflare, true);
    check("laplus runs the connector", created.body.health?.connector, "laplus");
  }
  // The whole point of resuming from observed state: one allocation, two routes,
  // across a refusal and a retry.
  check("the retry allocated no second tunnel", invocations(creation, "create"), 1);
  check("the retry did route the hostname", invocations(creation, "route"), 2);

  const madeConnector = await poll(async () => {
    const answer = await serverState("/api/access/cloudflare/connector");
    return answer.body.connectorState === "ready" ? answer : null;
  }, 30000);
  if (!madeConnector) {
    const last = await serverState("/api/access/cloudflare/connector");
    fail(`the created connector never became ready: ${JSON.stringify(last.body).slice(0, 400)}`);
  } else {
    check("connector tunnel ownership", madeConnector.body.tunnelOwnership, "laplus-created");
    check("connector is supervised", madeConnector.body.desiredState, "running");
    check("connector readiness", madeConnector.body.readiness, true);
    check("a deletion is offered for this one", madeConnector.body.deletableAtCloudflare, true);
  }

  const createdAccount = await serverState("/api/access/cloudflare/account");
  check("wizard resumes at the created step", createdAccount.body.step, "creating");
  check("the selection is a created tunnel", createdAccount.body.selection?.created, true);
  // The UUID Cloudflare allocated, never the name laplus asked for — cleanup
  // targets the resource that exists.
  check(
    "the recorded tunnel is the allocated one",
    createdAccount.body.selection?.tunnelId,
    CREATED,
  );

  const createdAnswers = JSON.stringify([created?.body, madeConnector?.body, createdAccount.body]);
  if (createdAnswers.includes("FAKE-TUNNEL-CREDENTIAL-SECRET")) {
    fail("a run credential reached a snapshot");
  }
  if (createdAnswers.includes("FAKE-ACCOUNT-CERTIFICATE-SECRET")) {
    fail("the account certificate reached a snapshot");
  }

  // --- ticket 07's closeout: the laplus-created-only delete path ---
  //
  // Stop, restart, and then the one operation that removes something outside
  // this machine. It is offered here and nowhere else, it is confirmed against
  // the resources the server recorded, and it needs DNS authority the CLI cannot
  // supply.

  await pressed(SELECTORS.stopConnector);
  const createdStopped = await poll(async () => {
    const answer = await serverState("/api/access/cloudflare/connector");
    return answer.body.desiredState === "stopped" ? answer : null;
  }, 20000);
  if (!createdStopped) fail("stop did not reach the creation server");
  await pressed(SELECTORS.startConnector);
  const createdRestarted = await poll(async () => {
    const answer = await serverState("/api/access/cloudflare/connector");
    return answer.body.connectorState === "ready" ? answer : null;
  }, 30000);
  if (!createdRestarted) {
    fail("the created connector did not come back after being stopped");
  } else {
    check("a restart allocates no second tunnel", invocations(creation, "create"), 1);
    check("a restart routes no second record", invocations(creation, "route"), 2);
  }

  await pressed(SELECTORS.offerDelete);
  // **The confirmation is the server's, and it names the resources the row
  // holds.** A screen that composed these from what the client believed is
  // exactly what ADR-0052 refuses.
  const confirmation = await poll(
    () =>
      session.evaluate(
        `return document.querySelector('[aria-label="Delete everywhere"]')?.innerText ?? null;`,
      ),
    15000,
  );
  if (!confirmation) {
    fail("the destructive confirmation never appeared");
  } else {
    for (const [what, shown] of [
      ["the allocated tunnel", CREATED],
      ["the recorded DNS record", NEW_HOSTNAME],
      ["that the account token is never revoked", "never revokes your Cloudflare account token"],
      ["that the certificate is never touched", "never touches your account certificate"],
    ]) {
      if (confirmation.includes(shown)) {
        console.log(`  ok the confirmation names ${what}`);
      } else {
        fail(`the confirmation does not name ${what} (${JSON.stringify(shown)})`);
      }
    }
  }
  check("nothing is deleted by being offered", invocations(creation, "delete"), 0);
  check("no DNS record is deleted by being offered", dnsApi.records().length, 1);

  const typedToken = await type(SELECTORS.dnsTokenField, DNS_API_TOKEN);
  if (typedToken !== "typed") fail(typedToken);
  await settle(400);
  await pressed(SELECTORS.confirmDelete);

  const removed = await poll(async () => {
    const answer = await serverState("/api/access/cloudflare");
    return answer.body.cleanup?.state === "fully-removed" ? answer : null;
  }, 30000);
  if (!removed) {
    const last = await serverState("/api/access/cloudflare");
    fail(`the deletion never completed: ${JSON.stringify(last.body).slice(0, 400)}`);
  } else {
    check("the endpoint is gone", removed.body.configured, false);
    check("nothing is left outstanding", removed.body.cleanup?.remaining?.length, 0);
    check("the tunnel it removed is named", removed.body.cleanup?.tunnelId, CREATED);
  }
  // **The exact recorded resources, and no others.** The row carried the record
  // by name alone, so the deletion had to resolve it through the zone first.
  check("the recorded DNS record is gone", dnsApi.records().length, 0);
  const deleted = dnsApi.requests().filter((line) => line.startsWith("DELETE"));
  check("exactly one DNS record was deleted", deleted.length, 1);
  check(
    "and it was the recorded one",
    deleted[0],
    `DELETE /client/v4/zones/${ZONE_ID}/dns_records/${RECORD_ID}`,
  );
  check("the tunnel was deleted once", invocations(creation, "delete"), 1);
  const deletionTrace = readFileSync(creation.trace, "utf8");
  if (!deletionTrace.includes(CREATED)) fail("the deletion did not target the allocated tunnel");
  if (deletionTrace.includes(DNS_API_TOKEN)) fail("the DNS API token reached a command line");
  check(
    "the account certificate is untouched",
    readFileSync(creation.certificate, "utf8"),
    "FAKE-ACCOUNT-CERTIFICATE-SECRET",
  );
  check("the deletion starts no new sign-in", invocations(creation, "login"), 0);
  const leftBehind = privateFiles(creation);
  if (leftBehind.length)
    fail(`the deletion left laplus's own files behind: ${leftBehind.join(", ")}`);
  const afterDeletion = await serverState("/api/access/cloudflare/connector");
  check("no connector survives the deletion", afterDeletion.body.configured, false);
  if (JSON.stringify([removed?.body, afterDeletion.body]).includes(DNS_API_TOKEN)) {
    fail("the DNS API token reached a snapshot");
  }

  // --- ticket 02: a connector token, and no Cloudflare account anywhere ---
  //
  // **The surface this ticket is named for**, and the one path here that signs
  // in to nothing: a developer who already made the tunnel at Cloudflare brings
  // a compatible `cloudflared` and that tunnel's own connector token. laplus
  // runs and supervises the connector; Cloudflare keeps the control plane, so
  // there is nothing for laplus to delete and nothing for it to allocate.
  //
  // A third server, and a world with **no account certificate written at all**.
  // A world where a certificate merely goes unused would prove nothing about a
  // path whose entire claim is that it never needs one.

  console.log("\n--- a connector token, and no account ---");
  const tokenPort = String(Number(PORT) + 2);
  let tokenServer = await startServer(connectorToken, tokenPort);
  if (!(await arriveAtConnections(tokenServer.url))) {
    giveUp("never reached the Connections page on the connector-token server");
  }

  await pressed(SELECTORS.openWizard);
  await pressed(SELECTORS.tokenPath);

  // Discovered rather than typed. Checkbox 1's claim is that the wizard offers
  // the executables it found, so a path pasted into the free-text field would
  // drive the panel while proving nothing about discovery.
  const picked = await session.evaluate(`
    const radios = [...document.querySelectorAll('input[name="cloudflared-executable"]')];
    const found = radios.find((element) => element.value === ${JSON.stringify(connectorToken.cloudflared)});
    if (!found) return "NOT DISCOVERED, offered: " + radios.map((one) => one.value).join(", ");
    found.click();
    return "picked";
  `);
  check("the wizard discovered the stand-in cloudflared", picked, "picked");
  const namedHost = await type(SELECTORS.managedHostnameField, TOKEN_HOSTNAME);
  if (namedHost !== "typed") fail(namedHost);
  const pastedToken = await type(SELECTORS.connectorTokenField, CONNECTOR_TOKEN);
  if (pastedToken !== "typed") fail(pastedToken);
  await settle(400);
  await pressed(SELECTORS.saveConnector);

  const ran = await poll(async () => {
    const answer = await serverState("/api/access/cloudflare/connector");
    return answer.body.connectorState === "ready" ? answer : null;
  }, 30000);
  if (!ran) {
    const last = await serverState("/api/access/cloudflare/connector");
    fail(
      `the connector-token connector never became ready: ${JSON.stringify(last.body).slice(0, 400)}`,
    );
  } else {
    check("the connector is ready", ran.body.readiness, true);
    check("laplus supervises it", ran.body.desiredState, "running");
    // Two owners, one connector: laplus owns the process and Cloudflare owns
    // the tunnel. Collapsing them is what ADR-0049 exists to stop.
    check("laplus runs the process", ran.body.ownership, "laplus");
    check("Cloudflare keeps the tunnel", ran.body.tunnelOwnership, "external");
    check("so nothing here is laplus's to delete", ran.body.deletableAtCloudflare, false);
    check("the hostname the developer typed", ran.body.httpsOrigin, `https://${TOKEN_HOSTNAME}`);
    check(
      "the executable the developer chose",
      ran.body.executablePath,
      connectorToken.cloudflared,
    );
    check(
      "the credential is a private file",
      ran.body.credentialPath?.endsWith("connector.token"),
      true,
    );
    // Readiness is the connector's own `/ready`, reached without any public
    // endpoint having been proven — the separation checkbox 4 is about.
    const endpoint = await serverState("/api/access/cloudflare");
    check("the endpoint is somebody else's tunnel", endpoint.body.ownership, "external");
    check("with laplus's connector in front of it", endpoint.body.health?.connector, "laplus");
    check(
      "a locally ready connector is not publicly verified",
      endpoint.body.advertisedEndpoint,
      null,
    );
  }

  // **Nothing at Cloudflare was touched, because nothing could be.** No
  // certificate exists in this world, so a build that reached for the account
  // path here would fail rather than quietly succeed against a stale one.
  const accountless = await serverState("/api/access/cloudflare/account");
  check("no account certificate exists", accountless.body.certificateDetected, false);
  check("no sign-in was ever started", accountless.body.loginState, "not-started");
  check("no certificate was consented to", accountless.body.certificateConsentedAt, null);
  check("no tunnel was selected", accountless.body.selection, null);
  check("no tunnel was listed", invocations(connectorToken, "list"), 0);
  check("no tunnel was allocated", invocations(connectorToken, "create"), 0);
  check("no DNS record was routed", invocations(connectorToken, "route"), 0);
  check("no run credential was fetched", invocations(connectorToken, "token"), 0);
  check("no sign-in was attempted", invocations(connectorToken, "login"), 0);

  // --- where the token is, and every place it must not be ---

  const tokenTrace = readFileSync(connectorToken.trace, "utf8");
  check("the token is handed over by file", tokenTrace.includes("--token-file"), true);
  if (tokenTrace.includes(CONNECTOR_TOKEN)) fail("the connector token reached a command line");
  const tokenAnswers = JSON.stringify([ran?.body, accountless.body]);
  if (tokenAnswers.includes(CONNECTOR_TOKEN)) fail("the connector token reached a snapshot");
  // **Non-secret persistence, read off the disk rather than off the wire.**
  // Everything laplus wrote for this world is walked, and exactly one file may
  // hold the token: the private one it was written to on purpose.
  const holding = filesHolding(connectorToken, CONNECTOR_TOKEN);
  check("exactly one file on disk holds the token", holding.length, 1);
  check(
    "and it is the private credential file",
    holding[0]?.endsWith("connector.token") ?? null,
    true,
  );
  const tokenFile = holding.length === 1 ? holding[0] : null;
  if (tokenFile) {
    check("kept private to this user", (statSync(tokenFile).mode & 0o777).toString(8), "600");
  }
  /**
   * When the token file was last written, which is when setup last ran.
   *
   * `configure` is the only thing that writes it, so "unchanged" is the
   * strongest available form of "the developer did not re-enter the token" —
   * stronger than the field being empty on screen, which a client could fake.
   */
  const tokenWrittenAt = () => (tokenFile ? statSync(tokenFile).mtimeMs : null);
  const setupRanAt = tokenWrittenAt();

  // --- supervision: the child dies, and nothing a developer pressed did it ---
  //
  // A prior audit of this driver found its "stop and restart" to be the
  // operator's own two buttons. Nothing killed a connector, so `restartCount`,
  // `degraded` and `restart-exhausted` never appeared in a verdict at all. Here
  // the stand-in is killed out from under the server, three times, which is the
  // budget `MAX_RESTARTS` sets — and the budget is *not* refilled by a
  // connector that recovers, only by an explicit Retry.
  //
  // The replacement is made slow to come up on purpose: `degraded` is the
  // window between the restart and the replacement answering `/ready`, and a
  // stand-in that binds instantly makes that window too small to read.
  writeFileSync(connectorToken.slow, "3");

  for (const spent of [1, 2]) {
    const doomed = killConnector(connectorToken);
    const degraded = await poll(async () => {
      const answer = await serverState("/api/access/cloudflare/connector");
      return answer.body.connectorState === "degraded" ? answer : null;
    }, 25000);
    if (!degraded) {
      const last = await serverState("/api/access/cloudflare/connector");
      fail(
        `a killed connector never reported degraded: ${JSON.stringify(last.body).slice(0, 300)}`,
      );
    } else {
      check(`restart ${spent} is counted`, degraded.body.restartCount, spent);
      check("a connector being restarted is not ready", degraded.body.readiness, false);
      check("and is still wanted running", degraded.body.desiredState, "running");
      // Degraded, never failed: the budget is not spent, so this is recoverable
      // and the row must not say the setup is broken.
      if (degraded.body.failureMessage === null) {
        fail("a degraded connector said nothing about why");
      } else {
        check(
          "with an actionable reason",
          degraded.body.failureMessage.includes("cloudflared"),
          true,
        );
      }
      if (JSON.stringify(degraded.body).includes(CONNECTOR_TOKEN)) {
        fail("the connector token reached a failure snapshot");
      }
    }
    const recovered = await poll(async () => {
      const answer = await serverState("/api/access/cloudflare/connector");
      return answer.body.connectorState === "ready" ? answer : null;
    }, 30000);
    if (!recovered) {
      fail(`the supervisor did not restart the connector after kill ${spent}`);
    } else {
      check(
        "the supervisor started a different process",
        connectorPid(connectorToken) !== doomed,
        true,
      );
      // **The budget is spent, not lent.** A connector that comes back does not
      // get its restarts back — that is what makes three kills reach the end of
      // the budget rather than three separate first failures.
      check("a recovered connector keeps its spent budget", recovered.body.restartCount, spent);
      check("and needed no new token", tokenWrittenAt(), setupRanAt);
    }
  }

  // The last one in the budget: no fourth launch, and an explicit retry is now
  // the only way back.
  const launchesBeforeExhaustion = invocations(connectorToken, "run");
  killConnector(connectorToken);
  const exhausted = await poll(async () => {
    const answer = await serverState("/api/access/cloudflare/connector");
    return answer.body.connectorState === "restart-exhausted" ? answer : null;
  }, 25000);
  if (!exhausted) {
    const last = await serverState("/api/access/cloudflare/connector");
    fail(`the restart budget never ran out: ${JSON.stringify(last.body).slice(0, 300)}`);
  } else {
    check("the budget is what the server enforces", exhausted.body.restartCount, MAX_RESTARTS);
    // Persisted, so a server that came back would not relaunch into the same
    // failure loop unattended.
    check("and is no longer wanted running", exhausted.body.desiredState, "stopped");
    check("the setup itself survives", exhausted.body.httpsOrigin, `https://${TOKEN_HOSTNAME}`);
  }
  await settle(2000);
  check(
    "no fourth restart was attempted",
    invocations(connectorToken, "run"),
    launchesBeforeExhaustion,
  );
  const settled = await serverState("/api/access/cloudflare/connector");
  // **`null`, not `false`.** Readiness is a fact about a running connector, and
  // an exhausted one has no child at all — the same answer a stopped one gives.
  // `false` would mean "running and not ready", which is a different screen.
  check("an exhausted connector reports no readiness at all", settled.body.readiness, null);
  check("and still says why it stopped", settled.body.failureMessage !== null, true);
  if (JSON.stringify(settled.body).includes(CONNECTOR_TOKEN)) {
    fail("the connector token reached an exhausted connector's snapshot");
  }

  // **Explicit retry, and nothing less.** Start is the same button as ever and
  // must be refused while the budget is spent, or "bounded restarts" would be
  // a bound the operator's own reflex walks straight through.
  await pressed(SELECTORS.startConnector);
  await settle(1500);
  const refused = await serverState("/api/access/cloudflare/connector");
  check("a plain start is refused", refused.body.connectorState, "restart-exhausted");
  check("and changes nothing", refused.body.desiredState, "stopped");
  check("still no further restart", invocations(connectorToken, "run"), launchesBeforeExhaustion);
  const explained = await session.evaluate(
    `return document.body?.innerText?.includes("Automatic restarts are exhausted") ? "said" : "missing";`,
  );
  check("the screen says Retry is what is needed", explained, "said");

  rmSync(connectorToken.slow, { force: true });
  await pressed(SELECTORS.retryConnector);
  const retried = await poll(async () => {
    const answer = await serverState("/api/access/cloudflare/connector");
    return answer.body.connectorState === "ready" ? answer : null;
  }, 30000);
  if (!retried) {
    const last = await serverState("/api/access/cloudflare/connector");
    fail(
      `an explicit retry did not start the connector: ${JSON.stringify(last.body).slice(0, 300)}`,
    );
  } else {
    check("retry refills the budget", retried.body.restartCount, 0);
    check("and starts the connector again", retried.body.desiredState, "running");
    check("retry re-entered no token", tokenWrittenAt(), setupRanAt);
  }

  // **Redacted, and read back off the server.** The stand-in printed its own
  // token on stderr the way a real cloudflared complaining about its credential
  // would, and that output is drained when the child exits — which here is a
  // kill rather than a stop, the harsher of the two moments ADR-0053 is about.
  const spoken = (await serverState("/api/access/cloudflare/connector")).body.logs ?? [];
  if (spoken.length === 0) {
    fail("the connector never reported the output the stand-in wrote");
  } else {
    const said = spoken.join("\n");
    if (said.includes(CONNECTOR_TOKEN)) {
      fail(`the connector's logs quoted its connector token: ${said.slice(0, 300)}`);
    } else {
      check("connector-token logs are redacted", true, true);
    }
    check("redacted rather than dropped", said.includes("[REDACTED]"), true);
    check("and still name what happened", said.includes("connector starting"), true);
  }

  // --- persistence: this server stops, and another one starts on its state ---
  //
  // The gap the box named. Each of the two walkthroughs above starts one server
  // and never restarts it, so every field checkbox 3 lists was only ever read
  // from the process that wrote it. Here the server is stopped and replaced
  // against the same data directory, and each field is read back **by name**.

  console.log("\n--- the same connector, after a server restart ---");
  const before = await serverState("/api/access/cloudflare/connector");
  const launchesBeforeRestart = invocations(connectorToken, "run");
  const graceful = (readFileSync(connectorToken.trace, "utf8").match(/^stopped$/gm) ?? []).length;

  await stopServer(tokenServer);
  // Checkbox 7's other half, and free here: a connector shuts down *with its
  // owner*. The stand-in can only write this line from its own signal handler,
  // so a server that dropped its child on the way out cannot produce it.
  const shutDown = await poll(
    async () =>
      (readFileSync(connectorToken.trace, "utf8").match(/^stopped$/gm) ?? []).length > graceful
        ? "yes"
        : null,
    15000,
  );
  check("the connector shut down with the server that owned it", shutDown, "yes");

  tokenServer = await startServer(connectorToken, tokenPort);
  if (!(await arriveAtConnections(tokenServer.url))) {
    giveUp("never reached the Connections page on the restarted server");
  }
  const survived = await poll(async () => {
    const answer = await serverState("/api/access/cloudflare/connector");
    return answer.body.connectorState === "ready" ? answer : null;
  }, 40000);
  if (!survived) {
    const last = await serverState("/api/access/cloudflare/connector");
    fail(
      `the connector did not survive a server restart: ${JSON.stringify(last.body).slice(0, 400)}`,
    );
  } else {
    // Every field checkbox 3 names, read back by name rather than implied by a
    // connector that reached `ready`.
    check("the configuration survives at all", survived.body.configured, true);
    check("the hostname survives", survived.body.httpsOrigin, before.body.httpsOrigin);
    check("the loopback origin survives", survived.body.loopbackOrigin, before.body.loopbackOrigin);
    check(
      "the executable selection survives",
      survived.body.executablePath,
      connectorToken.cloudflared,
    );
    check(
      "the private secret reference survives",
      survived.body.credentialPath,
      before.body.credentialPath,
    );
    check("the tunnel ownership survives", survived.body.tunnelOwnership, "external");
    check("the process ownership survives", survived.body.desiredState, "running");
    check("and the connector is ready again", survived.body.readiness, true);
    // **Restored, not re-set-up.** Nothing was typed into this browser after
    // the restart: the token file is untouched, no sign-in happened, and the
    // supervisor launched cloudflared again on its own.
    check(
      "the connector came back on its own",
      invocations(connectorToken, "run") > launchesBeforeRestart,
      true,
    );
    check("setup was not re-run", tokenWrittenAt(), setupRanAt);
    check("the restored connector starts fresh on its budget", survived.body.restartCount, 0);
    check("and still needs no account", invocations(connectorToken, "login"), 0);
    const restoredEndpoint = await serverState("/api/access/cloudflare");
    check(
      "the endpoint row survives too",
      restoredEndpoint.body.httpsOrigin,
      `https://${TOKEN_HOSTNAME}`,
    );
    check("with its ownership", restoredEndpoint.body.ownership, "external");
    check("and laplus still runs its connector", restoredEndpoint.body.health?.connector, "laplus");
    if (JSON.stringify([survived.body, restoredEndpoint.body]).includes(CONNECTOR_TOKEN)) {
      fail("the connector token reached a snapshot after a restart");
    }
  }
  const stillHolding = filesHolding(connectorToken, CONNECTOR_TOKEN);
  check("a restart widens nothing", stillHolding.length, 1);

  const errors = logs.filter((line) => /error|exception/i.test(line) && !/404|401/.test(line));
  if (errors.length) {
    console.log("\n=== CONSOLE ===");
    console.log(errors.slice(0, 6).join("\n").slice(0, 1600));
  }
} finally {
  await session.close();
  stopAll();
  // After the servers, because a live server would restart a connector this
  // reaped — and before the scratch directories, because it identifies each
  // process by the scratch path in its own command line.
  reapConnectors();
  dnsApi.stop();
  for (const world of worlds) {
    rmSync(world.scratch, { recursive: true, force: true });
  }
}

console.log(`\n=== ${failures ? `${failures} FAILURE(S)` : "OK"} ===`);
process.exit(failures ? 1 : 0);
