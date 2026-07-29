// Does the composer's context meter fill after a turn?
//
// RED  = no meter button in the composer at all. The client renders it only when
//        `deriveLatestContextWindowSnapshot` finds a `context-window.updated`
//        activity (`ChatComposer.tsx`), so its *absence* is the whole symptom:
//        a server that never emits the row leaves a composer with no meter,
//        which reads as a UI that does not have the feature.
// GREEN= the button is there and its aria-label carries a percentage.
//
//   node tools/ui-driver/probe-context-meter.mjs [url] [waitSeconds]
//
// Spends a **real agent turn**, like `repro.mjs`. Exit code 1 = no meter.
import { writeFileSync } from "node:fs";
import { launch, frameLog } from "./cdp.mjs";

const URL_ARG = process.argv[2] ?? "http://127.0.0.1:4773/";
const WAIT = Number(process.argv[3] ?? 60) * 1000;
const PROMPT = "Reply with exactly the word: pong";

const session = await launch({ url: URL_ARG });
const frames = frameLog(session);
await new Promise((r) => setTimeout(r, 6000));

const focused = await session.evaluate(`
  const el = document.querySelector('[contenteditable="true"]') ?? document.querySelector('textarea');
  if (!el) return 'no composer';
  el.focus();
  return el.tagName + '/' + (el.getAttribute('contenteditable') ?? 'textarea');
`);
console.log("composer:", focused);

// The meter before the turn, so a reading that was already there is not mistaken
// for one this turn produced.
const meterScript = `
  const el = document.querySelector('button[aria-label^="Context window"]');
  return el ? el.getAttribute('aria-label') : null;
`;
console.log("meter before:", await session.evaluate(meterScript));

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
console.log("submitted at t=0");

let label = null;
const timeline = [];
const deadline = Date.now() + WAIT;
while (Date.now() < deadline) {
  await new Promise((r) => setTimeout(r, 1000));
  const t = ((Date.now() - sentAt) / 1000).toFixed(0);
  label = await session.evaluate(meterScript);
  const rowOnTheWire = frames.some((f) => f.text.includes("context-window.updated"));
  timeline.push(`t+${t}s wire=${rowOnTheWire} meter=${JSON.stringify(label)}`);
  if (label) break;
}

// The tooltip is where the numbers are; the button only carries the percentage.
const details = label
  ? await session.evaluate(`
      const el = document.querySelector('button[aria-label^="Context window"]');
      el.dispatchEvent(new PointerEvent('pointerenter', { bubbles: true }));
      el.click();
      return 'opened';
    `)
  : "no meter to open";
await new Promise((r) => setTimeout(r, 800));
const tooltip = label
  ? await session.evaluate(`
      const heading = [...document.querySelectorAll('div')].find((d) => d.textContent === 'Context Window');
      return heading ? heading.parentElement.parentElement.innerText : 'no tooltip';
    `)
  : null;

console.log("=== TIMELINE ===");
console.log(timeline.join("\n"));
console.log("=== METER ===", label);
console.log("=== TOOLTIP ===", details, "\n" + (tooltip ?? ""));

const row = frames.find((f) => f.text.includes("context-window.updated"));
console.log("=== THE ROW ON THE WIRE ===");
console.log(row ? row.text.slice(0, 600) : "never arrived");

writeFileSync(
  new URL("./frames.log", import.meta.url),
  frames.map((f, i) => `${i} ${f.dir} ${f.text}`).join("\n"),
);

await session.close();
process.exit(label ? 0 : 1);
