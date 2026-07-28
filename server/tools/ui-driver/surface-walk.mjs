// Walk the application's surfaces and report which controls the server refuses.
//
// The other probes here each chase one ticket. This one asks the question no
// server-side reading can answer: *of everything the UI offers, what does
// nothing?* It navigates each route, enumerates the controls it finds, and
// watches the socket for the two shapes of refusal — "Method not implemented by
// this server" from an unimplemented method, and a command error naming a
// command this server does not parse.
//
// It spends no agent turn. Nothing here submits a prompt.
//
// Usage: node tools/ui-driver/surface-walk.mjs [url]
import { launch, frameLog, consoleLog } from "./cdp.mjs";

const URL = process.argv[2] ?? "http://127.0.0.1:4773/";

const ROUTES = [
  "/",
  "/settings/general",
  "/settings/providers",
  "/settings/keybindings",
  "/settings/archived",
  "/settings/connections",
  "/settings/diagnostics",
  "/settings/source-control",
  "/settings/beta",
];

const session = await launch({ url: URL });
const frames = frameLog(session);
const logs = consoleLog(session);

const settle = (ms) => new Promise((r) => setTimeout(r, ms));

// A refusal is a frame carrying one of the server's two "no" shapes. Matched on
// the wire text rather than decoded: the envelope is Effect-RPC's and the point
// here is which method was named, not the framing around it.
//
// The *sentence* is what this keys on, not the `_tag`. Since ticket 39 a method
// the contract declares is refused under a tag that method declares — so the tag
// is `EnvironmentAuthorizationError` for most of them, and matching on it would
// catch real authorization failures too. The sentence is the part that did not
// change, and it names the method in both shapes.
function refusals(from = 0) {
  const found = [];
  for (const frame of frames.slice(from)) {
    if (frame.dir !== "←") continue;
    const text = frame.text;
    if (!text.includes("not implemented by this server")) continue;
    const method = /Method not implemented by this server: ([\w.\-]+)/.exec(text)?.[1];
    const command = /Command not implemented by this server: ([a-z.\-]+)/.exec(text)?.[1];
    found.push(method ?? command ?? text.slice(0, 160));
  }
  return found;
}

const report = [];

for (const route of ROUTES) {
  const before = frames.length;
  const logsBefore = logs.length;
  await session.evaluate(`history.pushState({}, "", ${JSON.stringify(route)});
    window.dispatchEvent(new PopStateEvent("popstate"));
    return null;`);
  await settle(2500);

  const snapshot = await session.evaluate(`
    const text = document.body.innerText || "";
    const controls = [...document.querySelectorAll('button, [role="button"], a[href], input, select')]
      .filter((el) => el.offsetParent !== null)
      .map((el) => (el.getAttribute("aria-label") || el.innerText || el.getAttribute("placeholder") || el.tagName).trim())
      .filter((label) => label.length > 0 && label.length < 60);
    return JSON.stringify({ text: text.slice(0, 1200), controls: [...new Set(controls)] });
  `);

  const parsed = JSON.parse(snapshot ?? '{"text":"","controls":[]}');
  const errs = logs.slice(logsBefore).filter((l) => /error|Error/.test(l));

  report.push({
    route,
    empty: parsed.text.trim().length < 40,
    text: parsed.text,
    controls: parsed.controls,
    refused: refusals(before),
    consoleErrors: errs.slice(0, 6),
  });
}

console.log("================ SURFACE WALK ================");
for (const entry of report) {
  console.log(`\n--- ${entry.route} ---`);
  console.log(`controls (${entry.controls.length}): ${entry.controls.slice(0, 24).join(" | ")}`);
  if (entry.empty) console.log("!! RENDERS (near-)EMPTY");
  if (entry.refused.length) console.log(`!! REFUSED: ${[...new Set(entry.refused)].join(", ")}`);
  if (entry.consoleErrors.length) {
    console.log(`!! CONSOLE: ${entry.consoleErrors.map((e) => e.slice(0, 140)).join(" // ")}`);
  }
  console.log(`text: ${entry.text.replace(/\n+/g, " / ").slice(0, 420)}`);
}

console.log("\n================ ALL REFUSALS ON THE SOCKET ================");
console.log([...new Set(refusals())].join("\n") || "(none)");

console.log("\n================ METHODS CALLED ================");
const called = new Set();
for (const frame of frames) {
  if (frame.dir !== "→") continue;
  for (const m of frame.text.matchAll(/"_tag":"([a-zA-Z]+\.[a-zA-Z]+)"/g)) called.add(m[1]);
  for (const m of frame.text.matchAll(/"tag":"([a-zA-Z]+\.[a-zA-Z]+)"/g)) called.add(m[1]);
}
console.log([...called].sort().join("\n") || "(none seen)");

await session.close();
