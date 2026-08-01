#!/usr/bin/env node
// @effect-diagnostics nodeBuiltinImport:off - A launcher with no runtime, by design: see the note below.
// oxlint-disable t3code/no-global-process-runtime -- `HostProcessPlatform` is an Effect service, and the point of this file is that there is no Effect runtime here to provide it. `process.platform` is read once, handed to a pure function, and that function is what the tests exercise.
/**
 * `npx laplus` — start the server this repository builds, without building it.
 *
 * The reference server is Node, so upstream publishes one bundle and `npx
 * t3@latest` runs it anywhere. Ours is Rust, and this is the thin thing that
 * makes the same sentence true: it works out which machine it is on, finds the
 * binary npm installed for it, and runs it pointed at the UI bundle riding in
 * this package.
 *
 * **It has no dependencies and no runtime**, deliberately. Everything else in
 * this repository is Effect, and this file is `spawn` and `process.exit`. It
 * runs on a machine that has just downloaded it and nothing else, which is the
 * one machine whose experience this package exists to fix, and every dependency
 * is another thing that can be unavailable there.
 *
 * The decisions worth arguing about are in `invocation.ts` and `platform.ts`,
 * where they are pure and tested. What is left here is the part that needs a
 * real filesystem and a real process.
 */
import * as NodeChildProcess from "node:child_process";
import * as NodeFS from "node:fs";
import * as NodeModule from "node:module";
import * as NodeURL from "node:url";

import { invocation } from "./invocation.ts";
import { missingPackageMessage, targetFor, unsupportedMessage } from "./platform.ts";

/**
 * The UI, as staged by `scripts/stage-npm-release.ts` next to `dist/`.
 *
 * `../ui/dist` resolves to the same place whether this file is running as
 * `src/bin.ts` from a checkout or as the packed `dist/bin.mjs` from npm, which
 * is why the staging directory is a sibling of both rather than inside either.
 * `ui/package.json` beside it carries the release's product version, shared by
 * the launcher, UI, server and shell (ADR-0033).
 */
const BUNDLE = NodeURL.fileURLToPath(new URL("../ui/dist", import.meta.url));

const decided = invocation({
  argv: process.argv.slice(2),
  bundle: { directory: BUNDLE, present: NodeFS.existsSync(BUNDLE) },
  environment: process.env,
});

/** Everything this launcher says for itself goes to stderr, so a caller piping
 * the server's own output gets the server's own output and nothing else. */
function complain(message: string): void {
  process.stderr.write(`${message}\n`);
}

const target = targetFor(process.platform, process.arch);
if (target === undefined) {
  complain(unsupportedMessage(process.platform, process.arch));
  process.exit(1);
}

// `createRequire` rather than `import.meta.resolve`, which throws on a package
// that is not installed instead of reporting it: an optional dependency npm
// skipped is the expected case here, not an exceptional one, and it needs the
// sentence in `missingPackageMessage` rather than a stack trace.
const require = NodeModule.createRequire(import.meta.url);
let binary: string;
try {
  binary = require.resolve(`${target.package}/${target.binary}`);
} catch {
  complain(missingPackageMessage(target));
  process.exit(1);
}

for (const warning of decided.warnings) complain(`laplus: ${warning}`);

const server = NodeChildProcess.spawn(binary, decided.arguments, {
  stdio: "inherit",
  env: decided.environment,
});

server.on("error", (failure: NodeJS.ErrnoException) => {
  // EACCES is worth its own sentence because it has one cause and it is ours:
  // GitHub's artifact upload does not carry the executable bit, so the binary
  // arrives at the packing step unrunnable unless it is chmodded first. If this
  // ever prints, a published tarball is wrong and no amount of reinstalling
  // will fix it.
  const explanation =
    failure.code === "EACCES"
      ? "the server binary is not executable, which means it was published wrong."
      : failure.message;
  complain(`laplus: cannot run ${binary}: ${explanation}`);
  process.exit(1);
});

// Only where the signal names mean what they say. On POSIX a terminal already
// delivers Ctrl+C to the whole foreground group, so this is for a programmatic
// `kill` of the launcher and a second SIGINT the server ignores. On Windows
// `kill` has no signals to send: `child.kill("SIGINT")` is TerminateProcess,
// which would tear the server down mid-write rather than let it close its
// database — and the console delivers Ctrl+C to the group there too, so
// forwarding buys nothing and costs that.
if (process.platform !== "win32") {
  for (const signal of ["SIGINT", "SIGTERM"] as const) {
    process.on(signal, () => {
      server.kill(signal);
    });
  }
}

// A server killed by a signal has no exit code, and the shell convention for one
// is 128 + the signal's number. Node hands back the name rather than the number,
// so the two this launcher can actually cause are named here and anything else
// is simply a failure.
const AFTER_SIGNAL: Readonly<Record<string, number>> = { SIGINT: 130, SIGTERM: 143 };

server.on("exit", (code, signal) => {
  process.exit(code ?? (signal === null ? 1 : (AFTER_SIGNAL[signal] ?? 1)));
});
