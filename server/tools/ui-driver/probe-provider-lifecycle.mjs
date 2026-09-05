// Isolated Windows UI smoke probe. Never connects to the developer's server.
import { spawn } from "node:child_process";
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { launch, poll } from "./cdp.mjs";

const root = resolve(import.meta.dirname, "../../..");
const scratch = mkdtempSync(join(tmpdir(), "laplus-lifecycle-"));
const data = join(scratch, "laplus");
mkdirSync(data);
const fake = join(scratch, "fake.cjs");
const cmd = join(scratch, "fake.cmd");
const trace = join(scratch, "trace.jsonl");
writeFileSync(trace, "");
const fixture = resolve(root, "server/fixtures/codex-app-server/01-plain-turn.jsonl");
writeFileSync(cmd, `@echo off\r\n"${process.execPath}" "${fake}" %*\r\n`);
writeFileSync(
  fake,
  `
const fs = require('node:fs');
const lines = fs.readFileSync(${JSON.stringify(fixture)},'utf8').trim().split(/\\r?\\n/).map(JSON.parse);
const send = x => console.log(JSON.stringify(x));
const log = x => fs.appendFileSync(${JSON.stringify(trace)},JSON.stringify({...x,pid:process.pid})+'\\n');
if (process.argv.includes('--version')) { console.log('codex-cli 0.146.0'); process.exit(0); }
log({method:'process/start'});
require('node:readline').createInterface({input:process.stdin}).on('line', line => {
 let m; try { m=JSON.parse(line); } catch { return; }
 log({method:m.method,params:m.params});
 if(m.id == null) return;
 const reply = result => send({id:m.id,result});
 if(m.method==='initialize') reply({userAgent:'laplus/client/0.146.0 fixture'});
 else if(m.method==='account/read') reply({account:{type:'chatgpt',email:'fixture@example.invalid',planType:'plus'},requiresOpenaiAuth:false});
 else if(m.method==='model/list') reply({data:[{id:'gpt-5.4-mini',model:'gpt-5.4-mini',displayName:'Fixture Codex',description:'Fixture',hidden:false,isDefault:true,supportedReasoningEfforts:[],defaultReasoningEffort:'medium'}],nextCursor:null});
 else if(m.method==='skills/list') reply({data:[]});
 else if(m.method==='thread/start'||m.method==='thread/resume') reply({thread:{id:'codex-thread-1'},cwd:${JSON.stringify(scratch)}});
 else if(m.method==='turn/start') {
  reply({turn:{id:'codex-turn-1',status:'inProgress',error:null}});
  setTimeout(()=> {
   send({method:'item/completed',params:{threadId:'codex-thread-1',turnId:'codex-turn-1',item:{type:'collabAgentToolCall',id:'self-call',tool:'sendInput',status:'completed',senderThreadId:'child-thread',receiverThreadIds:['codex-thread-1'],agentsStates:{'codex-thread-1':{status:'running'}}}}});
   for(const r of lines.slice(7)) if(r.dir==='recv') send(r.msg);
   setTimeout(()=>{
    send({method:'turn/started',params:{threadId:'codex-thread-1',turn:{id:'automatic-2',status:'inProgress'}}});
    send({method:'item/completed',params:{threadId:'codex-thread-1',turnId:'automatic-2',item:{type:'agentMessage',id:'automatic-answer',text:'Automatic follow-up arrived.'}}});
    send({method:'turn/completed',params:{threadId:'codex-thread-1',turn:{id:'automatic-2',status:'completed'}}});
    log({method:'fixture/completed'});
   },100);
  },750);
 } else reply({});
});
`,
);
writeFileSync(
  join(data, "settings.json"),
  JSON.stringify({
    providerInstances: {
      claudeAgent: {
        driver: "claudeAgent",
        displayName: "Disabled fixture",
        enabled: false,
        config: { binaryPath: cmd, homePath: scratch, launchArgs: "", customModels: [] },
      },
      codex: {
        driver: "codex",
        displayName: "Fixture Codex",
        enabled: true,
        config: { binaryPath: cmd, homePath: scratch, launchArgs: "", customModels: [] },
      },
    },
  }),
);
const isolatedEnv = {
  ...process.env,
  LOCALAPPDATA: scratch,
  APPDATA: scratch,
  CODEX_HOME: scratch,
  CLAUDE_CONFIG_DIR: scratch,
  LAPLUS_TEST_CONVERSATION_IDLE_SECS: "1",
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
let session;
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
  const seed = await session.evaluate(`return (async()=> {
   const ws=new WebSocket(location.origin.replace('http','ws')+'/ws');
   await new Promise(r=>ws.onopen=r);
   let i=1;
   const call=(payload,tag="orchestration.dispatchCommand")=>new Promise((ok,bad)=>{const id=String(i++);const timer=setTimeout(()=>bad(Error('seed timeout')),10000);const listener=e=>{for(const f of JSON.parse('['+e.data.trim().split('\\n').join(',')+']'))if(f._tag==='Exit'&&f.requestId===id){clearTimeout(timer);ws.removeEventListener('message',listener);f.exit._tag==='Success'?ok(f.exit.value):bad(Error(JSON.stringify(f.exit)));}};ws.addEventListener('message',listener);ws.send(JSON.stringify({_tag:'Request',id,tag,payload,headers:[]}));});
   const settings=await call({},'server.getSettings');
   if(settings.providerInstances?.codex?.config?.binaryPath!==${JSON.stringify(cmd)} || Object.entries(settings.providerInstances??{}).some(([id,p])=>id!=='codex'&&p.enabled)) throw Error('Safety precondition: effective provider settings permit a real executable');
   await call({type:'project.create',commandId:'probe-project',projectId:'probe-project',title:'Lifecycle fixture',workspaceRoot:${JSON.stringify(scratch)},createWorkspaceRootIfMissing:true,defaultModelSelection:{instanceId:'codex',model:'gpt-5.4-mini'},createdAt:new Date().toISOString()});
   await call({type:'thread.create',commandId:'probe-thread',threadId:'probe-thread',projectId:'probe-project',title:'Lifecycle probe',modelSelection:{instanceId:'codex',model:'gpt-5.4-mini'},runtimeMode:'full-access',interactionMode:'default',branch:null,worktreePath:null,createdAt:new Date().toISOString()});
   ws.close();return 'seeded';
 })();`);
  console.log(seed);
  const marker = await poll(() => readFileSync(trace, "utf8").includes("initialize"), 10000);
  if (!marker)
    throw Error("Safety precondition: fake Codex was not probed; no turn will be submitted");
  const selected = await poll(
    () =>
      session.evaluate(
        `const archive=document.querySelector('[aria-label="Archive Lifecycle probe"]');if(!archive)return false;let el=archive.parentElement;while(el && !['BUTTON','A'].includes(el.tagName) && el.getAttribute('role')!=='button')el=el.parentElement;if(!el)return false;el.click();return true;`,
      ),
    15000,
  );
  if (!selected) throw Error("seeded conversation row missing");
  const composer = await poll(
    () =>
      session.evaluate(
        `const el=document.querySelector('[contenteditable="true"]');if(!el)return false;el.focus();return true;`,
      ),
    20000,
  );
  if (!composer)
    throw Error(
      "composer absent: " +
        (await session.evaluate("return document.body.innerText.slice(-1200);")),
    );
  for (let turn = 1; turn <= 2; turn++) {
    await session.evaluate(`document.querySelector('[contenteditable="true"]').focus();`);
    await session.send("Input.insertText", { text: "Fixture lifecycle " + turn });
    await session.send("Input.dispatchKeyEvent", {
      type: "keyDown",
      key: "Enter",
      code: "Enter",
      windowsVirtualKeyCode: 13,
      text: "\r",
    });
    await session.send("Input.dispatchKeyEvent", {
      type: "keyUp",
      key: "Enter",
      code: "Enter",
      windowsVirtualKeyCode: 13,
    });
    const done = await poll(() => {
      const requests = readFileSync(trace, "utf8")
        .trim()
        .split("\n")
        .filter(Boolean)
        .map(JSON.parse);
      return requests.filter((x) => x.method === "fixture/completed").length >= turn;
    }, 20000);
    if (!done)
      throw Error(
        "fixture turn never completed: " +
          (await session.evaluate("return document.body.innerText.slice(-1800);")),
      );
    const pid = readFileSync(trace, "utf8")
      .trim()
      .split("\n")
      .map(JSON.parse)
      .filter((x) => x.method === "fixture/completed")
      .at(-1).pid;
    const idle = await poll(async () => {
      try {
        process.kill(pid, 0);
        return null;
      } catch {}
      return session.evaluate(
        `return (async()=>{const t=await (await fetch('/api/orchestration/threads/probe-thread')).json();const thread=t.thread??t;return thread.latestTurn?.state==='completed' && !thread.session?.activeTurnId ? {status:thread.session?.status,body:document.body.innerText.slice(-1600)} : null;})();`,
      );
    }, 20000);
    if (!idle)
      throw Error(
        "UI/server did not settle and evict: " +
          (await session.evaluate(
            `return fetch('/api/orchestration/threads/probe-thread').then(r=>r.text());`,
          )),
      );
    if (/Working for/.test(idle.body)) throw Error("UI still claims working after idle eviction");
    if (!idle.body.includes("Automatic follow-up arrived."))
      throw Error("automatic Codex answer did not render");
    console.log(JSON.stringify({ turn, idle: true, stillWorking: false }));
  }
  const methods = readFileSync(trace, "utf8")
    .trim()
    .split("\n")
    .map(JSON.parse)
    .map((x) => x.method);
  if (!methods.includes("thread/resume"))
    throw Error("second turn did not resume saved Codex thread");
  console.log(
    "PASS: UI completed two turns, idle eviction between them, saved Codex thread resumed",
  );
} finally {
  if (session) await session.close();
  server.kill();
  console.log("Isolated fixture artifacts: " + scratch);
}
