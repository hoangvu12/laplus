// First look: does the UI boot at all under headless Chrome, and what does it
// say on the socket while it does?
//
// Takes the URL as an argument, defaulting to the port laplus serves on. A
// second laplus on a second port — `LAPLUS_PORT=4774`, and a `LOCALAPPDATA`
// of its own for a fresh profile — is how a change can be looked at without
// closing the one already running.
import { launch, frameLog, consoleLog, poll } from "./cdp.mjs";

const session = await launch({ url: process.argv[2] ?? "http://127.0.0.1:4773/" });
const frames = frameLog(session);
const logs = consoleLog(session);

await new Promise((r) => setTimeout(r, 6000));

const text = await session.evaluate(`return document.body.innerText;`);
console.log("=== BODY TEXT ===");
console.log(text?.slice(0, 3000));
console.log("=== CONSOLE ===");
console.log(logs.slice(0, 40).join("\n"));
console.log("=== FRAMES ===");
for (const f of frames) console.log(f.dir, f.text.slice(0, 300));

await session.close();
