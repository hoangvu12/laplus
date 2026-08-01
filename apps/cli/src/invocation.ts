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
 * **The server owns the flags.** Everything is passed through untouched to
 * `laplus-server`, whose parser owns help, version and every refusal; this is a
 * launcher, not a second command line in front of the first one.
 */

/** The bundled UI, and whether it actually landed in the installed package. */
export type Bundle = {
  readonly directory: string;
  readonly present: boolean;
};

export type Invocation = {
  readonly arguments: readonly string[];
  readonly environment: Readonly<Record<string, string | undefined>>;
  /** Printed before the server starts. Not fatal — see [`invocation`]. */
  readonly warnings: readonly string[];
};

/**
 * Decide the run.
 *
 * **A caller who names a bundle keeps it.** `LAPLUS_UI` is launcher metadata,
 * and an existing value wins. A command-line `--ui` remains the Rust parser's
 * explicit override because its CLI precedence is higher than the environment.
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
  if ((environment.LAPLUS_UI ?? "").trim() !== "") {
    return { arguments: [...argv], environment, warnings: [] };
  }

  if (!bundle.present) {
    return {
      arguments: [...argv],
      environment,
      warnings: [
        `this package carries no UI bundle at ${bundle.directory}, so nothing is served at /.`,
        "the socket is unaffected: point a development UI at this server, or reinstall laplus.",
      ],
    };
  }

  return {
    arguments: [...argv],
    environment: { ...environment, LAPLUS_UI: bundle.directory },
    warnings: [],
  };
}
