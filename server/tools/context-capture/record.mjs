// Records `fixtures/claude-cli/19-context-usage.ndjson` by asking the real
// `claude` how full its context window is.
//
// Fourth of the recorders, after `tools/wire-capture/` (the socket protocol),
// `tools/permission-capture/` (the agent's permission prompts) and
// `tools/interrupt-capture/` (stopping a turn). It exists for the reason the
// last two do: the exchange cannot be written by hand, because the reply is
// fifteen kilobytes of the CLI's own accounting and the point of a capture is
// that a later `claude` disagreeing with it means something.
//
//   node tools/context-capture/record.mjs <out.ndjson> [prompt]
//
// The request is `{"subtype": "get_context_usage"}` on stdin — the same control
// channel `--input-format stream-json` opens for the interrupt, and no flag
// turns it on. `crate::protocol::context_usage_line` is what the server sends.
//
// **The timing is the driver's, deliberately.** `crate::turn` asks at exactly
// two moments and this asks at the same two, because where the answer lands in
// the recording is what the replay in `tests/socket_turn.rs` will see:
//
//   1. on `init` — the session announcing itself, while the first turn is still
//      running. This is the one upstream does not do, and the one that gives the
//      opening turn of a session a percentage instead of a bare token count.
//   2. on `result` — the turn having ended, which is upstream's own timing
//      (`completeTurn` in `reference/t3code-server/.../ClaudeAdapter.ts`).
//
// The flags below are `crate::agent`'s, verbatim, plus `--model` for cost and
// `--permission-mode bypassPermissions` so the tool call in the prompt runs
// without a permission prompt in the way — this is the *context* channel being
// recorded, not the permission one.
//
// Re-recording costs real API usage, so it is a deliberate act rather than part
// of any test.
import { spawn } from "node:child_process";
import { createWriteStream, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import readline from "node:readline";

const [out, ...rest] = process.argv.slice(2);
if (!out) {
  console.error("usage: node record.mjs <out.ndjson> [prompt]");
  process.exit(2);
}

// A turn that *uses a tool*, and that is not incidental. On a turn without one
// the whole conversation is processed once, so `total_processed_tokens` equals
// what the window is carrying and both this server and upstream drop it as the
// same number twice. A capture of that turn could not show the CLI's answer and
// the turn's own total sitting on one reading.
const PROMPT =
  "Create a file note.txt containing one short sentence about bicycles, then tell me you have done it.";
const prompt = rest.join(" ") || PROMPT;

/** A backstop so a wedged recording ends rather than hanging a terminal. */
const KILL_AFTER_MS = 120_000;

/** How long to keep reading after the last question, for the answer to it. */
const LINGER_AFTER_LAST_ASK_MS = 15_000;

const cwd = mkdtempSync(join(tmpdir(), "laplus-context-"));
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
    "--permission-mode",
    "bypassPermissions",
    "--model",
    "claude-haiku-4-5",
  ],
  { cwd, stdio: ["pipe", "pipe", "inherit"] },
);

/**
 * The question, as the CLI's own schema spells it.
 *
 * Found in the binary rather than in documentation: the control-request union
 * accepts `{"subtype": "get_context_usage"}`, described there as "requests a
 * breakdown of current context window usage by category", and the handler
 * answers with a `control_response` naming the same id.
 */
function askContextUsage(requestId) {
  return (
    JSON.stringify({
      type: "control_request",
      request_id: requestId,
      request: { subtype: "get_context_usage" },
    }) + "\n"
  );
}

let asked = 0;
function ask(why) {
  asked += 1;
  console.error(`asking (${why}): context-${asked}`);
  child.stdin.write(askContextUsage(`context-${asked}`));
}

readline.createInterface({ input: child.stdout }).on("line", (line) => {
  log.write(line + "\n");

  let event;
  try {
    event = JSON.parse(line);
  } catch {
    return;
  }
  react(event);
});

function react(event) {
  if (event.type === "system" && event.subtype === "init") {
    ask("the session announced itself");
    return;
  }

  if (event.type === "control_response") {
    const answer = event.response?.response;
    console.error(
      `answered ${event.response?.request_id}: subtype=${event.response?.subtype} ` +
        `total=${answer?.totalTokens} max=${answer?.maxTokens} ` +
        `autoCompact=${answer?.isAutoCompactEnabled}`,
    );
    return;
  }

  if (event.type === "result") {
    console.error(`result: ${event.subtype} is_error=${event.is_error}`);
    ask("the turn ended");
    // Everything after this is the second answer. Stdin stays open until it has
    // had time to arrive.
    setTimeout(() => {
      if (child.stdin.writable) console.error("done reading; closing stdin");
      child.stdin.end();
    }, LINGER_AFTER_LAST_ASK_MS).unref();
  }
}

child.on("exit", (code) => {
  console.error(`claude exited ${code}; asked ${asked} times; ran in ${cwd}`);
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
