// What the pane shows in the seconds after the FIRST message of a fresh
// conversation — the window a developer reports as "nothing happens".
//
//   node tools/ui-driver/first-turn.mjs [url] [waitSeconds]
//
// Samples four times a second, because the whole question is about a gap that
// `repro.mjs`'s one-second polling steps straight over.
import { launch, frameLog } from "./cdp.mjs";

const URL_ = process.argv[2] ?? "http://127.0.0.1:4773/";
const WAIT = Number(process.argv[3] ?? 12) * 1000;
const PROMPT = "Reply with exactly the word: pong";

const session = await launch({ url: URL_ });
const frames = frameLog(session);
await new Promise((r) => setTimeout(r, 6000));

const focused = await session.evaluate(`
  const el = document.querySelector('[contenteditable="true"]') ?? document.querySelector('textarea');
  if (!el) return 'no composer';
  el.focus();
  return 'ok';
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

const paneScript = `
  const body = document.body.innerText;
  const at = body.indexOf('Add action');
  return at < 0 ? body : body.slice(at + 'Add action'.length);
`;

console.log("t(ms)  prompt working worked spinner reply  | first line of pane");
const deadline = Date.now() + WAIT;
let last = "";
while (Date.now() < deadline) {
  const pane = await session.evaluate(paneScript);
  const spinner = await session.evaluate(
    `return document.querySelectorAll('.animate-spin, [data-loading], [aria-busy="true"]').length;`,
  );
  const where = await session.evaluate("return location.hash || location.pathname;");
  const t = String(Date.now() - sentAt).padStart(5);
  const row = [
    /Reply with exactly/.test(pane),
    /Working for/.test(pane),
    /Worked for/.test(pane),
    spinner,
    /\bpong\b/i.test(pane.replace(PROMPT, "")),
  ].join(" ");
  const head = pane
    .split("\n")
    .map((l) => l.trim())
    .filter(Boolean)
    .slice(0, 3)
    .join(" | ")
    .slice(0, 90);
  if (row + head !== last) {
    console.log(`${t}  ${row}  | ${head}`);
    last = row + head;
  }
  await new Promise((r) => setTimeout(r, 250));
}

console.log("=== socket frames since submit ===");
for (const f of frames.slice(frameMark).slice(0, 40)) {
  console.log(f.dir, f.text.slice(0, 150));
}
await session.close();
process.exit(0);
