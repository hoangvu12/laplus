// Settings → Connections → Add environment → Remote link, driven for real.
//
// The other half of `remote-pairing.mjs`. That one walks the pairing chain with
// `fetch`, which proves the *server's* answers are ones a browser accepts; this
// one presses the buttons a person presses, which proves the *client's* own path
// reaches those calls and does something with the result. Ticket 02 needed the
// first. **Ticket 06's acceptance criterion is this one.**
//
//   node tools/ui-driver/add-remote-environment.mjs <page-url> <remote-url> <code>
//
// Two servers, each with a profile of its own — the recipe in
// `remote-pairing.mjs`'s header, unchanged:
//
//   LOCALAPPDATA=…\lc-a laplus-server.exe --ui apps/web/dist --port 5773
//   LOCALAPPDATA=…\lc-b laplus-server.exe --ui apps/web/dist --port 5774
//
// ## It is expected to fail until ticket 06 lands
//
// And it should be read as a pending acceptance check rather than a broken
// driver. On 2026-07-30, against a server with ticket 02 but not 06, it reports:
//
//   pairing calls:  4, all 200 or 204
//   listed:         no  — "No saved remote environments"
//
// which is exactly the bug 06 describes: every laplus answers
// `environmentId: "local"`, the client's registry is one slot per id, and the
// desktop's own backend already holds that slot — so the registration succeeds
// and has nowhere to be listed. **The distinction this driver draws is the whole
// point of running it:** the pairing calls succeeding while the list stays empty
// is what tells you the remaining fault is not CORS. When 06 lands this should
// go green with nothing else changing.
//
// ## The two things that make it brittle, said out loud
//
// It reads the DOM by placeholder and by button text, so a copy change in
// `ConnectionsSettings.tsx` breaks it and the break looks like a product bug.
// `probe-open-thread.mjs` carries the same warning for the same reason. The
// selectors it depends on are named in `SELECTORS` below so there is one place
// to fix.
import { launch, consoleLog, crossOriginLines, poll, wireLog } from "./cdp.mjs";

const page = process.argv[2] ?? "http://127.0.0.1:5773/";
const remote = process.argv[3] ?? "http://127.0.0.1:5774";
const code = process.argv[4];
if (!code) {
  console.error("usage: add-remote-environment.mjs <page-url> <remote-url> <pairing-code>");
  process.exit(2);
}

/** Every piece of `ConnectionsSettings.tsx` this driver reaches into. */
const SELECTORS = {
  // The ghost button in the "Remote environments" section header.
  openDialog: '[aria-label="Add environment"]',
  // `renderConnectionModeCard({ mode: "remote", title: "Remote link", … })`.
  remoteModeCard: "Remote link",
  // `renderRemoteFields()` — placeholders rather than labels, because the labels
  // are spans beside the input rather than `for=` targets.
  host: 'input[placeholder="backend.example.com"]',
  pairingCode: 'input[placeholder="PAIRCODE"]',
  // The full-width submit inside the dialog, which shares its text with the
  // ghost button that opened it — hence the `w-full` discriminator.
  submit: "Add environment",
  // `EmptyRemoteEnvironments`, the state this is trying to leave.
  emptyList: "No saved remote environments",
};

const session = await launch({ url: page });
const logs = consoleLog(session);
const wire = wireLog(session);

const loaded = await poll(
  () => session.evaluate(`return document.readyState === "complete" ? "yes" : null;`),
  20000,
);
if (!loaded) {
  console.error("the page never finished loading — is the page server up?");
  process.exit(1);
}
// Let the boot handshake finish before navigating, so the session cookie is set
// and the settings route does not bounce to pairing.
await new Promise((resolve) => setTimeout(resolve, 6000));

await session.evaluate(`location.assign("/settings/connections"); return 1;`);
const arrived = await poll(
  () => session.evaluate(`return document.querySelector('${SELECTORS.openDialog}') ? 1 : null;`),
  20000,
);
if (!arrived) {
  console.error("never reached settings/connections. What the page showed instead:");
  console.error((await session.evaluate(`return document.body.innerText;`))?.slice(0, 1500));
  await session.close();
  process.exit(1);
}

const filled = await session.evaluate(`
  document.querySelector('${SELECTORS.openDialog}').click();
  return 1;
`);
await new Promise((resolve) => setTimeout(resolve, 1200));

// React tracks an input's value on the DOM node, so assigning \`.value\` updates
// the box and tells the component nothing. Going through the prototype's setter
// and dispatching \`input\` is what makes it a change the component sees — the
// trap any hand-written form driver falls into once.
const fields = await session.evaluate(`
  const set = (element, value) => {
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set;
    setter.call(element, value);
    element.dispatchEvent(new Event("input", { bubbles: true }));
  };
  const card = [...document.querySelectorAll("button,div[role=button]")]
    .find((element) => element.innerText?.trim().startsWith(${JSON.stringify(SELECTORS.remoteModeCard)}));
  if (card) card.click();
  const host = document.querySelector('${SELECTORS.host}');
  const pairingCode = document.querySelector('${SELECTORS.pairingCode}');
  if (!host || !pairingCode) {
    return { error: "the remote-link fields are not on the page", text: document.body.innerText.slice(0, 1200) };
  }
  set(host, ${JSON.stringify(remote)});
  set(pairingCode, ${JSON.stringify(code)});
  return { host: host.value, pairingCode: pairingCode.value };
`);
if (fields.error) {
  console.error(fields.error);
  console.error(fields.text);
  await session.close();
  process.exit(1);
}
// The scheme matters and is easy to lose: `resolveRemotePairingTarget`
// (`packages/shared/src/remote.ts`) defaults a bare host to `https://`, which
// fails against a plain-HTTP server with an error about the backend rather than
// about the scheme.
console.log(`form: host=${fields.host} code=${fields.pairingCode}`);

await session.evaluate(`
  const submits = [...document.querySelectorAll("button")]
    .filter((button) => button.innerText?.trim() === ${JSON.stringify(SELECTORS.submit)}
      && button.className.includes("w-full"));
  submits[submits.length - 1].click();
  return 1;
`);

// Long enough for the descriptor fetch, the token exchange and a render.
await new Promise((resolve) => setTimeout(resolve, 8000));

const after = await session.evaluate(`
  const dialog = document.querySelector('[role=dialog]');
  const body = document.body.innerText;
  return {
    dialogOpen: dialog !== null && dialog.innerText.includes(${JSON.stringify(SELECTORS.submit)}),
    error: dialog?.querySelector(".text-destructive")?.innerText ?? null,
    listedEmpty: body.includes(${JSON.stringify(SELECTORS.emptyList)}),
    section: body.slice(body.indexOf("Remote environments"), body.indexOf("Remote environments") + 400),
  };
`);

const calls = crossOriginLines(wire, remote);
console.log("=== what chrome put on the wire ===");
for (const line of calls) console.log(line);

const refused = [...wire.values()].filter(
  (seen) => seen.url.startsWith(remote) && (seen.failed || Number(seen.status) >= 400),
);
console.log(`pairing calls:  ${calls.length}, ${refused.length} refused`);
console.log(`dialog:         ${after.dialogOpen ? "still open" : "closed"}`);
console.log(`dialog error:   ${after.error ?? "none"}`);
console.log(`listed:         ${after.listedEmpty ? "no" : "yes"}`);
console.log("=== the section it should be listed in ===");
console.log(after.section);

const corsErrors = logs.filter((line) => /CORS|Access-Control|Failed to fetch/i.test(line));
if (corsErrors.length) {
  console.log("=== console, CORS ===");
  console.log(corsErrors.join("\n"));
}

await session.close();

const why = [
  calls.length === 0 ? "the page made no request to the remote at all" : null,
  ...refused.map((seen) => `${seen.method} ${seen.url} — ${seen.failed ?? seen.status}`),
  after.error ? `the dialog reported: ${after.error}` : null,
  after.listedEmpty
    ? `paired, but "${SELECTORS.emptyList}" — the ticket 06 symptom if the calls above all passed`
    : null,
  ...corsErrors,
].filter(Boolean);
if (why.length) {
  console.error(`\nFAILED:\n  ${why.join("\n  ")}`);
  process.exit(1);
}
console.log("\nOK — the remote environment is paired and listed.");
