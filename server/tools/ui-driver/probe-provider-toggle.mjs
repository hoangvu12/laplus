// What the OpenCode card's enable toggle actually sends, and what the server
// answers. Takes the URL (with `#token=`) as an argument.
import { launch, frameLog } from "./cdp.mjs";

const url = process.argv[2] ?? "http://127.0.0.1:4773/";
const session = await launch({ url });
const frames = frameLog(session);

await new Promise((r) => setTimeout(r, 8000));

await session.evaluate(
  `window.history.pushState({}, "", "/settings/providers"); window.dispatchEvent(new PopStateEvent("popstate")); return true;`,
);
await new Promise((r) => setTimeout(r, 4000));

const switches = await session.evaluate(`
  return [...document.querySelectorAll('[role="switch"]')].map((el, index) => ({
    index,
    label: (el.getAttribute('aria-label') || el.innerText || '').trim().slice(0, 80),
    checked: el.getAttribute('aria-checked'),
    card: (el.closest('[data-provider-instance-card], section, li, div.rounded-lg')?.innerText ?? '').slice(0, 70).replace(/\\n/g, ' / '),
  }));
`);
console.log("=== SWITCHES ===\n" + JSON.stringify(switches, null, 1));

const target = Number(process.argv[3] ?? -1);
if (target >= 0) {
  frames.length = 0;
  await session.evaluate(`
    const el = [...document.querySelectorAll('[role="switch"]')][${target}];
    el.scrollIntoView();
    el.click();
    return true;
  `);
  await new Promise((r) => setTimeout(r, 3500));
  console.log("=== FRAMES AFTER CLICK ===");
  for (const f of frames) {
    if (!/updateSettings|Exit|Defect|Failure/.test(f.text)) continue;
    console.log(f.dir, f.text.slice(0, 1400));
    console.log("");
  }
  const after = await session.evaluate(`
    const el = [...document.querySelectorAll('[role="switch"]')][${target}];
    return { checked: el.getAttribute('aria-checked'), body: document.body.innerText.slice(0, 300) };
  `);
  console.log("=== AFTER ===\n" + JSON.stringify(after, null, 1));
}

await session.close();
