# 09 — Provider configuration and agent binary resolution

**What to build:** The server finds the developer's installed Claude Code binary
and the UI shows a configured, ready provider instance. When the binary is missing,
the developer gets a diagnostic naming exactly what was looked for and where it
looked — enough to fix it without opening a log file.

Resolution order is: an explicitly configured path, then a lookup on PATH, then a
clear failure. On Windows the executable is a native binary, so the upstream
server's npm shim resolution logic is deliberately not ported — it is dead weight
here.

No agent is spawned by this ticket. It establishes that the driver exists, is
locatable, and is configured.

**Blocked by:** 03 (Socket endpoint, local handshake, and the configuration
method).

**Status:** done

- [x] The agent binary is located on PATH without configuration on a machine where
      it is installed
- [x] An explicitly configured path takes precedence over the PATH lookup
- [x] A missing binary produces a diagnostic naming both the configured path (if
      any) and the fact that PATH was searched
- [x] A configured path that exists but is not executable is reported distinctly
      from one that does not exist
- [x] The UI shows the provider instance as configured and ready
- [x] The resolved binary's version is reported, so a pinned known-good version can
      be confirmed
- [~] Tests cover the resolution order and each failure mode through the socket
      boundary, without requiring a real agent binary to be installed — every
      *answer* is read through the socket and no test needs an agent installed, but
      the **cause** cannot be: there is no method on this wire that means "re-probe".
      One test drives the real trigger; the rest inject it. See below.

## Comments

### The lookup moved out of the child process, and that is the whole ticket

The reference server finds out whether `claude` is installed by running it and
seeing what happens. When that fails it says:

> Claude Agent CLI (`claude`) is not installed or not on PATH.

True, and no use to the developer it is addressed to. They have installed it —
that is why they are looking at this app — and the sentence names neither what was
looked for nor where, so the one thing they cannot do with it is find out why the
server disagrees. Everything in `crate::provider` follows from refusing to send
that sentence.

So the lookup happens in the server, before anything is spawned.
`process::Search` is the directories and suffixes resolution may see;
`provider::Located` is what it found, or what it tried. The diagnostic is then not
a string a failure path had to guess at, it is a rendering of a value:

> The configured Claude Code binary path C:\tools\claude\claude.exe does not
> exist. claude.exe was then looked for in 34 PATH directories: C:\Windows\system32;
> …

Two side effects, and both were worth more than the sentence:

- **The whole module is testable with no agent installed.** Spec story 61 wants
  agent-facing tests deterministic, offline and free, and the last criterion here
  says so again. A probe that could only be exercised where Claude Code happens to
  be installed would fail that twice over. Every test writes its own stand-in
  binary — a `.cmd` on Windows, a shell script elsewhere — and hands its directory
  to a `Search`.
- **`PATH` is never read as ambient state.** It is process-global and mutable, so
  a test that had to set it could not run beside another one. Passing the
  directories in is what makes `socket_provider.rs` safe to run in parallel with
  the rest of the suite, and it is also the only reason the socket tests can drive
  the `PATH` half of the resolution order at all.

`editor::on_path` was the same walk, written first, and carried a latent bug the
shared version does not: it appended suffixes with `Path::with_extension`, which
*replaces* the last extension rather than adding one, so a command whose name
contained a dot would have been probed under the wrong name entirely. No editor id
in `EDITORS` contains a dot, so nothing was actually broken — but `claude.exe` as a
configured `binaryPath` does, and the resolver had to get it right. Both now go
through the same `Search`, which appends, and the editor list builds one `PATH`
split rather than twenty-one.

### The one judgement call: the fallback is asymmetric

A configured path that **is not there** falls through to `PATH`. A configured path
that **is there and cannot be started** does not.

The first is the ticket's stated order taken literally — configured path, then
PATH, then a clear failure — and the case it serves is a setting that outlived an
install: the CLI moved, or was reinstalled somewhere else, and the working one is
on `PATH`. It is also what makes the third criterion satisfiable as written, since
"naming both the configured path and the fact that PATH was searched" requires that
PATH was in fact searched.

Falling back silently would be the wrong half of that, so it is not silent: a
provider that resolved this way is `ready` **and** carries a message, which is the
only case where a working provider says anything at all.

> The configured Claude Code binary path C:\old\claude.exe does not exist, so
> C:\Users\me\.local\bin\claude.exe was used instead, found on PATH.

The second case does not fall back because falling back is precisely what would
make it indistinguishable from the first, and the fourth criterion asks for them to
be distinct. A path that exists and is the wrong kind of file is not a stale
setting; it is a statement about a specific file, usually the install *directory*
where the binary was meant to go. Quietly running something else would hide a
mistake the developer is one sentence away from fixing.

`provider::tests::a_configured_path_that_cannot_be_started_does_not_fall_back`
asserts this against a `Search` that *does* hold a working binary, so the test
fails if the fallback ever becomes symmetric.

### The models table, and why the version is load-bearing

**This is scope the ticket did not ask for**, so the argument for it comes first.
No checklist item mentions models. But `providers` was ticket 09's field to fill
and `models` is a required member of the snapshot, so the choice was between the
real list and an empty one, not between doing this and doing nothing.

An empty one is survivable: the UI falls back to
`DEFAULT_MODEL_BY_PROVIDER["claudeAgent"]`, a constant in `apps/web`, and the
composer would run turns on `claude-sonnet-5` with an empty model picker. That puts
the decision about which model the agent runs in the client, which is the wrong
side of this seam, and shows the developer a provider they cannot choose anything
for. Slugs are what the CLI is given as `--model`; that is provider
*configuration*, which is this ticket's subject.

So `provider::BUILT_IN_MODELS` is upstream's table, in upstream's order — the
order matters, because the UI's default is the first non-custom model a provider
reports.

Two smaller pieces went the other way. **Custom slugs from
`settings.providers.claudeAgent.customModels` were implemented and then removed**:
nothing can write that setting until ticket 22, so the code was a branch no test
could reach through any real path, and "there will be no second place to change"
is not worth carrying unreachable behaviour for. The setting is still on the wire,
as it was before this ticket. And **`capabilities` is `null`**, which the contract
permits: the descriptors behind it are the reasoning-effort, fast-mode and
context-window toggles, which are turn parameters, and advertising a
"Reasoning: Max" control whose value this server would drop on the floor is worse
than not advertising it.

**A table, because there is nothing to ask.** Ticket 03 left a note saying this
ticket would fill `settings.textGenerationModelSelection` "once model slugs are
known from the CLI". There is no such call: the CLI cannot be asked what models it
accepts, which is why upstream hardcodes them too. The conformance declaration for
that field has been corrected rather than left standing — what is actually missing
there is a *stored preference* over these slugs, which is ticket 22's.

Correcting it turned up something worth leaving where the next ticket will find
it, and the corrected declaration says so: the field's decoding default in
`settings.ts` is `{instanceId: "codex", model: DEFAULT_GIT_TEXT_GENERATION_MODEL}`.
So "absent, and the client fills in a default" is not free here — the default names
an instance v1 does not ship. Nothing reads it yet, because generated thread titles
and commit messages are later tickets, but whichever ticket wants them has to send
this field rather than inherit the default.

What a table costs is that it goes stale, and the version gate is what keeps the
staleness off the screen in the direction that matters. Each entry carries the
first `claude` that accepts its slug, so an install at 2.1.100 is never offered
`claude-opus-5`, and a version that could not be read offers only the slugs every
version knows. That is what makes the sixth criterion more than a label: the
version is not displayed, it decides what the UI may ask for.

**The gate is silent, so `version_advice` speaks for it.** Filtering a model out
and saying nothing is the one combination that leaves a developer with a shorter
list than the release notes promised and no way to find out why, so a `ready`
provider on an old CLI carries a sentence naming what is out of reach and the
version that reaches it. Upstream produces the same sentence and picks the
*nearest* unreachable model — for 2.1.100 it says "too old for Claude Opus 4.7,
upgrade to v2.1.111", which is a smaller ask that leaves three more models
unmentioned. This names the newest, because one sentence that clears the whole
table is worth more than the first of four.

Two declared divergences fell out of the gate, and review caught that neither was
written down:

- **Upstream sends the unfiltered table when it has no version** (`ClaudeProvider.ts:795`,
  `:958`) and only filters once one has been read, so it will offer
  `claude-opus-5` for a CLI it has established nothing about. This offers only the
  ungated slugs. Both are defensible; a slug that cannot work is worse than a
  shorter list.
- **The advice above was initially dropped**, which would have been the silent
  half of the same decision. It is implemented now, and
  `the_models_offered_follow_the_version_that_answered` asserts both ends: the old
  CLI is told, the current one is not.

### Probing is a method, not a mode — and the first attempt got this wrong

The first version of this took a `ProviderProbe { OnStart, Deferred }` parameter on
`Server::bind_with`: the app asked for the probe, the suite asked for it to be
skipped. Review measured that against the spec's own rule and it fails —

> It is injected through the existing agent-executable-path configuration — a value
> the server already needs for real use, **so no test-only seam is added to
> production code.**

`Deferred` had no non-test caller. It was a switch in the startup path whose only
purpose was to change what happens under test, which is exactly the thing that
sentence rules out.

What replaced it is `Server::probe_provider`, a method with a real job:
`Server::bind` calls it after binding, and it is what a five-minute refresh or a
settings change will call again — the reference server re-probes on both. Nothing
about it is test-shaped, and the test that drives it drives *it*, not a variant of
it.

The reason it is not simply part of binding is worth keeping: resolving means
walking `PATH` and then waiting on a child, and a socket that did not open until the
agent had answered would not open at all on a machine where the agent is wedged.
So it returns at once and publishes when it knows.

What remains injected is a `Search` — the directories a lookup may see. That is
data a real caller supplies, not a mode, and the resolution it drives is the whole
production path with a different list of directories. It has to be an argument
because `PATH` is process-global: a test that set it would be changing it for every
test running beside it.

### There is no provider instance until something has looked, and that is the answer

The first version reported a placeholder here too — `status: "warning"`,
`installed: false`, "the binary has not been looked for yet" — and claimed the UI
would show "Checking provider status". Review checked, and it does not:

- `getProviderSummary` tests `!provider.installed` **before** it looks at `status`,
  so the placeholder renders as **"Not found"**. "Checking provider status" is the
  branch for an *absent* instance.
- `shouldShowProviderStatusBanner` returns true for anything that is not `ready` or
  `disabled`, so every single launch would flash a warning alert into the chat view
  until the probe landed.

Upstream has the same flash, which is not a reason to keep it when the alternative
costs nothing. `providers` is now empty until the probe answers — which is the state
upstream's own copy is written for, "Waiting for the server to report installation
and authentication details" — and `ServerConfig::detect` is back to building it that
way.

That turned out to be the better answer for conformance too, not a concession.
`socket_conformance.rs` now resolves a stand-in agent reporting a current version
*before* it compares, so what is held against the capture is a **ready provider with
a version string** rather than a placeholder — like for like. Two declarations
disappeared as a result: `providers[0].version` is no longer a retype, and
`providers[0].message` is no longer an addition, which puts the `ADDED` list back to
empty where its own comment says it belongs.

`detect()` is still not free — `available_editors` stats every candidate command
against every `PATH` entry, and an earlier draft of its doc comment claimed
otherwise in the same sentence that described the walk. What it does guarantee is
that it starts **no child process**, which is the property that matters: it runs
before the listener exists.

### What the UI actually ends up showing

Worth writing down, because "configured and ready" is a criterion and the UI
computes it from three fields rather than one:

- `isProviderInstancePickerReady` is `enabled && isAvailable && status === "ready"`.
  `availability` is absent, which the contract reads as available, so a resolved
  binary satisfies all three and the instance is offered.
- `ProviderStatusBanner` returns `null` for `ready`, so a working provider raises
  nothing.
- `getProviderSummary` reads `auth.status` before `status`, and ours is `unknown`
  — nothing in this ticket reads a credential. The settings card therefore says
  "Available · Installed and ready, but authentication could not be verified",
  which is exactly the state of the world after this ticket and not a warning about
  it.
- `message` is what the card's detail line and the banner's body render, which is
  why the diagnostic goes there and not only to stderr.
- `versionAdvisory` is absent, and an earlier draft justified that with "an update
  check is a network call on boot". Review showed that reason is wrong: upstream
  emits the field with `status: "unknown"` and four nulls when checks are off — the
  `grok` entry in `fixtures/socket-wire/02-request-response.ndjson` is exactly that
  — and `updateCommand` needs no network. The real reason is better:
  `getProviderVersionAdvisoryPresentation` returns `null` for an `unknown` advisory
  and for a missing one alike, so the two render identically, and nothing here has a
  latest version to put in one.

The resolved path is deliberately **not** on the wire: `ServerProvider` has no
field for it, and an invented key risks the whole configuration payload. It goes to
stderr instead, once per probe, because "which binary is it actually running" is
the first question a developer asks when a turn misbehaves.

### A second clock, hand-rolled

`checkedAt` is a required `IsoDateTime` and there is no date crate in the
workspace. `store.rs` gets its timestamps from SQLite's `strftime` because the
registry's clock has to be the database's; a provider snapshot never reaches
SQLite, so asking the database what time it is would mean taking a lock to answer
a question that has nothing to do with it.

So `provider::now_iso` is thirty lines of Howard Hinnant's `civil_from_days`,
which is the standard way to get a civil date out of a day count without a leap
table. Adding a dependency for one string, in a repo that walks `PATH` by hand
rather than spawn `where`, would have been the odd choice. It is pinned against
five known instants including a leap day and the year 2100, and
`the_clock_renders_the_way_the_registrys_does` compares its layout digit for digit
against SQLite's — one payload carries both clocks and a client parses both with
the same `new Date`.

### What the tests can and cannot assert

The two files divide the work rather than covering it twice — an earlier draft had
five tests in both, which review flagged, and each is now in exactly one place.

- **`provider.rs`** (17 tests) holds the rules, where a failure names the case
  rather than a JSON pointer and a private function can be reached: precedence, the
  asymmetric fallback, the name the fallback looks for, the two unstartable-file
  shapes, every clause of the missing-binary sentence, the three ways a binary that
  ran can answer, version parsing at its edges, the model gate at its exact
  boundary (2.1.218 against 2.1.219), the advice, and the clock.
- **`socket_provider.rs`** (16 tests) drives only what the UI observes. Precedence
  is asserted through the reported *version* rather than the resolved path — two
  fakes, two versions — because the version is the outcome and the path is the
  mechanism.
- **The cause cannot come over the socket, and the file says so.** There is no
  method on this wire meaning "re-probe the provider": upstream refreshes on a timer
  and on settings changes, and lightcode has neither yet. So one test drives the
  real trigger (`Server::probe_provider`) and the rest inject the same resolution
  with directories of their own. Review was right to call the criterion partial on
  this; what was missing was the sentence admitting it, not a mechanism.
- **The probe timeout is driven, in fifty milliseconds.** `probe` takes its patience
  as an argument — a private function's parameter, not a public seam — so
  `a_binary_that_does_not_answer_is_given_up_on` uses a binary that sleeps for a
  second and asserts the giving-up happened on the deadline rather than on the
  child.
- **`the_real_agent_on_this_machine_is_found_and_reports_its_version`** is the one
  test that resolves the genuinely installed CLI, skipped unless
  `LIGHTCODE_TEST_REAL_AGENT` is set, the same way
  `editor::tests::the_file_manager_can_be_started` is. Everything else proves the
  resolver against a file the test wrote, and not one of those would notice if the
  real binary turned out to be something this server cannot start. Run here, it
  resolves `C:\Users\ADMIN\.local\bin\claude.exe` and reports `2.1.220`; the
  built binary logs the same path on startup.
- **The `.cmd` fakes prove the npm-shim claim as a side effect.** The ticket says
  shim following is dead weight; a test binary that *is* a launcher script and
  still resolves, starts and reports a version is the evidence, since
  `std::process::Command` runs a batch file through `cmd.exe` where the Claude
  Agent SDK could not.

### What review caught, and what the evidence was

Six things changed the code rather than the prose. The three biggest are written up
in their own sections above — the test-only `ProviderProbe` seam, the placeholder
provider that would have flashed a warning banner on every launch, and the silently
dropped version advice. The rest:

- **Two comments asserted things that were false.** `detect()`'s doc claimed "no
  I/O" in the same sentence that described a `PATH` walk, and the `versionAdvisory`
  declaration blamed a network call that upstream does not make. Both are corrected
  above; in a codebase where comments carry the evidence, a wrong one is a defect
  and not a typo.
- **Two comments elsewhere had been left stale by this diff.**
  `config_store.rs` still said "nothing mutates the configuration yet — ticket 09
  fills `providers`", which this ticket made false, and `projects.rs` still routed
  `defaultModelSelection` to "ticket 09, once model slugs are known" — the premise
  this ticket demolished. Neither file is otherwise touched by the change, which is
  exactly why they were easy to miss.
- **`disabled()` hand-built all thirteen fields** while `snapshot()`'s doc claimed
  every field came from it. It now calls `snapshot()`, which makes the claim true
  and removes the duplication; `enabled` comes off the settings either way, so the
  two were identical.
- **Five tests existed in both the unit and the socket file.** They are now in one
  place each, and both headers say which place and why.

### Not covered automatically

- **A resolved file that then refuses to start.** `Probed::Unstartable` covers the
  window between resolving a path and `CreateProcess` rejecting it — a file deleted
  in between, or an executable for the wrong architecture. Neither is arrangeable
  from a test without shipping a broken binary.
- **Killing a wedged launcher script.** The timeout kills the child this server
  started, which for a `.cmd` on Windows is `cmd.exe` rather than the program behind
  it: std spawns batch files through the shell and `TerminateProcess` does not walk
  the tree. The real agent is a native `claude.exe` spawned directly, where the kill
  does what it says; a wedged shim would leave a grandchild for the OS to reap at
  exit. A job object would fix it and is not worth it for a `--version`.
- **`versionAdvisory`, slash commands, skills and authentication.** All four are
  declared divergences in `socket_conformance.rs` with the ticket that owns them.
  The first is off by design; the other three need the agent's own initialisation
  output or its directories, which is the ticket that runs one.
- **The model table being right.** The gate stops a slug being offered to a CLI too
  old for it. Nothing can check that a slug the table claims exists really does —
  only a turn can, and that is ticket 10.
- **A `PATH` entry that is not a directory.** Junk on `PATH` is walked past
  silently, which is what a shell does; there is no test because there is no
  behaviour to observe.
- **`PATHEXT` as ambient state.** A `Search` takes its directories from the caller
  but still reads `PATHEXT` from the process. It is effectively constant per machine
  and is not a thing a test needs to vary, so the claim "`PATH` is never read as
  ambient state" is about directories and should be read as exactly that.
