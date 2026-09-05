// Windows-only: actual Tauri WebView2 callbacks -> OS default browser.
// Isolated app data/profile; never attaches to or stops the installed laplus.
import { spawn } from "node:child_process";
import { mkdtempSync, mkdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { createServer } from "node:http";
import { createServer as createTcpServer } from "node:net";

const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
async function freePort() {
  const socket = createTcpServer();
  await new Promise((resolve) => socket.listen(0, "127.0.0.1", resolve));
  const port = socket.address().port;
  await new Promise((resolve) => socket.close(resolve));
  return port;
}
async function until(read, description) {
  for (let attempt = 0; attempt < 150; attempt++) {
    const value = await read();
    if (value) return value;
    await delay(200);
  }
  throw new Error(`Timed out: ${description}`);
}

const root = mkdtempSync(join(tmpdir(), "laplus-native-link-"));
mkdirSync(join(root, "webview"));
const debugPort = await freePort();
const appPort = await freePort();
const requests = [];
const fixture = createServer((request, response) => {
  if (request.url.startsWith("/native-link/")) {
    requests.push({ path: request.url, userAgent: request.headers["user-agent"] });
  }
  response.writeHead(200, { "content-type": "text/html", "cache-control": "no-store" });
  response.end(
    "<!doctype html><title>laplus link verification</title><p>Link opened successfully. This verification tab can be closed.</p><script>setTimeout(()=>window.close(),250)</script>",
  );
});
await new Promise((resolve) => fixture.listen(0, "127.0.0.1", resolve));
const fixtureOrigin = `http://127.0.0.1:${fixture.address().port}`;
const shell = spawn(resolve("server/target/debug/laplus.exe"), ["--port", String(appPort)], {
  windowsHide: true,
  stdio: "ignore",
  env: {
    ...process.env,
    LOCALAPPDATA: root,
    APPDATA: root,
    WEBVIEW2_USER_DATA_FOLDER: join(root, "webview"),
    WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${debugPort}`,
  },
});
let ws;
try {
  const target = await until(async () => {
    const response = await fetch(`http://127.0.0.1:${debugPort}/json/list`).catch(() => null);
    if (!response?.ok) return null;
    return (await response.json()).find(
      (item) => item.type === "page" && item.url.startsWith(`http://127.0.0.1:${appPort}/`),
    );
  }, "isolated WebView2 page");
  ws = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => {
    ws.addEventListener("open", resolve, { once: true });
    ws.addEventListener("error", reject, { once: true });
  });
  const pending = new Map();
  let nextId = 1;
  ws.addEventListener("message", (event) => {
    const data = JSON.parse(event.data);
    const waiter = pending.get(data.id);
    if (!waiter) return;
    pending.delete(data.id);
    if (data.error) waiter.reject(new Error(JSON.stringify(data.error)));
    else waiter.resolve(data.result);
  });
  const send = (method, params = {}) =>
    new Promise((resolve, reject) => {
      const id = nextId++;
      pending.set(id, { resolve, reject });
      ws.send(JSON.stringify({ id, method, params }));
    });
  const evaluate = async (expression) => {
    const result = await send("Runtime.evaluate", {
      expression,
      returnByValue: true,
      awaitPromise: true,
      userGesture: true,
    });
    if (result.exceptionDetails) throw new Error(result.exceptionDetails.text);
    return result.result.value;
  };
  await until(
    () => evaluate("document.readyState === 'complete' && !!window.__TAURI_INTERNALS__"),
    "Tauri document",
  );
  const shellUserAgent = await evaluate("navigator.userAgent");
  for (const kind of ["blank", "navigation"]) {
    const url = `${fixtureOrigin}/native-link/${kind}?first=one&second=two%20words`;
    await evaluate(
      `(() => { const a = document.createElement('a'); a.href = ${JSON.stringify(url)}; a.textContent = 'Verify native browser'; ${kind === "blank" ? "a.target = '_blank';" : ""} document.body.append(a); a.click(); a.remove(); })()`,
    );
    await until(
      () =>
        requests.find(
          (request) =>
            request.path.startsWith(`/native-link/${kind}?`) &&
            request.userAgent !== shellUserAgent,
        ),
      `${kind} callback reaches external browser`,
    );
    const currentOrigin = await evaluate("location.origin");
    if (currentOrigin !== `http://127.0.0.1:${appPort}`)
      throw new Error("The shell navigated away instead of dispatching externally");
  }
  console.log(
    JSON.stringify(
      {
        result: "PASS",
        shellPid: shell.pid,
        isolatedDataDirectory: root,
        shellUserAgent,
        requests,
        shellKeptItsOrigin: true,
      },
      null,
      2,
    ),
  );
  // The page can disappear before CDP acknowledges close; never await that reply.
  void evaluate(
    "window.__TAURI_INTERNALS__.invoke('plugin:window|close', {label: 'main'}).catch(() => {})",
  ).catch(() => {});
} finally {
  ws?.close();
  if (shell.exitCode === null) {
    await Promise.race([new Promise((resolve) => shell.once("exit", resolve)), delay(2000)]);
  }
  if (shell.exitCode === null) shell.kill();
  fixture.closeAllConnections();
  await new Promise((resolve) => fixture.close(resolve));
  // Retain isolated diagnostic data; cleanup can safely target the printed path.
}
