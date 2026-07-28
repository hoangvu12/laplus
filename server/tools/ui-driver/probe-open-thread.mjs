// Ticket 28's cheapest discriminator: open the conversation that is already in
// the registry and see whether the thread view draws its four messages.
import { launch, frameLog, consoleLog } from "./cdp.mjs";

const THREAD = "0ea19ef1-d1d2-4745-875b-a4c0ef996950";
const session = await launch({ url: "http://127.0.0.1:4773/" });
const frames = frameLog(session);
const logs = consoleLog(session);

await new Promise((r) => setTimeout(r, 6000));

const clicked = await session.evaluate(`
  const rows = [...document.querySelectorAll('a,button,[role="button"],[role="option"],li,div')];
  const hit = rows.filter((el) => (el.innerText ?? '').trim().startsWith('Hey') && el.getClientRects().length)
                  .sort((a, b) => (a.innerText.length - b.innerText.length))[0];
  if (!hit) return 'no row';
  hit.scrollIntoView();
  hit.click();
  return hit.tagName + ' | ' + hit.innerText.slice(0, 80).replace(/\\n/g, ' / ');
`);
console.log("clicked:", clicked);

await new Promise((r) => setTimeout(r, 4000));

console.log("=== BODY TEXT ===");
console.log(await session.evaluate(`return document.body.innerText;`));
console.log("=== URL ===", await session.evaluate(`return location.href;`));
console.log("=== FRAMES after click ===");
for (const f of frames.filter((f) => f.text.includes("subscribeThread") || f.text.includes("snapshot"))) {
  console.log(f.dir, f.text.slice(0, 240));
}
console.log("=== CONSOLE ===");
console.log(logs.join("\n").slice(0, 2000));

await session.close();
