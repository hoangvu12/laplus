// Attach an image on a phone-sized viewport, where paste and drop do not exist.
//
//   node tools/ui-driver/probe-attach-image.mjs
//
// The composer only ever accepted images from a paste or a drop, and a phone
// offers neither. So this drives the one path a phone has: expand the collapsed
// mobile composer, press the paperclip, and hand its file input a real PNG the
// way the picker would.
//
// Isolated: its own server, its own app data, a provider pointed at a text file
// and no turn sent. Never connects to the developer's server and spends nothing.
//
// Exit 1 if the paperclip is missing or hidden at either width, or if the
// picked image never reaches the composer.
import { spawn } from "node:child_process";
import { mkdtempSync, mkdirSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { launch, poll } from "./cdp.mjs";

const root = resolve(import.meta.dirname, "../../..");
const scratch = mkdtempSync(join(tmpdir(), "laplus-attach-"));
const data = join(scratch, "laplus");
mkdirSync(data);

// A 1x1 PNG, so the picker hands the composer bytes a decoder accepts.
const png = join(scratch, "probe-attachment.png");
writeFileSync(
  png,
  Buffer.from(
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==",
    "base64",
  ),
);

// A thread will not be created against a disabled instance, so one is enabled —
// pointed at a text file rather than an executable, the way
// `probe-thread-modes.mjs` is. This probe never sends, so nothing launches; if
// something ever did, it would die at the child rather than reach an account.
// Both defaults are named, or the server supplies its own: enabled, and pointed
// at whatever it finds on the machine.
const stub = join(scratch, "not-an-executable.txt");
writeFileSync(stub, "This probe must never launch a provider.\n");
const instance = (driver, enabled) => ({
  driver,
  displayName: "Attach fixture",
  enabled,
  config: { binaryPath: stub, homePath: scratch, launchArgs: "", customModels: [] },
});
writeFileSync(
  join(data, "settings.json"),
  JSON.stringify({
    providerInstances: {
      claudeAgent: instance("claudeAgent", false),
      codex: instance("codex", true),
    },
  }),
);

const isolatedEnv = {
  ...process.env,
  LOCALAPPDATA: scratch,
  APPDATA: scratch,
  CODEX_HOME: scratch,
  CLAUDE_CONFIG_DIR: scratch,
  PATH: `${process.env.SystemRoot}\\System32;${process.env.SystemRoot}`,
};
for (const key of Object.keys(isolatedEnv))
  if (/API_KEY|AUTH_TOKEN|ACCESS_TOKEN/.test(key)) delete isolatedEnv[key];

const server = spawn(
  process.env.LAPLUS_SERVER ?? join(root, "server/target/debug/laplus-server.exe"),
  ["serve", "--port", "0", "--ui", join(root, "apps/web/dist")],
  { windowsHide: true, env: isolatedEnv, stdio: ["ignore", "pipe", "pipe"] },
);
let startup = "";
server.stdout.on("data", (x) => (startup += x));
server.stderr.on("data", (x) => (startup += x));

/** The paperclip and its input, as the page can see them right now. */
const readAttachAffordance = (session) =>
  session.evaluate(`
    const visible = (el) => Boolean(el) && el.getClientRects().length > 0;
    const button = document.querySelector('[data-chat-composer-attach]');
    const input = document.querySelector('[data-chat-composer-attach-input]');
    return {
      button: visible(button),
      label: button?.getAttribute('aria-label') ?? null,
      // The input is deliberately hidden; presence is what matters.
      input: Boolean(input),
    };
  `);

/** Hand the file input a path, the way the operating system's picker does. */
async function pickImage(session, path) {
  await session.send("DOM.enable");
  const { root: document } = await session.send("DOM.getDocument", { depth: -1 });
  const { nodeId } = await session.send("DOM.querySelector", {
    nodeId: document.nodeId,
    selector: "[data-chat-composer-attach-input]",
  });
  if (!nodeId) throw Error("the attach input is not in the document");
  await session.send("DOM.setFileInputFiles", { nodeId, files: [path] });
}

let session;
let failure = null;
try {
  const url = await poll(() => startup.match(/http:\/\/[^\s]+#token=[^\s)]+/)?.[0], 20000);
  if (!url) throw Error("isolated server did not announce startup");
  session = await launch({ url });
  await poll(
    () =>
      session.evaluate(
        'return !location.pathname.includes("pair") && document.body.innerText.length > 30;',
      ),
    15000,
  );

  await session.evaluate(`return (async()=> {
    const ws=new WebSocket(location.origin.replace('http','ws')+'/ws');
    await new Promise(r=>ws.onopen=r);
    let i=1;
    const call=(payload,tag="orchestration.dispatchCommand")=>new Promise((ok,bad)=>{const id=String(i++);const timer=setTimeout(()=>bad(Error('seed timeout')),10000);const listener=e=>{for(const f of JSON.parse('['+e.data.trim().split('\\n').join(',')+']'))if(f._tag==='Exit'&&f.requestId===id){clearTimeout(timer);ws.removeEventListener('message',listener);f.exit._tag==='Success'?ok(f.exit.value):bad(Error(JSON.stringify(f.exit)));}};ws.addEventListener('message',listener);ws.send(JSON.stringify({_tag:'Request',id,tag,payload,headers:[]}));});
    const settings=await call({},'server.getSettings');
    if(Object.entries(settings.providerInstances??{}).some(([id,p])=>p.enabled&&(id!=='codex'||p.config?.binaryPath!==${JSON.stringify(stub)}))) throw Error('Safety precondition: effective provider settings permit a real executable');
    await call({type:'project.create',commandId:'attach-project',projectId:'attach-project',title:'Attach fixture',workspaceRoot:${JSON.stringify(scratch)},createWorkspaceRootIfMissing:true,defaultModelSelection:{instanceId:'codex',model:'gpt-5.4-mini'},createdAt:new Date().toISOString()});
    await call({type:'thread.create',commandId:'attach-thread',threadId:'attach-thread',projectId:'attach-project',title:'Attach probe',modelSelection:{instanceId:'codex',model:'gpt-5.4-mini'},runtimeMode:'full-access',interactionMode:'default',branch:null,worktreePath:null,createdAt:new Date().toISOString()});
    ws.close();return 'seeded';
  })();`);

  const selected = await poll(
    () =>
      session.evaluate(
        `const archive=document.querySelector('[aria-label="Archive Attach probe"]');if(!archive)return false;let el=archive.parentElement;while(el && !['BUTTON','A'].includes(el.tagName) && el.getAttribute('role')!=='button')el=el.parentElement;if(!el)return false;el.click();return true;`,
      ),
    15000,
  );
  if (!selected) throw Error("seeded conversation row missing");
  const composer = await poll(
    () => session.evaluate(`return Boolean(document.querySelector('[contenteditable="true"]'));`),
    20000,
  );
  if (!composer)
    throw Error(
      "composer absent: " +
        (await session.evaluate("return document.body.innerText.slice(-1200);")),
    );

  // Desktop first: the affordance is not mobile-only, and a regression there
  // would be invisible from a phone.
  const desktop = await poll(async () => {
    const seen = await readAttachAffordance(session);
    return seen.button ? seen : null;
  }, 10000);
  console.log("desktop:", JSON.stringify(desktop));
  if (!desktop) failure = "the paperclip is absent at desktop width";

  // Now the phone. The composer rests collapsed until it is touched, and the
  // footer that carries the paperclip is not drawn until then.
  await session.send("Emulation.setDeviceMetricsOverride", {
    width: 390,
    height: 844,
    deviceScaleFactor: 3,
    mobile: true,
  });
  await session.send("Emulation.setTouchEmulationEnabled", { enabled: true, maxTouchPoints: 5 });
  await new Promise((r) => setTimeout(r, 1500));

  const collapsed = await readAttachAffordance(session);
  console.log("mobile, resting:", JSON.stringify(collapsed));

  const expanded = await poll(
    () =>
      session.evaluate(
        `const el=document.querySelector('[aria-label="Expand composer"]');if(el){el.click();return true;}
         const editor=document.querySelector('[contenteditable="true"]');if(!editor)return false;editor.focus();return true;`,
      ),
    10000,
  );
  if (!expanded) throw Error("the mobile composer could not be expanded");

  const mobile = await poll(async () => {
    const seen = await readAttachAffordance(session);
    return seen.button && seen.input ? seen : null;
  }, 10000);
  console.log("mobile, expanded:", JSON.stringify(mobile));
  if (!mobile) {
    failure ??= "the paperclip is unreachable on a phone-sized viewport";
    throw Error("the paperclip is unreachable on a phone-sized viewport");
  }

  await pickImage(session, png);

  const staged = await poll(
    () =>
      session.evaluate(
        `const card=document.querySelector('[aria-label="Preview probe-attachment.png"]');
         if(!card)return null;
         const img=card.querySelector('img');
         return JSON.stringify({visible:card.getClientRects().length>0,src:(img?.getAttribute('src')??'').slice(0,5)});`,
      ),
    10000,
  );
  console.log("staged:", staged);
  if (!staged) {
    failure ??= "the picked image never reached the composer";
  } else if (!JSON.parse(staged).visible) {
    failure ??= "the picked image reached the composer but is not drawn";
  }

  console.log("=== COMPOSER TEXT ===");
  console.log(
    await session.evaluate(
      `const form=document.querySelector('form');return (form?.innerText ?? '').slice(0, 600);`,
    ),
  );
} catch (error) {
  failure ??= String(error?.message ?? error);
} finally {
  await session?.close();
  server.kill();
  await new Promise((r) => setTimeout(r, 500));
}

if (failure) {
  console.error("FAIL:", failure);
  process.exit(1);
}
console.log("OK: the paperclip is reachable at both widths and the picked image is staged");
