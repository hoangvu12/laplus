// Settings → Connections → Add environment → Remote link, driven for real.
//
// The other half of `remote-pairing.mjs`. That one walks the pairing chain with
// `fetch`, which proves the *server's* answers are ones a browser accepts; this
// one presses the buttons a person presses, which proves the *client's* own path
// reaches those calls and does something with the result. Ticket 02 needed the
// first. **Ticket 06's acceptance criterion is this one.**
//
//   node tools/ui-driver/add-remote-environment.mjs <page-url> <remote-url> <code> [<remote-url> <code> …]
//
// Two servers, each with a profile of its own — the recipe in
// `remote-pairing.mjs`'s header, unchanged:
//
//   LOCALAPPDATA=…\lc-a laplus-server.exe --ui apps/web/dist --port 5773
//   LOCALAPPDATA=…\lc-b laplus-server.exe --ui apps/web/dist --port 5774
//
// **A data directory per server is what makes them different environments**, and
// since ticket 06 that is load-bearing rather than tidy: the id lives in
// `state.sqlite`, so two servers sharing one directory answer with one id and
// collide exactly as every laplus used to.
//
// ## More than one remote, in one browser
//
// Pass as many `<remote-url> <code>` pairs as there are servers. The last of
// ticket 06's acceptance criteria is a *third* environment — "several at once"
// is the actual requirement, and a distinct id is only the first thing that
// could have prevented it. Adding them in one session is the point: three
// separate runs would prove three servers each pair once, not that a client
// holds three at the same time.
//
// ## What it looked like before ticket 06
//
// Worth keeping, because it is what the driver is shaped to tell apart. Against a
// server with ticket 02 but not 06 it reported:
//
//   pairing calls:  4, 0 refused
//   listed:         no  — "No saved remote environments"
//
// Every call passing while the list stayed empty is what said the remaining
// fault was not CORS: every laplus answered `environmentId: "local"`, the
// client's registry is one slot per id, and the desktop's own backend already
// held that slot, so the registration succeeded and had nowhere to be listed.
// Since ticket 06 it goes green, and it goes further than it used to — the
// environment is not merely listed but connected, so the run also shows the
// ticket exchange and the orchestration fetch that only happen to a backend the
// client actually adopted.
//
// ## The two things that make it brittle, said out loud
//
// It reads the DOM by placeholder and by button text, so a copy change in
// `ConnectionsSettings.tsx` breaks it and the break looks like a product bug.
// `probe-open-thread.mjs` carries the same warning for the same reason. The
// selectors it depends on are named in `SELECTORS` below so there is one place
// to fix.
import { launch, consoleLog, crossOriginLines, frameLog, poll, wireLog } from "./cdp.mjs";

const page = process.argv[2] ?? "http://127.0.0.1:5773/";
const pairs = process.argv.slice(3);
const expectedUsageTokens = process.env.VERIFY_USAGE_TOKENS ?? null;
const usagePrivacySentinel = process.env.VERIFY_USAGE_PRIVACY_SENTINEL ?? null;
if (pairs.length === 0 || pairs.length % 2 !== 0) {
  console.error(
    "usage: add-remote-environment.mjs <page-url> <remote-url> <pairing-code> [<remote-url> <pairing-code> …]",
  );
  process.exit(2);
}
/** One entry per server to add, in the order the form will be driven. */
const remotes = [];
for (let index = 0; index < pairs.length; index += 2) {
  remotes.push({ url: pairs[index], code: pairs[index + 1] });
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
  // `SavedBackendListRow`'s state button, and the only one of its texts in the
  // whole component — so counting these counts saved rows, which is how this
  // knows two were added rather than one being overwritten. The label cannot do
  // that job: two data directories on one machine share a hostname, and
  // therefore share a label, by design.
  rowStateButton: ["Connect", "Connecting…", "Disconnect", "Disconnecting…"],
};

const session = await launch({ url: page });
const logs = consoleLog(session);
const wire = wireLog(session);
const frames = frameLog(session);

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

/** How many environments the "Remote environments" section is listing. */
const listedRows = () =>
  session.evaluate(`
    const texts = ${JSON.stringify(SELECTORS.rowStateButton)};
    return [...document.querySelectorAll("button")]
      .filter((button) => texts.includes(button.innerText?.trim())).length;
  `);

/**
 * Fill the dialog once and submit it.
 *
 * Returns what the page showed afterwards rather than deciding anything: with
 * several remotes to add, a failure on the second is worth reporting beside the
 * first that worked rather than exiting on the spot.
 */
async function addRemote({ url, code }) {
  await session.evaluate(`document.querySelector('${SELECTORS.openDialog}').click(); return 1;`);
  await new Promise((resolve) => setTimeout(resolve, 1200));

  // React tracks an input's value on the DOM node, so assigning `.value` updates
  // the box and tells the component nothing. Going through the prototype's
  // setter and dispatching `input` is what makes it a change the component sees
  // — the trap any hand-written form driver falls into once.
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
    set(host, ${JSON.stringify(url)});
    set(pairingCode, ${JSON.stringify(code)});
    return { host: host.value, pairingCode: pairingCode.value };
  `);
  if (fields.error) {
    return fields;
  }
  // The scheme matters and is easy to lose: `resolveRemotePairingTarget`
  // (`packages/shared/src/remote.ts`) defaults a bare host to `https://`, which
  // fails against a plain-HTTP server with an error about the backend rather
  // than about the scheme.
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

  return await session.evaluate(`
    const dialog = document.querySelector('[role=dialog]');
    return {
      dialogOpen: dialog !== null && dialog.innerText.includes(${JSON.stringify(SELECTORS.submit)}),
      error: dialog?.querySelector(".text-destructive")?.innerText ?? null,
    };
  `);
}

const outcomes = [];
for (const [index, target] of remotes.entries()) {
  console.log(`\n=== adding ${target.url} (${index + 1} of ${remotes.length}) ===`);
  const after = await addRemote(target);
  const rows = await listedRows();
  console.log(`dialog:         ${after.dialogOpen ? "still open" : "closed"}`);
  console.log(`dialog error:   ${after.error ?? "none"}`);
  console.log(`listed rows:    ${rows}`);
  outcomes.push({ ...target, ...after, rows, expected: index + 1 });
}

const body = await session.evaluate(`
  const body = document.body.innerText;
  return {
    listedEmpty: body.includes(${JSON.stringify(SELECTORS.emptyList)}),
    section: body.slice(body.indexOf("Remote environments"), body.indexOf("Remote environments") + 600),
  };
`);

console.log("\n=== what chrome put on the wire ===");
const refused = [];
for (const target of remotes) {
  const calls = crossOriginLines(wire, target.url);
  console.log(`-- ${target.url}: ${calls.length} calls`);
  for (const line of calls) console.log(`   ${line}`);
  refused.push(
    ...[...wire.values()].filter(
      (seen) => seen.url.startsWith(target.url) && (seen.failed || Number(seen.status) >= 400),
    ),
  );
}

const listed = await listedRows();
console.log(`\nrefused calls:  ${refused.length}`);
console.log(`listed:         ${body.listedEmpty ? "no" : "yes"}`);
console.log(`environments:   ${listed} of ${remotes.length} added`);
console.log("=== the section they should be listed in ===");
console.log(body.section);

const corsErrors = logs.filter((line) => /CORS|Access-Control|Failed to fetch/i.test(line));
if (corsErrors.length) {
  console.log("=== console, CORS ===");
  console.log(corsErrors.join("\n"));
}

let usageFailure = null;
if (expectedUsageTokens) {
  await session.evaluate(`location.assign("/usage"); return 1;`);
  const usageText = await poll(async () => {
    const text = await session.evaluate(`return document.body?.innerText ?? "";`);
    return text.includes("Processed tokens") && text.includes(expectedUsageTokens) ? text : null;
  }, 20_000);
  const usageCalls = frames.filter((frame) => frame.text.includes("server.getUsageSummary"));
  if (!usageText) usageFailure = "the multi-environment Usage report did not settle";
  else if (!usageText.includes("source duplicates")) {
    usageFailure = "the duplicate physical source was not reported";
  } else if (usagePrivacySentinel && usageText.includes(usagePrivacySentinel)) {
    usageFailure = "raw transcript content reached the multi-environment page";
  }
  console.log(`usage settled:  ${usageText ? "yes" : "no"}`);
  console.log(`usage total:    ${expectedUsageTokens}`);
  console.log(`usage calls:    ${usageCalls.length}`);
}

await session.close();

const why = [
  ...remotes
    .filter((target) => crossOriginLines(wire, target.url).length === 0)
    .map((target) => `the page made no request to ${target.url} at all`),
  ...refused.map((seen) => `${seen.method} ${seen.url} — ${seen.failed ?? seen.status}`),
  ...outcomes
    .filter((seen) => seen.error)
    .map((seen) => `${seen.url}: the dialog reported ${seen.error}`),
  body.listedEmpty
    ? `paired, but "${SELECTORS.emptyList}" — the ticket 06 symptom if the calls above all passed`
    : null,
  // Each add should leave one more row than the last. A count that stops going
  // up is the interesting failure this driver was extended to find: two servers
  // that collide on an id would pair successfully and overwrite one another,
  // which reads as a working run everywhere except here.
  ...outcomes
    .filter((seen) => seen.rows !== seen.expected)
    .map(
      (seen) =>
        `after adding ${seen.url} the list held ${seen.rows} rows, expected ${seen.expected}`,
    ),
  ...corsErrors,
  usageFailure,
].filter(Boolean);
if (why.length) {
  console.error(`\nFAILED:\n  ${why.join("\n  ")}`);
  process.exit(1);
}
console.log(
  `\nOK — ${remotes.length} remote environment${remotes.length === 1 ? " is" : "s are"} paired and listed.`,
);
