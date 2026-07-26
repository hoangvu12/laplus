// Records `fixtures/claude-cli/11-*.ndjson` by driving the real `claude` and
// stopping it mid-turn.
//
// Third of the recorders, after `tools/wire-capture/` (the socket protocol) and
// `tools/permission-capture/` (the agent's permission prompts). It exists for
// the same reason the second one does: an interrupt capture cannot be made by
// hand, because everything in it after the request is a consequence of the
// request, and *when* the request is sent decides what the recording contains.
//
//   node tools/interrupt-capture/record.mjs <what> <out.ndjson> [prompt]
//
// where <what> is one of:
//
//   text      interrupt a reply mid-sentence      -> 11-interrupted-turn.ndjson
//   continue  interrupt, then send another turn   -> 12-interrupt-then-continue.ndjson
//   tool      interrupt a run of tool calls       -> 13-interrupt-during-tool-use.ndjson
//   idle      interrupt when the turn has ended   -> 14-interrupt-with-nothing-running.ndjson
//
// `continue` is the one that answers the ticket's hardest question, and it is a
// question no single-turn recording can answer: after an interrupt the *process*
// is still there, but nothing except a second turn on the same stdin proves it
// will take one.
//
// The flags below are `crate::agent`'s, verbatim, plus `--model` for cost. The
// `tool` recording adds `--permission-mode bypassPermissions` so the long
// command starts without a prompt in the way — this is the *interrupt* channel
// being recorded, not the permission one.
//
// Re-recording costs real API usage, so it is a deliberate act rather than part
// of any test.
import { spawn } from "node:child_process";
import { createWriteStream, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import readline from "node:readline";

const SUBJECTS = new Set(["text", "continue", "tool", "idle"]);

/** How many text deltas to let through before stopping the agent. */
const DELTAS_BEFORE_INTERRUPT = 40;

/** Which announced tool call to stop the agent on, in `tool` mode.
 *
 * A clock does not work here. Four seconds landed before the model had decided
 * on a tool at all, and twenty landed after the whole turn had finished — so
 * the trigger is the thing being waited for rather than a guess at how long it
 * takes: the second `tool_use` block the agent opens. */
const INTERRUPT_ON_TOOL_CALL = 2;

/** How long to keep reading after the interrupt, for whatever the CLI still says. */
const LINGER_AFTER_INTERRUPT_MS = 20_000;

/** A backstop so a wedged recording ends rather than hanging a terminal. */
const KILL_AFTER_MS = 120_000;

const [subject, out, ...rest] = process.argv.slice(2);
if (!SUBJECTS.has(subject) || !out) {
  console.error(`usage: node record.mjs <${[...SUBJECTS].join("|")}> <out.ndjson> [prompt]`);
  process.exit(2);
}

// A prompt long enough that there is still a turn to interrupt by the time the
// interrupt is sent. A short answer would finish first and the recording would
// be of a completed turn with an ignored request in it.
const prompts = {
  text: "Write about 800 words on the history of the bicycle, from the draisine to the safety bicycle.",
  continue:
    "Write about 800 words on the history of the bicycle, from the draisine to the safety bicycle.",
  // Ten small writes rather than one long command: `sleep 60` is classified as
  // a long leading sleep, blocked, and re-run in the background, so the turn
  // ends in nine seconds with nothing left to interrupt. A run of tool calls is
  // also the case the server has to survive — an invocation announced with no
  // result ever coming.
  tool: "Create ten files note-1.txt through note-10.txt, one at a time, each containing a short paragraph about bicycles.",
  idle: "Say the word 'ready' and nothing else.",
};

/** The correction sent after an interrupt, in `continue` mode. */
const FOLLOW_UP = "Never mind the essay. Just say the word 'stopped' and nothing else.";
const prompt = rest.join(" ") || prompts[subject];
const cwd = mkdtempSync(join(tmpdir(), "lightcode-interrupt-"));
const log = createWriteStream(out);

const child = spawn(
  "claude",
  [
    "--print",
    "--input-format",
    "stream-json",
    "--output-format",
    "stream-json",
    "--include-partial-messages",
    "--verbose",
    "--permission-prompt-tool",
    "stdio",
    ...(subject === "tool" ? ["--permission-mode", "bypassPermissions"] : []),
    "--model",
    "claude-haiku-4-5",
  ],
  { cwd, stdio: ["pipe", "pipe", "inherit"] },
);

/**
 * The interrupt, as the CLI's own schema spells it.
 *
 * Found in the binary rather than in documentation: the control-request union
 * accepts `{"subtype": "interrupt", "reason"?: string}`, and the handler aborts
 * the turn's `AbortSignal` with that reason and answers with a
 * `control_response`.
 */
function interrupt(requestId) {
  return (
    JSON.stringify({
      type: "control_request",
      request_id: requestId,
      request: { subtype: "interrupt" },
    }) + "\n"
  );
}

let sent = false;
let followedUp = false;
let deltas = 0;
let tools = 0;

function turnLine(text) {
  return (
    JSON.stringify({
      type: "user",
      message: { role: "user", content: [{ type: "text", text }] },
    }) + "\n"
  );
}

function stopTheTurn(why) {
  if (sent) return;
  sent = true;
  console.error(`interrupting: ${why}`);
  child.stdin.write(interrupt("interrupt-1"));
  // Everything after this is the recording's point. Stdin stays open — the
  // whole question is whether the *session* survives an interrupted turn.
  setTimeout(() => {
    if (child.stdin.writable) console.error("done reading; closing stdin");
    child.stdin.end();
  }, LINGER_AFTER_INTERRUPT_MS).unref();
}

readline.createInterface({ input: child.stdout }).on("line", (line) => {
  log.write(line + "\n");

  let event;
  try {
    event = JSON.parse(line);
  } catch {
    return;
  }

  if (
    subject === "tool" &&
    event.type === "stream_event" &&
    event.event?.type === "content_block_start" &&
    event.event.content_block?.type === "tool_use"
  ) {
    tools += 1;
    if (tools >= INTERRUPT_ON_TOOL_CALL) stopTheTurn(`tool call ${tools} is under way`);
  }

  if (
    !["tool", "idle"].includes(subject) &&
    event.type === "stream_event" &&
    event.event?.type === "content_block_delta" &&
    event.event.delta?.type === "text_delta"
  ) {
    deltas += 1;
    if (deltas >= DELTAS_BEFORE_INTERRUPT) stopTheTurn(`${deltas} deltas in`);
  }

  if (event.type === "control_response") {
    console.error(`control_response: ${JSON.stringify(event.response)}`);
  }

  if (event.type === "result") {
    console.error(
      `result: ${JSON.stringify(event.subtype)} is_error=${event.is_error} ` +
        `terminal_reason=${JSON.stringify(event.terminal_reason)}`,
    );
    if (subject === "idle" && !sent) {
      // The turn is over. Interrupting now is the case the ticket calls a
      // no-op, asked of the CLI rather than assumed about it.
      stopTheTurn("the turn has already finished");
      return;
    }
    if (subject === "continue" && sent && !followedUp) {
      // The whole of what this mode records: a correction, on the same stdin,
      // to the same process, immediately after the turn it replaces was stopped.
      followedUp = true;
      console.error("sending the follow-up turn");
      child.stdin.write(turnLine(FOLLOW_UP));
      return;
    }
    if (sent) child.stdin.end();
  }
});

child.on("exit", (code) => {
  console.error(`claude exited ${code}; interrupted=${sent}; ran in ${cwd}`);
  log.end();
  process.exit(code ?? 0);
});

child.stdin.write(
  JSON.stringify({
    type: "user",
    message: { role: "user", content: [{ type: "text", text: prompt }] },
  }) + "\n",
);

setTimeout(() => {
  console.error("nothing happened for two minutes; killing");
  child.kill();
}, KILL_AFTER_MS).unref();
