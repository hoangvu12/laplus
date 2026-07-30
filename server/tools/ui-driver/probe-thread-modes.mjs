// Ticket 02 of the thread-lifecycle effort: the two mode pickers, pressed for
// real.
//
// **The picker does not dispatch on click.** `handleRuntimeModeChange` writes
// the composer's own draft and nothing else; the command goes out from
// `persistThreadSettingsForNextTurn`, on **send**, immediately before
// `startThreadTurn`. That is worth knowing before reading this file, and it is
// what makes the bug ticket 02 fixes worse than the ticket says: the send is
// guarded by `if (failure === null)`, so a refused `thread.runtime-mode.set`
// does not merely lose the mode — **it stops the message being sent at all**.
// Changing the picker on an existing conversation broke sending.
//
// It also explains why the picker used to look like it worked. The chosen mode
// lives in `localStorage` (`composerDraftStore`), and `ChatView` reads
// `composerRuntimeMode ?? activeThread?.runtimeMode` — so the label survived a
// reload on the same origin whether or not the server had ever heard of it. A
// probe that believed the label would have gone green against a server that
// refused every command. This one reads the **server's** copy over HTTP and
// treats the label as decoration.
//
// Spends no agent turn *against the API*, but it does send a message, so point
// it at a laplus whose agent binary is not an agent. Belt and braces around the
// real risk here, which is a probe that starts a real conversation:
//
//   printf '{"providers":{"claudeAgent":{"binaryPath":"C:/tmp/not-an-agent.txt"}}}' \
//     > "$SCRATCH/laplus/settings.json"
//   LOCALAPPDATA=$SCRATCH LAPLUS_PORT=4779 ./target/release/laplus.exe &
//   node tools/ui-driver/probe-thread-modes.mjs http://127.0.0.1:4779/ <code>
//
// Copy `state.sqlite` in from the real profile first — this needs a conversation
// to exist, and it will not make one. The turn it sends then fails at the child,
// which is fine: both mode commands are answered before `startThreadTurn` is
// even called.
//
// **The second argument is the boot credential, and it is not optional.** Since
// ticket 73 a browser on a fresh profile has none, so it lands on `/pair` and
// every `/api` call is a 401 — which reads exactly like a broken server. The
// shell does not print the code (only `laplus-server` does), and it is stored in
// plaintext on purpose (`crate::pairing::PairingLink`), so:
//
//   sqlite3 "$SCRATCH/laplus/state.sqlite" \
//     "select credential from auth_pairing_links
//      where subject='desktop-bootstrap' and consumed_at is null
//        and revoked_at is null order by created_at desc limit 1"
//
// It goes in the URL **fragment**, which is where `Server::window_url` puts it
// and where `getPairingTokenFromUrl` reads it from — a fragment is not sent to
// the server, which is the point.
//
// Exit code 1 if either command was refused, if neither was dispatched, or if
// the mode the server holds afterwards is not the one that was picked.

import { launch, frameLog, consoleLog, poll } from "./cdp.mjs";

const ORIGIN = process.argv[2] ?? "http://127.0.0.1:4773/";
const CODE = process.argv[3];
if (!CODE) {
  console.error("usage: probe-thread-modes.mjs <origin> <boot-credential>");
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

/**
 * Frames since `from` that refused something.
 *
 * Keyed on the **sentence** rather than on a `_tag`: since ticket 39 a refusal
 * carries whatever tag the called method declares, and this file's whole subject
 * is a command that used to come back as "Command not implemented by this
 * server: thread.runtime-mode.set".
 */
const refusals = (from) =>
  frames
    .slice(from)
    .filter(
      (f) =>
        f.dir === "←" &&
        /not implemented by this server|OrchestrationDispatchCommandError/.test(f.text),
    )
    .map((f) => f.text.slice(0, 240));

/** The commands the page dispatched since `from`, by `type`, in order. */
const dispatched = (from) =>
  frames
    .slice(from)
    .filter((f) => f.dir === "→" && f.text.includes("dispatchCommand"))
    .flatMap((f) => [...f.text.matchAll(/"type":"([a-z.\-]+)"/g)].map((m) => m[1]));

/** What the server holds for this conversation, which is the only real answer. */
const storedModes = (id) =>
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
        runtimeMode: thread.runtimeMode,
        interactionMode: thread.interactionMode,
      });
    })();
  `,
    )
    .then((raw) => (raw ? JSON.parse(raw) : null));

/**
 * Open the first conversation in the sidebar, by clicking its row.
 *
 * **Clicked rather than routed to.** `history.pushState` plus a `popstate`
 * changes the address bar and TanStack Router does not hear it, so the URL reads
 * `/local/<id>` and the pane never mounts — which looks exactly like "the
 * composer has no picker" and is a whole wrong diagnosis. The row is found
 * through its own Archive button, because that button carries the only stable
 * label in a row whose visible text is the developer's own prompt.
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
  await settle(5000);

  // The id out of the address bar, which is the router's own answer to which
  // conversation is open rather than a guess.
  const id = (await session.evaluate(`return location.href;`)).split("/").pop();
  const stored = await storedModes(id);
  if (!stored) throw new Error(`the server would not describe ${id}`);
  return { id, title: clicked, ...stored };
}

/** What the runtime picker's trigger reads. Decoration — see the header. */
const pickerReads = () =>
  session.evaluate(`
    const trigger = document.querySelector('[aria-label="Runtime mode"]');
    return trigger ? (trigger.innerText || "").trim().replace(/\\s+/g, " ") : "NO PICKER";
  `);

/** What the interaction toggle reads: "Build" or "Plan". Also decoration. */
const toggleReads = () =>
  session.evaluate(`
    const button = [...document.querySelectorAll('button')]
      .filter((e) => e.offsetParent !== null)
      .find((e) => /^(Build|Plan)$/.test((e.innerText || "").trim()));
    return button ? button.innerText.trim() : "NO TOGGLE";
  `);

const held = await openAConversation();
console.log("=== DRIVING ===");
console.log(`thread ${held.id} — ${held.title.slice(0, 48)}`);
console.log(`server holds runtimeMode=${held.runtimeMode} interactionMode=${held.interactionMode}`);
console.log(`picker reads: ${await pickerReads()}   toggle reads: ${await toggleReads()}`);

// "Supervised" is `approval-required` — the *tightening* direction, and the one a
// server that rounded an unknown mode to the nearest it understood would get
// wrong in the dangerous direction.
const wantedRuntime = held.runtimeMode === "approval-required" ? "Full access" : "Supervised";
const wantedRuntimeMode = wantedRuntime === "Supervised" ? "approval-required" : "full-access";
const wantedInteractionMode = held.interactionMode === "plan" ? "default" : "plan";

console.log(`\n--- picking ${wantedRuntime} and toggling the interaction mode`);
// Taken *before* the clicks. Reading `frames.length` after them and slicing
// from there is the mistake that makes "nothing was dispatched" unfalsifiable —
// it slices from the current end, so it is empty however much was sent.
const beforeTheClicks = frames.length;
const picked = await session.evaluate(`
  return (async () => {
    const trigger = document.querySelector('[aria-label="Runtime mode"]');
    if (!trigger) return "NO PICKER";
    trigger.click();
    // The popup mounts on a frame of its own, so the options do not exist yet.
    await new Promise((r) => setTimeout(r, 800));
    const options = [...document.querySelectorAll('[role="option"]')];
    const option = options
      .find((e) => (e.innerText || "").trim().startsWith(${JSON.stringify(wantedRuntime)}));
    if (!option) return "NO OPTION: " + options
      .map((e) => (e.innerText || "").split("\\n")[0]).join(" | ");
    option.click();
    return "picked";
  })();
`);
if (picked !== "picked") fail(picked);

const toggled = await session.evaluate(`
  const button = [...document.querySelectorAll('button')]
    .filter((e) => e.offsetParent !== null)
    .find((e) => /^(Build|Plan)$/.test((e.innerText || "").trim()));
  if (!button) return "NO TOGGLE";
  button.click();
  return "toggled";
`);
if (toggled !== "toggled") fail(toggled);
await settle(1500);
console.log(`  picker reads: ${await pickerReads()}   toggle reads: ${await toggleReads()}`);

const byTheClicks = dispatched(beforeTheClicks);
console.log(
  `  dispatched by the clicks alone: ${byTheClicks.join(", ") || "(nothing, which is the documented behaviour)"}`,
);
if (byTheClicks.length) {
  fail(
    `the pickers now dispatch on click (${byTheClicks.join(", ")}) — this file's ` +
      `whole account of when the commands go out is out of date`,
  );
}

// --- the send, which is where the commands actually go out -----------------
console.log("\n--- sending a message, which is what dispatches them");
const mark = frames.length;
const sent = await session.evaluate(`
  return (async () => {
    const box = document.querySelector('[contenteditable="true"], textarea');
    if (!box) return "NO COMPOSER";
    box.focus();
    document.execCommand("insertText", false, "probe: thread-lifecycle ticket 02");
    await new Promise((r) => setTimeout(r, 400));
    box.dispatchEvent(new KeyboardEvent("keydown", {
      key: "Enter", code: "Enter", keyCode: 13, which: 13, bubbles: true,
    }));
    return "sent";
  })();
`);
console.log(`  ${sent}`);
if (sent !== "sent") fail(sent);
await settle(5000);

const commands = dispatched(mark);
console.log(`  dispatched: ${commands.join(", ") || "(nothing)"}`);
for (const frame of frames
  .slice(mark)
  .filter((f) => f.dir === "→" && f.text.includes("dispatchCommand"))) {
  console.log(`  → ${frame.text.slice(0, 400)}`);
}
for (const refused of refusals(mark)) fail(`REFUSED: ${refused}`);
if (!commands.includes("thread.runtime-mode.set")) {
  fail("no thread.runtime-mode.set was dispatched");
}
if (!commands.includes("thread.interaction-mode.set")) {
  fail("no thread.interaction-mode.set was dispatched");
}
// The order is the claim about the guard: the modes are persisted *before* the
// turn, so a refusal there is what used to stop the message.
const turnAt = commands.indexOf("thread.turn.start");
const modeAt = commands.indexOf("thread.runtime-mode.set");
if (turnAt !== -1 && modeAt !== -1 && modeAt > turnAt) {
  fail("the mode was persisted after the turn started, not before it");
}

// --- what the server holds, which is the whole point -----------------------
console.log("\n=== WHAT THE SERVER HOLDS ===");
const after = await storedModes(held.id);
console.log(`runtimeMode=${after?.runtimeMode} interactionMode=${after?.interactionMode}`);
if (after?.runtimeMode !== wantedRuntimeMode) {
  fail(`the server holds runtimeMode=${after?.runtimeMode}, not ${wantedRuntimeMode}`);
}
if (after?.interactionMode !== wantedInteractionMode) {
  fail(`the server holds interactionMode=${after?.interactionMode}, not ${wantedInteractionMode}`);
}

// --- and after a reload, read off the server rather than the label ---------
console.log("\n=== AFTER A RELOAD ===");
// Without the fragment: the code is single-use and the first load spent it, so
// the reload stands on the session the page kept.
await session.send("Page.navigate", { url: ORIGIN });
await settle(7000);
const reopened = await openAConversation();
console.log(
  `server holds runtimeMode=${reopened.runtimeMode} interactionMode=${reopened.interactionMode}`,
);
console.log(`picker reads: ${await pickerReads()}   toggle reads: ${await toggleReads()}`);
if (reopened.runtimeMode !== wantedRuntimeMode) {
  fail(`the mode did not survive the reload: ${reopened.runtimeMode}`);
}
if (reopened.interactionMode !== wantedInteractionMode) {
  fail(`the interaction mode did not survive the reload: ${reopened.interactionMode}`);
}

const errors = logs.filter((l) => /error|exception/i.test(l) && !/404|401/.test(l));
if (errors.length) {
  console.log("\n=== CONSOLE ===");
  console.log(errors.slice(0, 6).join("\n").slice(0, 1600));
}

console.log(`\n=== ${failures ? `${failures} FAILURE(S)` : "OK"} ===`);
await session.close();
process.exit(failures ? 1 : 0);
