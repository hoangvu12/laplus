// Minimal CDP driver: launch headless Chrome, attach to the laplus UI, and
// expose (a) the DOM as text, (b) the console, (c) every WebSocket frame the
// client sends and receives. Ticket 28 needs the third one most.

import { spawn } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

// Whatever Chrome the machine has. Overridable because the default is one
// machine's path, and a driver nobody else can run is not a tool.
const CHROME = process.env.CHROME ?? "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe";

export async function launch({ url, headless = true, keepProfile = null }) {
  const profile = keepProfile ?? mkdtempSync(join(tmpdir(), "lc28-"));
  const port = 9222 + Math.floor(process.pid % 500);
  const chrome = spawn(
    CHROME,
    [
      `--remote-debugging-port=${port}`,
      `--user-data-dir=${profile}`,
      ...(headless ? ["--headless=new"] : []),
      "--no-first-run",
      "--no-default-browser-check",
      "--disable-features=Translate,MediaRouter",
      "--window-size=1600,1000",
      "about:blank",
    ],
    { stdio: "ignore" },
  );

  const target = await poll(async () => {
    const res = await fetch(`http://127.0.0.1:${port}/json/list`).catch(() => null);
    if (!res?.ok) return null;
    const list = await res.json();
    return list.find((t) => t.type === "page") ?? null;
  }, 15000);
  if (!target) throw new Error("chrome never produced a page target");

  const ws = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((ok, bad) => {
    ws.addEventListener("open", ok, { once: true });
    ws.addEventListener("error", bad, { once: true });
  });

  let nextId = 1;
  const pending = new Map();
  const events = [];
  const listeners = [];

  ws.addEventListener("message", (event) => {
    const message = JSON.parse(event.data);
    if (message.id != null) {
      const seat = pending.get(message.id);
      pending.delete(message.id);
      if (message.error) seat.bad(new Error(JSON.stringify(message.error)));
      else seat.ok(message.result);
      return;
    }
    events.push(message);
    for (const listener of listeners) listener(message);
  });

  const send = (method, params = {}) =>
    new Promise((ok, bad) => {
      const id = nextId++;
      pending.set(id, { ok, bad });
      ws.send(JSON.stringify({ id, method, params }));
    });

  const session = {
    send,
    events,
    on: (fn) => listeners.push(fn),
    async evaluate(expression) {
      const result = await send("Runtime.evaluate", {
        expression: `(() => { ${expression} })()`,
        awaitPromise: true,
        returnByValue: true,
      });
      if (result.exceptionDetails) {
        throw new Error(result.exceptionDetails.exception?.description ?? "evaluate threw");
      }
      return result.result.value;
    },
    async close() {
      ws.close();
      chrome.kill();
      await new Promise((r) => setTimeout(r, 300));
      if (!keepProfile) rmSync(profile, { recursive: true, force: true });
    },
  };

  await send("Page.enable");
  await send("Runtime.enable");
  await send("Log.enable");
  await send("Network.enable");
  await send("Page.navigate", { url });
  return session;
}

export function poll(fn, timeoutMs, everyMs = 100) {
  const deadline = Date.now() + timeoutMs;
  return (async function attempt() {
    const value = await fn();
    if (value) return value;
    if (Date.now() > deadline) return null;
    await new Promise((r) => setTimeout(r, everyMs));
    return attempt();
  })();
}

/** Every socket frame, in order, decoded far enough to be readable. */
export function frameLog(session) {
  const frames = [];
  session.on((message) => {
    if (message.method === "Network.webSocketFrameSent") {
      frames.push({ dir: "→", text: message.params.response.payloadData });
    }
    if (message.method === "Network.webSocketFrameReceived") {
      frames.push({ dir: "←", text: message.params.response.payloadData });
    }
    if (message.method === "Network.webSocketCreated") {
      frames.push({ dir: "**", text: `created ${message.params.url}` });
    }
  });
  return frames;
}

/**
 * Every HTTP request Chrome actually put on the wire, **including the preflights
 * the page never sees**, keyed by CDP request id.
 *
 * Read from the protocol rather than from the page, and for CORS work that is
 * not a convenience — it is the only place the answer exists. Ticket 02 of the
 * headless-Linux effort needed both halves:
 *
 * - `Access-Control-Allow-Origin` is **not** a header JavaScript may read. It is
 *   not CORS-safelisted, so `response.headers.get` of it is `null` on a response
 *   that carried it perfectly well. A driver that reports what the page can see
 *   reports `—` either way and proves nothing.
 * - A **preflight is not a request the page made**, so it has no `fetch` to
 *   fail. It is invisible from inside the page even in principle.
 *
 * `asked` is the preflight's own `Access-Control-Request-Headers`, which is how
 * ticket 02 found that the client asks for `b3, traceparent` — and therefore that
 * a header list written from what the server implements would fail the first
 * call with `Allow-Origin: *` present.
 */
export function wireLog(session) {
  const wire = new Map();
  const headerOf = (headers, name) =>
    Object.entries(headers ?? {}).find(([key]) => key.toLowerCase() === name)?.[1];
  session.on((message) => {
    const { requestId, request, response, type } = message.params ?? {};
    if (message.method === "Network.requestWillBeSent") {
      wire.set(requestId, {
        method: request.method,
        url: request.url,
        kind: type,
        asked: headerOf(request.headers, "access-control-request-headers"),
      });
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
  return wire;
}

/** The wire log's calls to one origin, in order, as printable lines. */
export function crossOriginLines(wire, origin) {
  return [...wire.values()]
    .filter((seen) => seen.url.startsWith(origin))
    .map((seen) => {
      const verdict = seen.failed ? `FAILED ${seen.failed}` : String(seen.status ?? "—");
      return (
        `${verdict.padEnd(8)} ${seen.method.padEnd(7)} ${seen.url.replace(origin, "")}` +
        `  allow-origin=${seen.allowOrigin ?? "—"}` +
        (seen.method === "OPTIONS" ? `  asked=${seen.asked ?? "—"}` : "")
      );
    });
}

export function consoleLog(session) {
  const lines = [];
  session.on((message) => {
    if (message.method === "Runtime.consoleAPICalled") {
      lines.push(
        `${message.params.type}: ${message.params.args
          .map((a) => a.value ?? a.description ?? a.type)
          .join(" ")}`,
      );
    }
    if (message.method === "Log.entryAdded") {
      lines.push(`${message.params.entry.level}: ${message.params.entry.text}`);
    }
    if (message.method === "Runtime.exceptionThrown") {
      lines.push(
        `exception: ${message.params.exceptionDetails.exception?.description ?? message.params.exceptionDetails.text}`,
      );
    }
  });
  return lines;
}
