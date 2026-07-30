// Ticket 04 of the thread-lifecycle effort: ending an agent process, pressed for
// real.
//
// **There is no stop button.** The client sends `thread.session.stop` from two
// places and neither is one: `useThreadActions.ts` sends it before deleting a
// thread, and `BranchToolbarBranchSelector.tsx` sends it when a conversation is
// moved to another worktree. The second is unreachable here — this server refuses
// to prepare a worktree — so the delete flow is the only route the real client has
// to this command, and that is what this drives.
//
// Two consequences of driving it that way, and both are the point rather than
// noise:
//
// - **The delete itself is refused**, because `thread.delete` is thread-lifecycle
//   ticket 10 and this server does not answer it yet. So the run ends with
//   "Failed to delete thread" on screen. What is under test is the command *in
//   front of* that one, which used to be refused as well — and a stop that is
//   answered while the delete is refused is exactly what this ticket changed.
// - **The context menu is the browser fallback**, not the native one.
//   `readLocalApi().contextMenu.show` reaches `showContextMenuFallback` when
//   there is no `desktopBridge`, which builds ordinary `<button>`s — so a
//   headless browser can press "Delete" where it cannot press a Win32 menu item.
//   The confirm is a real `window.confirm`, accepted over CDP below.
//
// Spends no agent turn against the API, but it *does* send a message, so point it
// at a laplus whose `binaryPath` is a stand-in that holds a session. There has to
// be a live process for a stop to end:
//
//   SCRATCH=.scratch/stop-drive
//   cp ~/AppData/Local/laplus/state.sqlite $SCRATCH/profile/laplus/   # a project
//   printf '{"providers":{"claudeAgent":{"binaryPath":"…/fake-claude.cmd"}}}' \
//     > $SCRATCH/profile/laplus/settings.json
//   LOCALAPPDATA=$SCRATCH/profile ./target/debug/laplus-server.exe \
//     --port 4779 --ui ../apps/web/dist
//   node tools/ui-driver/probe-session-stop.mjs http://127.0.0.1:4779/ <code>
//
// `laplus-server` rather than the shell because it prints the boot credential and
// can serve the page from a directory. Repoint the copied profile's project at a
// throwaway folder before running it: a turn writes a checkpoint into whatever
// repository the project names.
//
// Exit code 1 if the stop was refused, if the flow never dispatched one, or if the
// server does not report the session as stopped afterwards.

import { launch, frameLog, consoleLog, poll } from "./cdp.mjs";

const ORIGIN = process.argv[2] ?? "http://127.0.0.1:4773/";
const CODE = process.argv[3];
if (!CODE) {
  console.error("usage: probe-session-stop.mjs <origin> <boot-credential>");
  console.error("see the header for where the credential comes from");
  process.exit(2);
}
const settle = (ms) => new Promise((r) => setTimeout(r, ms));

const session = await launch({ url: `${ORIGIN}#token=${CODE}` });
const frames = frameLog(session);
const logs = consoleLog(session);
let failures = 0;
const fail = (why) => {
  console.log(`  !! ${why}`);
  failures += 1;
};

// The delete asks before it deletes (`confirmThreadDelete` defaults to true) and
// in a browser that is `window.confirm`, which blocks the page until somebody
// answers. Nobody is here, so this is.
const dialogs = [];
session.on((message) => {
  if (message.method !== "Page.javascriptDialogOpening") return;
  dialogs.push(message.params.message);
  void session.send("Page.handleJavaScriptDialog", { accept: true });
});

/** The commands the page dispatched since `from`, by `type`, in order. */
const dispatched = (from) =>
  frames
    .slice(from)
    .filter((f) => f.dir === "→" && f.text.includes("dispatchCommand"))
    .flatMap((f) => [...f.text.matchAll(/"type":"([a-z.\-]+)"/g)].map((m) => m[1]));

/**
 * Refusals since `from`, keyed on the sentence rather than on a `_tag` — since
 * ticket 39 a refusal carries whatever tag the called method declares, and this
 * file's whole subject is a command that used to come back as "Command not
 * implemented by this server: thread.session.stop".
 */
const refusals = (from) =>
  frames
    .slice(from)
    .filter(
      (f) => f.dir === "←" && /not implemented by this server|DispatchCommandError/.test(f.text),
    )
    .map((f) => f.text.slice(0, 300));

/** What the server holds for this conversation, which is the only real answer. */
const stored = (id) =>
  session
    .evaluate(
      `
    return (async () => {
      const r = await fetch("/api/orchestration/threads/" + ${JSON.stringify(id)},
                            {credentials: "include"});
      if (!r.ok) return null;
      const body = await r.json();
      const thread = body.thread ?? body;
      return JSON.stringify({
        session: thread.session,
        latestTurn: thread.latestTurn,
        messages: thread.messages.length,
      });
    })();
  `,
    )
    .then((raw) => (raw ? JSON.parse(raw) : null));

/**
 * Open the first conversation in the sidebar, by clicking its row.
 *
 * Clicked rather than routed to, and found through its own Archive button —
 * `probe-thread-modes.mjs` carries the whole argument for both.
 */
async function openAConversation() {
  const clicked = await poll(
    () =>
      session.evaluate(`
      const archive = [...document.querySelectorAll('[aria-label^="Archive "]')]
        .filter((e) => e.offsetParent !== null)[0];
      if (!archive) return null;
      let node = archive.parentElement;
      while (node && node.getClientRects().length && node.clientHeight < 80) {
        if (node.tagName === 'BUTTON' || node.tagName === 'A' || node.getAttribute('role') === 'button') break;
        node = node.parentElement;
      }
      (node ?? archive.parentElement).click();
      return archive.getAttribute('aria-label').replace(/^Archive /, '');
    `),
    20000,
  );
  if (!clicked) throw new Error("the sidebar has no conversation to drive");
  await settle(4000);
  const id = (await session.evaluate(`return location.href;`)).split("/").pop();
  return { id, title: clicked };
}

const held = await openAConversation();
console.log("=== DRIVING ===");
console.log(`thread ${held.id} — ${held.title.slice(0, 48)}`);

// --- a live session to end -------------------------------------------------
console.log("\n--- sending a message, so there is a process to stop");
const sent = await session.evaluate(`
  return (async () => {
    const box = document.querySelector('[contenteditable="true"], textarea');
    if (!box) return "NO COMPOSER";
    box.focus();
    document.execCommand("insertText", false, "probe: thread-lifecycle ticket 04");
    await new Promise((r) => setTimeout(r, 400));
    box.dispatchEvent(new KeyboardEvent("keydown", {
      key: "Enter", code: "Enter", keyCode: 13, which: 13, bubbles: true,
    }));
    return "sent";
  })();
`);
console.log(`  ${sent}`);
if (sent !== "sent") fail(sent);

const running = await poll(
  () => stored(held.id).then((held) => (held?.session ? held : null)),
  30000,
  250,
);
console.log(`  session: ${running?.session?.status ?? "NONE"}`);
if (!running?.session) fail("no session ever opened, so there is nothing to stop");

// --- the stop, through the only control that sends one ----------------------
console.log("\n--- deleting the thread, which stops the session first");
const mark = frames.length;
const menu = await session.evaluate(`
  return (async () => {
    const archive = [...document.querySelectorAll('[aria-label^="Archive "]')]
      .filter((e) => e.offsetParent !== null)[0];
    if (!archive) return "NO ROW";
    let row = archive.parentElement;
    while (row && row.clientHeight < 24) row = row.parentElement;
    const box = (row ?? archive).getBoundingClientRect();
    (row ?? archive).dispatchEvent(new MouseEvent("contextmenu", {
      bubbles: true, cancelable: true,
      clientX: box.x + 20, clientY: box.y + 8, button: 2,
    }));
    await new Promise((r) => setTimeout(r, 900));
    const item = [...document.querySelectorAll('button')]
      .filter((e) => e.offsetParent !== null)
      .find((e) => (e.innerText || "").trim() === "Delete");
    if (!item) return "NO DELETE ITEM: " + [...document.querySelectorAll('button')]
      .filter((e) => e.offsetParent !== null)
      .map((e) => (e.innerText || "").trim().split("\\n")[0])
      .filter(Boolean).slice(0, 24).join(" | ");
    item.click();
    return "pressed";
  })();
`);
console.log(`  ${menu}`);
if (menu !== "pressed") fail(menu);
await settle(4000);

const commands = dispatched(mark);
console.log(`  dispatched: ${commands.join(", ") || "(nothing)"}`);
console.log(
  `  dialogs answered: ${dialogs.length ? dialogs.map((d) => d.split("\n")[0]).join(" / ") : "(none)"}`,
);
for (const frame of frames
  .slice(mark)
  .filter((f) => f.dir === "→" && f.text.includes("session.stop"))) {
  console.log(`  → ${frame.text.slice(0, 240)}`);
}

if (!commands.includes("thread.session.stop")) {
  fail("the delete never dispatched a stop — this file's account of the flow is out of date");
}
// The delete's own refusal is expected and is ticket 10's; a refusal naming the
// stop is this ticket's failure.
for (const refused of refusals(mark)) {
  if (/session\.stop/.test(refused)) fail(`REFUSED: ${refused}`);
  else console.log(`  (expected refusal) ${refused.slice(0, 160)}`);
}

// --- what the server holds afterwards --------------------------------------
const after = await poll(
  () => stored(held.id).then((h) => (h?.session?.status === "stopped" ? h : null)),
  15000,
  250,
).catch(() => null);
const now = after ?? (await stored(held.id));
console.log(`\n--- afterwards`);
console.log(`  session: ${JSON.stringify(now?.session)}`);
console.log(`  latestTurn: ${JSON.stringify(now?.latestTurn)}`);
console.log(`  messages kept: ${now?.messages}`);
if (now?.session?.status !== "stopped") {
  fail(`the session is ${now?.session?.status ?? "gone"} rather than stopped`);
}
if (!now?.messages) fail("the conversation lost its transcript");

const errors = logs.filter((line) => /error/i.test(line));
if (errors.length) console.log(`\nconsole: ${errors.slice(0, 4).join(" | ").slice(0, 400)}`);

console.log(failures ? `\nFAILED (${failures})` : "\nOK");
await session.close();
process.exit(failures ? 1 : 0);
