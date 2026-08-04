// Drive a real subagent through the window and watch two things at once:
// whether the developer can SEE it working, and whether the composer says the
// turn is over while it still is.
//
//   CHROME=/usr/bin/chromium-browser node tools/ui-driver/probe-subagent.mjs \
//     "http://127.0.0.1:4790/#token=..." [waitSeconds]
//
// Reports, per second: what the pane says about the turn ("Working for" /
// "Worked for"), and every subagent row visible in it. Then prints the subagent
// activities that actually crossed the socket, which is the difference between
// "the server never said it" and "the client never drew it".
//
// Exit 1 if no subagent row was ever visible.
import { launch, frameLog } from "./cdp.mjs";

const URL = process.argv[2] ?? "http://127.0.0.1:4773/";
const WAIT = Number(process.argv[3] ?? 90) * 1000;

// Deliberately harmless: the subagent sleeps and says a word. This drives a real
// agent in a real checkout, so the prompt must not be able to change anything.
const PROMPT =
  "Spawn a subagent in the background with run_in_background: true. " +
  "Its entire job is to run the shell command `sleep 25` and then reply with the " +
  "single word done. Do not read, write or edit any files, and do not run any " +
  "other command. As soon as you have launched it, reply to me immediately with " +
  "one short sentence and end your turn without waiting for it.";

const session = await launch({ url: URL });
const frames = frameLog(session);
await new Promise((r) => setTimeout(r, 7000));

const focused = await session.evaluate(`
  const el = document.querySelector('[contenteditable="true"]') ?? document.querySelector('textarea');
  if (!el) return 'no composer';
  el.focus();
  return 'ok';
`);
if (focused !== "ok") {
  console.error("composer:", focused);
  process.exit(2);
}

await session.send("Input.insertText", { text: PROMPT });
await new Promise((r) => setTimeout(r, 500));
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
console.log("submitted at t=0\n");

const paneScript = `
  const body = document.body.innerText;
  const at = body.indexOf('Add action');
  return at < 0 ? body : body.slice(at + 'Add action'.length);
`;

let sawSubagentRow = false;
let idleWhileRunning = null;
const deadline = Date.now() + WAIT;
let last = "";
while (Date.now() < deadline) {
  await new Promise((r) => setTimeout(r, 1000));
  const pane = await session.evaluate(paneScript);
  const t = ((Date.now() - sentAt) / 1000).toFixed(0).padStart(3);
  const working = /Working for/.test(pane);
  const worked = /Worked for/.test(pane);
  // A work-log row *heading*, not any line mentioning the word — the prompt
  // itself says "subagent" several times, and matching that reported success
  // for a row that was never drawn.
  const rows = pane
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => /^Subagent\b/i.test(line));
  if (rows.length) sawSubagentRow = true;

  const state = working ? "WORKING" : worked ? "worked " : "  ?    ";
  const line = `${state} | rows=${rows.length ? rows.join(" ¦ ") : "(none)"}`;
  if (line !== last) {
    console.log(`t=${t}s ${line}`);
    last = line;
  }

  // The symptom: the turn reads as finished while a subagent is still going.
  // "Still going" is the absence of a terminal row rather than the presence of
  // the word running, because a finished row is what the UI tells us with.
  if (
    worked &&
    !working &&
    rows.length &&
    !rows.some((row) => /✓|done|completed|failed/i.test(row))
  ) {
    idleWhileRunning ??= Number(t);
  }
}

console.log("\n=== the pane, at the end ===");
const finalPane = await session.evaluate(paneScript);
console.log(
  finalPane
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean)
    .slice(-40)
    .join("\n"),
);

console.log("\n=== subagent activities that crossed the socket ===");
const seen = new Set();
for (const frame of frames) {
  const text = typeof frame === "string" ? frame : (frame.text ?? JSON.stringify(frame));
  if (!/collab_agent_tool_call|taskId|subagent/i.test(text)) continue;
  for (const match of text.matchAll(
    /"status":"(\w+)"[^}]*?"title":"([^"]{0,60})"|"title":"([^"]{0,60})"[^}]*?"status":"(\w+)"/g,
  )) {
    const key = match[0].slice(0, 90);
    if (seen.has(key)) continue;
    seen.add(key);
    console.log("  ", key);
  }
}
if (seen.size === 0) console.log("   (none — the server never published one)");

console.log("\n=== verdict ===");
console.log("subagent row ever visible:", sawSubagentRow);
console.log(
  "composer read finished while a subagent still ran:",
  idleWhileRunning === null ? "no" : `yes, from t=${idleWhileRunning}s`,
);

await session.close?.();
process.exit(sawSubagentRow ? 0 : 1);
