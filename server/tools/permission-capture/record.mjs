// Records `fixtures/claude-cli/07-permission-*.ndjson` by driving the real
// `claude` and answering its permission prompt.
//
// Sibling of `tools/wire-capture/`, which records the *other* protocol. This one
// exists because a permission capture cannot be made by hand: the CLI stops on a
// `control_request` and everything after it in the recording is a consequence of
// the answer, so recording one means being a client that answers.
//
//   node tools/permission-capture/record.mjs <decision> <out.ndjson> [prompt]
//
// where <decision> is one of:
//
//   allow    approve once                 -> 07-permission-approved.ndjson
//   deny     decline                      -> 08-permission-declined.ndjson
//   ignore   never answer, then close     -> 09-permission-unanswered.ndjson
//   session  approve and stop being asked -> 10-permission-for-the-session.ndjson
//   cancel   decline and stop the turn    -> 15-permission-cancelled.ndjson
//
// `cancel` is `deny` with `interrupt: true`, which is the composer's fourth
// button and the one ticket 13 sent correctly and never recorded the answer to.
//
// The flags below are `crate::agent`'s, verbatim, plus `--model` for cost. The
// agent runs in a fresh temporary directory so the prompt has somewhere harmless
// to write. Re-recording costs real API usage, so it is a deliberate act rather
// than part of any test.
import { spawn } from "node:child_process";
import { createWriteStream, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import readline from "node:readline";

const DECISIONS = new Set(["allow", "deny", "ignore", "session", "cancel"]);

/** How long to wait, after a request goes unanswered, before closing stdin. */
const GIVE_UP_AFTER_MS = 5_000;

/** A backstop so a wedged recording ends rather than hanging a terminal. */
const KILL_AFTER_MS = 120_000;

const [decision, out, ...rest] = process.argv.slice(2);
if (!DECISIONS.has(decision) || !out) {
  console.error(`usage: node record.mjs <${[...DECISIONS].join("|")}> <out.ndjson> [prompt]`);
  process.exit(2);
}

// A prompt that reliably asks. Note that a *safe* command does not: the CLI
// classifies `echo hello` as harmless and runs it without a prompt, which is how
// the first attempt at this recording came back with no request in it. A `Write`
// always asks in the default permission mode.
const prompt = rest.join(" ") || "Create a file called note.txt containing the word hello";
const cwd = mkdtempSync(join(tmpdir(), "laplus-perm-"));
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
    "--model",
    "claude-haiku-4-5",
  ],
  { cwd, stdio: ["pipe", "pipe", "inherit"] },
);

/** The permission decision, in the CLI's own vocabulary. */
function responseFor(request) {
  switch (decision) {
    case "allow":
      return {
        behavior: "allow",
        updatedInput: request.input,
        decisionClassification: "user_temporary",
      };
    case "session":
      return {
        behavior: "allow",
        updatedInput: request.input,
        // The CLI's own suggestions, handed back. This is what stops it asking.
        updatedPermissions: request.permission_suggestions ?? [],
        decisionClassification: "user_permanent",
      };
    case "cancel":
      return {
        behavior: "deny",
        message: "The developer cancelled the turn.",
        // The whole difference between this and a decline: the CLI stops the
        // turn on it rather than handing the refusal to the model as a result.
        interrupt: true,
        decisionClassification: "user_reject",
      };
    default:
      return {
        behavior: "deny",
        message: "The developer declined this action.",
        decisionClassification: "user_reject",
      };
  }
}

let answered = false;
readline.createInterface({ input: child.stdout }).on("line", (line) => {
  log.write(line + "\n");

  let event;
  try {
    event = JSON.parse(line);
  } catch {
    return;
  }

  if (event.type === "control_request" && event.request?.subtype === "can_use_tool") {
    console.error(`asked about ${event.request.tool_name}: ${event.request_id}`);
    if (decision === "ignore") {
      // Leave it hanging, then close stdin — which is what the server does on
      // shutdown, and what makes the CLI abandon the request instead of waiting.
      setTimeout(() => child.stdin.end(), GIVE_UP_AFTER_MS);
      return;
    }
    child.stdin.write(
      JSON.stringify({
        type: "control_response",
        response: {
          subtype: "success",
          request_id: event.request_id,
          response: responseFor(event.request),
        },
      }) + "\n",
    );
    answered = true;
  }

  if (event.type === "result") {
    child.stdin.end();
  }
});

child.on("exit", (code) => {
  console.error(`claude exited ${code}; answered=${answered}; ran in ${cwd}`);
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
