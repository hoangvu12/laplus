// Ticket 28's repro: type a prompt into the composer of a NEW thread and watch
// the pane.
//
// RED  = the pane still says "Working for …" and no assistant reply is drawn.
// GREEN= the pane says "Worked for …" and the reply text is there.
//
//   node tools/ui-driver/repro.mjs [waitSeconds]
//
// Exit code 1 = bug reproduced.
import { writeFileSync } from "node:fs";
import { launch, frameLog, consoleLog } from "./cdp.mjs";

const WAIT = Number(process.argv[2] ?? 30) * 1000;
const PROMPT = "Reply with exactly the word: pong";

const session = await launch({ url: "http://127.0.0.1:4773/" });
const frames = frameLog(session);
const logs = consoleLog(session);
await new Promise((r) => setTimeout(r, 6000));

const focused = await session.evaluate(`
  const el = document.querySelector('[contenteditable="true"]') ?? document.querySelector('textarea');
  if (!el) return 'no composer';
  el.focus();
  return el.tagName + '/' + (el.getAttribute('contenteditable') ?? 'textarea');
`);
console.log("composer:", focused);

await session.send("Input.insertText", { text: PROMPT });
await new Promise((r) => setTimeout(r, 400));
for (const type of ["keyDown", "keyUp"]) {
  await session.send("Input.dispatchKeyEvent", {
    type,
    key: "Enter",
    code: "Enter",
    windowsVirtualKeyCode: 13,
    nativeVirtualKeyCode: 13,
    text: type === "keyDown" ? "\r" : undefined,
  });
}
const sentAt = Date.now();
const frameMark = frames.length;
console.log("submitted at t=0");

/** The pane, not the sidebar: everything after the breadcrumb's "Add action". */
const paneScript = `
  const body = document.body.innerText;
  const at = body.indexOf('Add action');
  return at < 0 ? body : body.slice(at + 'Add action'.length);
`;

let settledAt = null;
const timeline = [];
const deadline = Date.now() + WAIT;
while (Date.now() < deadline) {
  await new Promise((r) => setTimeout(r, 1000));
  const pane = await session.evaluate(paneScript);
  const t = ((Date.now() - sentAt) / 1000).toFixed(0);
  const working = /Working for/.test(pane);
  const worked = /Worked for/.test(pane);
  const replied = /\bpong\b/i.test(pane.replace(PROMPT, ""));
  timeline.push(`t+${t}s working=${working} worked=${worked} reply=${replied}`);
  if (worked && replied) {
    settledAt = Date.now() - sentAt;
    break;
  }
}

const pane = await session.evaluate(paneScript);
const url = await session.evaluate(`return location.href;`);

console.log("=== TIMELINE ===");
console.log(timeline.join("\n"));
console.log("=== PANE ===");
console.log(pane);
console.log("=== URL ===", url);

writeFileSync(
  new URL("./frames.log", import.meta.url),
  frames.map((f, i) => `${i}${i >= frameMark ? " *" : "  "} ${f.dir} ${f.text}`).join("\n"),
);
console.log(`=== ${frames.length} frames written to tools/ui-driver/frames.log (submit at #${frameMark}) ===`);
console.log("=== CONSOLE (tail) ===");
console.log(logs.join("\n").slice(-1500));

const green = settledAt != null;
console.log(
  green
    ? `\nRESULT: GREEN — turn settled and reply rendered after ${(settledAt / 1000).toFixed(1)}s`
    : `\nRESULT: RED — pane never settled/rendered within ${(WAIT / 1000).toFixed(0)}s`,
);
await session.close();
process.exit(green ? 0 : 1);
