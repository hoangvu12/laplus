/**
 * What this command does with what it was given, decided before anything is
 * spawned.
 *
 * All of it is one pure function so the interesting decisions are testable
 * without a binary on disk: whether the bundled UI is pointed at, whether the
 * caller's own `--ui` is left alone, and what is said when the bundle is
 * missing. `bin.ts` is then the part that cannot be tested this way — resolving
 * a package and running a process — and nothing else.
 *
 * **The server owns the flags.** Everything here except `--help` and
 * `--version` is passed through untouched to `laplus-server`, whose parser
 * refuses what it does not recognise; this is a launcher, not a second command
 * line in front of the first one.
 */

/** The bundled UI, and whether it actually landed in the installed package. */
export type Bundle = {
  readonly directory: string;
  readonly present: boolean;
};

export type Invocation =
  | { readonly kind: "help" }
  | { readonly kind: "version" }
  | {
      readonly kind: "run";
      readonly arguments: readonly string[];
      /** Printed before the server starts. Not fatal — see [`invocation`]. */
      readonly warnings: readonly string[];
    };

/**
 * The one page this command prints for itself.
 *
 * The flags are the server's, and `server/crates/laplus-server/src/launch.rs`
 * is the authority on them — this list has to be kept beside that one. It is
 * here at all because the server has no `--help` of its own: a reader who tried
 * the obvious thing would otherwise be told `unrecognised argument --help`,
 * which is true and useless.
 */
export const HELP = `laplus — the laplus server, and the UI it serves.

  npx laplus                  listen on 127.0.0.1:4773 and print a URL to open
  npx laplus --network        also listen off loopback, for a phone on this network

Keeping it running (Linux with systemd):

  npx laplus service install [flags]   start at boot, and survive logging out
  npx laplus service status            whether one is installed, and up to date
  npx laplus service uninstall         stop it and take it off startup

The flags given to \`service install\` are the ones the service runs with. The
pairing URL then goes to ~/.laplus/logs/service.log rather than to a terminal.

Flags, all of them the server's own:

  --port <n>                  the port to listen on (default 4773, LAPLUS_PORT)
  --network[=false]           leave loopback, for this run only (LAPLUS_NETWORK)
  --advertise-host <host>     the host to print for other machines to reach
                              (LAPLUS_ADVERTISE_HOST)
  --ui <dir>                  serve a bundle other than this package's own
                              (LAPLUS_UI)
  --help, --version           this page, and this package's version

The server drives the \`claude\` CLI on the machine it runs on, so that machine
needs its own \`claude\` installed and authenticated — not yours.

https://github.com/hoangvu12/laplus/blob/main/server/docs/running-headless.md
`;

/**
 * Decide the run.
 *
 * **A caller who names a bundle keeps it.** `--ui` given twice is an error in
 * the server's parser, and `--ui` on the command line beats `LAPLUS_UI` in the
 * environment there — so appending this package's own bundle to an invocation
 * that already carries either would turn a deliberate override into a failure
 * to start, or into a silent substitution of the bundle the caller asked for.
 * The developer pointing a published launcher at a working copy of the UI is
 * the whole reason that flag exists.
 *
 * **A missing bundle is a warning and not a refusal.** The server without
 * `--ui` is a perfectly good socket endpoint that answers 404 at `/`, which is
 * what it was before it could serve pages at all and still what a development
 * run against `pnpm dev` wants. Refusing to start would be this launcher
 * deciding that a server without a page is useless, which is not its call.
 */
export function invocation({
  argv,
  bundle,
  environment,
}: {
  readonly argv: readonly string[];
  readonly bundle: Bundle;
  readonly environment: Readonly<Record<string, string | undefined>>;
}): Invocation {
  if (argv.includes("--help") || argv.includes("-h")) return { kind: "help" };
  if (argv.includes("--version") || argv.includes("-v")) return { kind: "version" };

  if (namesABundle(argv, environment)) {
    return { kind: "run", arguments: [...argv], warnings: [] };
  }

  if (!bundle.present) {
    return {
      kind: "run",
      arguments: [...argv],
      warnings: [
        `this package carries no UI bundle at ${bundle.directory}, so nothing is served at /.`,
        "the socket is unaffected: point a development UI at this server, or reinstall laplus.",
      ],
    };
  }

  return { kind: "run", arguments: [...argv, "--ui", bundle.directory], warnings: [] };
}

/** Did the caller already say which bundle to serve, either way of saying it? */
function namesABundle(
  argv: readonly string[],
  environment: Readonly<Record<string, string | undefined>>,
): boolean {
  const flagged = argv.some((argument) => argument === "--ui" || argument.startsWith("--ui="));
  return flagged || (environment.LAPLUS_UI ?? "").trim() !== "";
}
