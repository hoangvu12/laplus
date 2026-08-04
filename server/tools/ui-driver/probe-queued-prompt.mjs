// Send a prompt while the agent is working, and watch what the composer says.
//
//   CHROME=/usr/bin/chromium-browser node tools/ui-driver/probe-queued-prompt.mjs \
//     "http://127.0.0.1:4791/#token=..." [waitSeconds]
//
// The bug: typing a second message while a turn is running flipped the pane from
// "working" to **"connecting"**, and the reply stopped rendering. The server
// published `Session { status: Starting, activeTurnId: <the queued turn> }` on a
// conversation that was mid-turn; the client draws `starting` as "connecting"
// (`session-logic.ts`, `derivePhase`) and stops treating the running turn as
// running (`threadReducer.ts`, `turnStillRunning`).
//
// So this samples, twice a second, the two things that would show it:
//
// - every control the composer is offering, by `aria-label` — the send button
//   becomes a *stop* button while a turn runs, and a spinner labelled
//   "Connecting" is the symptom;
// - whether the pane still says "Working for", and whether the assistant's text
//   is still growing.
//
// Spends **two real agent turns** against the configured `claude`. Both prompts
// are deliberately incapable of changing the checkout: the first only counts,
// the second only says a word.
//
// Exit 1 if the composer ever reported "Connecting" after the second prompt, or
// if the reply stopped growing across the queue.
import { launch } from "./cdp.mjs";

const URL = process.argv[2] ?? "http://127.0.0.1:4791/";
const WAIT = Number(process.argv[3] ?? 75) * 1000;

// Long enough to still be running when the second prompt lands, and unable to
// touch anything: no tool is needed to count.
const FIRST =
  "Without using any tools at all, count from 1 to 60. Put each number on its " +
  "own line with a short word after it. Do not read, write or run anything.";
const SECOND = "Stop counting. Just reply with the single word: acknowledged.";

const session = await launch({ url: URL });
await new Promise((r) => setTimeout(r, 8000));

const type = async (text) => {
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
  await session.send("Input.insertText", { text });
  await new Promise((r) => setTimeout(r, 400));
  for (const kind of ["keyDown", "keyUp"]) {
    await session.send("Input.dispatchKeyEvent", {
      type: kind,
      key: "Enter",
      code: "Enter",
      windowsVirtualKeyCode: 13,
      nativeVirtualKeyCode: 13,
      text: kind === "keyDown" ? "\r" : undefined,
    });
  }
};

// The composer's own controls, not the whole page: the send/stop button is the
// one that carries the phase, and the topbar has buttons of its own.
const READ = `
  const form = document.querySelector('form') ?? document.body;
  const labels = [...form.querySelectorAll('button')]
    .map((b) => b.getAttribute('aria-label') ?? b.innerText.trim())
    .filter(Boolean);
  const body = document.body.innerText;
  return JSON.stringify({
    labels,
    working: /Working for/.test(body),
    worked: /Worked for/.test(body),
    connecting: /Connecting/i.test(body),
    length: body.length,
  });
`;

const read = async () => JSON.parse(await session.evaluate(READ));

await type(FIRST);
const sentAt = Date.now();
console.log("first prompt submitted at t=0\n");

const at = () => ((Date.now() - sentAt) / 1000).toFixed(1).padStart(5);
const sample = [];
let last = "";
const note = (row, mark = " ") => {
  const line = `${mark} labels=[${row.labels.join(" ¦ ")}] working=${row.working} connecting=${row.connecting} len=${row.length}`;
  if (line !== last) {
    console.log(`t=${at()}s ${line}`);
    last = line;
  }
  sample.push({ t: Number(at()), ...row });
};

// Wait until the agent is demonstrably mid-turn — the pane says so and its text
// is growing — because a second prompt sent before the first turn started would
// be a test of something else entirely.
let queuedAt = null;
let lengthAtQueue = 0;
const deadline = Date.now() + WAIT;
let growth = [];
while (Date.now() < deadline) {
  await new Promise((r) => setTimeout(r, 500));
  const row = await read();
  note(row);
  growth.push(row.length);
  if (
    queuedAt === null &&
    row.working &&
    growth.length > 4 &&
    row.length > growth[growth.length - 4]
  ) {
    lengthAtQueue = row.length;
    queuedAt = Date.now();
    console.log(`\n--- second prompt submitted while the first turn is running ---\n`);
    await type(SECOND);
  }
}

if (queuedAt === null) {
  console.error("\nthe first turn never got going, so nothing was queued mid-turn");
  process.exit(2);
}

const after = sample.filter((row) => row.t >= (queuedAt - sentAt) / 1000);
const saidConnecting = after.filter((row) => row.connecting || row.labels.includes("Connecting"));
const grew = after.some((row) => row.length > lengthAtQueue);
const keptWorking = after.filter((row) => row.working).length;

console.log("\n=== after the second prompt was sent mid-turn ===");
console.log(`samples:            ${after.length}`);
console.log(`said "Connecting":  ${saidConnecting.length}`);
console.log(`said "Working for": ${keptWorking}`);
console.log(`the reply kept growing: ${grew}`);

console.log("\n=== the pane, at the end ===");
const pane = await session.evaluate(`return document.body.innerText`);
console.log(
  pane
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean)
    .slice(-30)
    .join("\n"),
);

if (saidConnecting.length > 0) {
  console.error(`\nFAIL: the composer said "Connecting" on a conversation that was working`);
  process.exit(1);
}
if (!grew) {
  console.error(`\nFAIL: the reply stopped rendering once the prompt was queued`);
  process.exit(1);
}
console.log(`\nOK: stayed working, kept streaming`);
await session.close?.();
process.exit(0);
