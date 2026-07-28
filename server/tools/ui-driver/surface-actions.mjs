// The second half of the surface walk: press the things, and record what the
// server says. `surface-walk.mjs` navigates and reads; this one acts.
//
// Spends no agent turn — every action here is a thread-lifecycle or panel
// control, never a prompt submission.
import { launch, frameLog, consoleLog } from "./cdp.mjs";

const URL = process.argv[2] ?? "http://127.0.0.1:4773/";
const session = await launch({ url: URL });
const frames = frameLog(session);
const logs = consoleLog(session);
const settle = (ms) => new Promise((r) => setTimeout(r, ms));

await settle(5000);

// Every 404 the page took, which the console reports without saying for what.
const failed = await session.evaluate(`
  return JSON.stringify(performance.getEntriesByType("resource")
    .filter((e) => e.responseStatus >= 400)
    .map((e) => e.name + " → " + e.responseStatus));
`);
console.log("=== FAILED REQUESTS ===");
console.log(JSON.parse(failed ?? "[]").join("\n") || "(none)");

async function press(label) {
  const before = frames.length;
  const logsBefore = logs.length;
  const hit = await session.evaluate(`
    const el = [...document.querySelectorAll('button, [role="button"], [role="menuitem"]')]
      .filter((e) => e.offsetParent !== null)
      .find((e) => ((e.getAttribute("aria-label") || e.innerText || "").trim()).startsWith(${JSON.stringify(label)}));
    if (!el) return "NOT FOUND";
    el.click();
    return (el.getAttribute("aria-label") || el.innerText || "").trim().slice(0, 60);
  `);
  await settle(2000);
  const refused = frames
    .slice(before)
    // The sentence, not the `_tag`. Since ticket 39 a refusal carries whatever
    // tag the called method declares, so `ServerMethodNotImplementedError` only
    // answers a method the contract has never heard of — and matching the tag
    // that replaced it would catch real authorization failures too. The
    // sentence is the part that did not move; `crate::refusals` builds it.
    .filter((f) => f.dir === "←" && /not implemented by this server/.test(f.text))
    .map((f) => /not implemented by this server: ([a-zA-Z.\-]+)/.exec(f.text)?.[1] ?? "refused");
  const errs = logs.slice(logsBefore).filter((l) => /error/i.test(l) && !/404/.test(l));
  const toast = await session.evaluate(`
    const t = document.body.innerText.match(/(Failed[^\\n]{0,120}|Could not[^\\n]{0,120}|not implemented[^\\n]{0,120})/);
    return t ? t[0] : "";
  `);
  console.log(`\n--- pressed: ${label} → ${hit}`);
  if (refused.length) console.log(`  !! REFUSED: ${[...new Set(refused)].join(", ")}`);
  if (toast) console.log(`  !! ON SCREEN: ${toast}`);
  if (errs.length)
    console.log(
      `  !! CONSOLE: ${errs
        .slice(0, 3)
        .map((e) => e.slice(0, 160))
        .join(" // ")}`,
    );
  if (!refused.length && !toast && !errs.length) console.log("  (no refusal seen)");
}

console.log("\n=== ACTIONS ===");
for (const label of [
  "Archive",
  "Project actions",
  "Toggle right panel",
  "Toggle terminal drawer",
]) {
  await press(label);
}

// Whatever the project-actions menu opened, list it — the menu items are where
// rename/delete live and they are not buttons until the menu is up.
const menu = await session.evaluate(`
  return JSON.stringify([...document.querySelectorAll('[role="menuitem"], [role="menu"] button')]
    .filter((e) => e.offsetParent !== null)
    .map((e) => (e.innerText || "").trim()).filter(Boolean));
`);
console.log("\n=== OPEN MENU ITEMS ===");
console.log(JSON.parse(menu ?? "[]").join(" | ") || "(no menu open)");

console.log("\n=== PROVIDER ROWS THAT NEVER RESOLVE ===");
const providers = await session.evaluate(`
  history.pushState({}, "", "/settings/providers");
  window.dispatchEvent(new PopStateEvent("popstate"));
  return null;
`);
await settle(4000);
const stuck = await session.evaluate(`
  const text = document.body.innerText;
  return JSON.stringify((text.match(/^(Codex|Claude|Grok|OpenCode|Cursor)[\\s\\S]{0,120}?$/gm) || []).slice(0, 12));
`);
console.log(JSON.parse(stuck ?? "[]").join("\n"));

await session.close();
