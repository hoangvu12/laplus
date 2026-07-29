// Ticket 02 of the headless-Linux effort, driven: a page served by one laplus,
// pairing with a *second* laplus on another origin.
//
// This is the acceptance criterion no test in `crates/laplus-server/tests` can
// reach. Those drive the server's own answers; the thing that had to be proven
// here is that a **real browser** accepts them — that the preflight is answered
// in a shape Chrome's CORS implementation is satisfied by, and that the response
// therefore reaches the page instead of becoming "could not reach the backend"
// for a server that answered fine.
//
// It walks the chain `preparePairingRegistration` walks, in the same order, with
// the same request shapes (`packages/client-runtime/src/authorization/remote.ts`
// and `connection/onboarding.ts`) — and then opens the socket, which is where
// the remote path stops being HTTP.
//
//   node tools/ui-driver/remote-pairing.mjs <page-url> <remote-url> <pairing-code>
//
// Two servers, each with a profile of its own, is the whole setup:
//
//   LOCALAPPDATA=…\lc-a laplus-server.exe --ui apps/web/dist --port 5773
//   LOCALAPPDATA=…\lc-b laplus-server.exe --ui apps/web/dist --port 5774
//
// The pairing code is the one the *remote* printed at startup. A different port
// is a different origin, which is all a browser means by cross-origin — so this
// exercises the same code path a desktop window on one machine and a laplus on
// another do, without needing the second machine.
import { launch, consoleLog, poll } from "./cdp.mjs";

const page = process.argv[2] ?? "http://127.0.0.1:5773/";
const remote = process.argv[3] ?? "http://127.0.0.1:5774";
const code = process.argv[4];
if (!code) {
  console.error("usage: remote-pairing.mjs <page-url> <remote-url> <pairing-code>");
  process.exit(2);
}

const session = await launch({ url: page });
const logs = consoleLog(session);

// Every request Chrome actually put on the wire, **including the preflights the
// page never sees**, read from the DevTools protocol rather than from the page.
//
// This is not the same information as the `fetch` results below and the
// difference is the point twice over. `Access-Control-Allow-Origin` is not a
// header JavaScript may read — it is not CORS-safelisted, so
// `response.headers.get` of it is `null` on a response that carried it
// perfectly well — and a preflight is not a request the page made, so it has no
// `fetch` to fail. Both are only visible here.
const wire = new Map();
const headerOf = (headers, name) =>
  Object.entries(headers ?? {}).find(([key]) => key.toLowerCase() === name)?.[1];
session.on((message) => {
  const { requestId, request, response, type } = message.params ?? {};
  if (message.method === "Network.requestWillBeSent") {
    wire.set(requestId, { method: request.method, url: request.url, kind: type });
  }
  const seen = wire.get(requestId);
  if (seen && message.method === "Network.responseReceived") {
    seen.status = response.status;
    seen.allowOrigin = headerOf(response.headers, "access-control-allow-origin");
    seen.allowHeaders = headerOf(response.headers, "access-control-allow-headers");
  }
  if (seen && message.method === "Network.loadingFailed") {
    seen.failed = message.params.corsErrorStatus?.corsError ?? message.params.errorText;
  }
});

const ready = await poll(
  () => session.evaluate(`return document.readyState === "complete" ? "yes" : null;`),
  20000,
);
if (!ready) {
  console.error("the page never finished loading — is the page server up?");
  process.exit(1);
}

// The chain, in the page, so that every request carries the page's origin and is
// subject to the browser's own enforcement. Each step's output is the next
// step's input: nothing here is arranged, so a step that reports a status has
// genuinely been through the one before it.
const walked = await session.evaluate(`
  const remote = ${JSON.stringify(remote)};
  const code = ${JSON.stringify(code)};
  const steps = [];
  const note = async (what, response) => {
    const body = await response.text();
    steps.push({ what, status: response.status, body: body.slice(0, 400) });
    try {
      return JSON.parse(body);
    } catch {
      return null;
    }
  };

  return (async () => {
    try {
      // 1. fetchRemoteEnvironmentDescriptor. The call the whole attempt used to
      //    die at, and a simple request — no preflight, just an unreadable answer.
      const descriptor = await note(
        "GET /.well-known/t3/environment",
        await fetch(remote + "/.well-known/t3/environment"),
      );

      // 2. bootstrapRemoteBearerSession. Form-encoded because RFC 6749 says so.
      const granted = await note(
        "POST /oauth/token",
        await fetch(remote + "/oauth/token", {
          method: "POST",
          headers: { "content-type": "application/x-www-form-urlencoded" },
          body: new URLSearchParams({
            grant_type: "urn:ietf:params:oauth:grant-type:token-exchange",
            subject_token: code,
            subject_token_type: "urn:t3:params:oauth:token-type:environment-bootstrap",
            requested_token_type: "urn:ietf:params:oauth:token-type:access_token",
          }),
        }),
      );
      const bearer = granted?.access_token;
      if (!bearer) return { steps, error: "the exchange returned no access_token" };

      // 3. fetchRemoteSessionState. The first call carrying an Authorization
      //    header, so the first that Chrome will not send without a preflight.
      await note(
        "GET /api/auth/session",
        await fetch(remote + "/api/auth/session", {
          headers: { authorization: "Bearer " + bearer },
        }),
      );

      // 4. issueRemoteWebSocketTicket.
      const ticketed = await note(
        "POST /api/auth/websocket-ticket",
        await fetch(remote + "/api/auth/websocket-ticket", {
          method: "POST",
          headers: { authorization: "Bearer " + bearer },
        }),
      );

      // 5. The snapshot the shell reads on every load, over the same bearer.
      await note(
        "GET /api/orchestration/shell",
        await fetch(remote + "/api/orchestration/shell", {
          headers: { authorization: "Bearer " + bearer },
        }),
      );

      // 6. And then the socket, which CORS does not govern at all — the ticket
      //    rides in the query string precisely because a browser cannot put a
      //    header on an upgrade. A real request over it, so that "open" means
      //    the server answered rather than that TCP connected.
      const ticket = ticketed?.ticket ?? ticketed?.wsTicket ?? ticketed?.token;
      if (!ticket) return { steps, error: "no ticket in " + JSON.stringify(ticketed) };
      const wsUrl = remote.replace(/^http/, "ws") + "/ws?wsTicket=" + encodeURIComponent(ticket);
      const answered = await new Promise((resolve) => {
        const socket = new WebSocket(wsUrl);
        const done = setTimeout(() => resolve("no answer within 10s"), 10000);
        socket.onopen = () =>
          socket.send(
            JSON.stringify({ _tag: "Request", id: "0", tag: "server.getConfig", payload: {}, headers: [] }),
          );
        socket.onmessage = (event) => {
          clearTimeout(done);
          socket.close();
          resolve(String(event.data).slice(0, 200));
        };
        socket.onerror = () => {
          clearTimeout(done);
          resolve("the socket refused the ticket");
        };
      });
      steps.push({ what: "WS /ws?wsTicket=…", status: "open", body: answered });

      return { steps, environmentId: descriptor?.environmentId };
    } catch (error) {
      return { steps, error: String(error) };
    }
  })();
`);

console.log(`=== what the page got back, page ${page} → remote ${remote} ===`);
for (const step of walked.steps) {
  console.log(`${String(step.status).padEnd(6)} ${step.what}`);
  if (step.body) console.log(`       ${step.body.replace(/\n/g, " ")}`);
}

// Only the calls to the remote. The page's own requests to the server it was
// loaded from are same-origin and have nothing to say here.
const crossOrigin = [...wire.values()].filter((seen) => seen.url.startsWith(remote));
console.log("=== what chrome put on the wire ===");
for (const seen of crossOrigin) {
  const verdict = seen.failed ? `FAILED ${seen.failed}` : String(seen.status);
  console.log(
    `${verdict.padEnd(8)} ${seen.method.padEnd(7)} ${seen.url.replace(remote, "")}` +
      `  allow-origin=${seen.allowOrigin ?? "—"}` +
      (seen.method === "OPTIONS" ? `  allow-headers=${seen.allowHeaders ?? "—"}` : ""),
  );
}
const preflights = crossOrigin.filter((seen) => seen.method === "OPTIONS");
console.log(
  `${preflights.length} preflight(s), ${preflights.filter((f) => f.failed).length} refused`,
);

const corsErrors = logs.filter((line) => /CORS|Access-Control|Failed to fetch/i.test(line));
if (corsErrors.length) {
  console.log("=== console, CORS ===");
  console.log(corsErrors.join("\n"));
}

await session.close();

const refused = walked.steps.filter((step) => Number(step.status) >= 400);
const failedFlights = crossOrigin.filter((seen) => seen.failed);
// Every reason, not the first: a refused preflight and a refused request are
// different faults, and "FAILED:" with nothing after it is what an earlier
// version of this printed when the failure was not the page's own exception.
const why = [
  walked.error,
  ...refused.map((step) => `${step.what} answered ${step.status}`),
  ...failedFlights.map((flight) => `${flight.method} ${flight.url} — ${flight.failed}`),
  ...corsErrors,
].filter(Boolean);
if (why.length) {
  console.error(`\nFAILED:\n  ${why.join("\n  ")}`);
  process.exit(1);
}
console.log(`\nOK — paired with ${walked.environmentId} from another origin.`);
