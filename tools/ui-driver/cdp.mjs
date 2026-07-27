// Minimal CDP driver: launch headless Chrome, attach to the laplus UI, and
// expose (a) the DOM as text, (b) the console, (c) every WebSocket frame the
// client sends and receives. Ticket 28 needs the third one most.

import { spawn } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

// Whatever Chrome the machine has. Overridable because the default is one
// machine's path, and a driver nobody else can run is not a tool.
const CHROME =
  process.env.CHROME ?? "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe";

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
