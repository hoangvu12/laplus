//! The agent this server drives: where its binary is, whether it will run, and
//! what the UI is told about it.
//!
//! **Nothing here starts an agent.** What this establishes is that the driver
//! *can* be started — the binary is located, it answers `--version`, and the
//! answer is published as the one provider instance the UI routes turns
//! through. The tickets after this one spawn it for real.
//!
//! ## Resolution happens here, not in a child process
//!
//! The obvious way to find out whether `claude` is installed is to run it and
//! see what happens, which is what the reference server does. It produces
//! diagnostics like "Claude Agent CLI (`claude`) is not installed or not on
//! PATH" — true, and no use to a developer who *has* installed it, somewhere the
//! server did not look, because the sentence names neither what was looked for
//! nor where.
//!
//! So the lookup is done first, by this server: a [`Search`] is the directories
//! resolution may see and [`Located`] is what it found or what it tried, which
//! is where the diagnostic comes from. It is also what makes this module
//! testable on a machine with no `claude` on it at all — the suite has to run
//! offline and for free (spec story 61), and a provider probe that could only be
//! exercised where the agent happens to be installed would fail that twice over.
//!
//! ## What is deliberately not ported
//!
//! - **npm shim following.** Upstream resolves a `claude.cmd` launcher to the
//!   package entry behind it, because the Claude Agent SDK spawns
//!   `pathToClaudeCodeExecutable` with no shell and `CreateProcess` cannot run a
//!   batch file. lightcode drives the CLI itself and hands
//!   `std::process::Command` a resolved absolute path, and std runs a `.cmd`
//!   through `cmd.exe` on our behalf; on the platform v1 ships the executable is
//!   a native `claude.exe` in any case. The ticket calls it dead weight, and it
//!   is.
//! - **`homePath` and `launchArgs`.** Both describe the environment the *agent*
//!   runs in. `--version` needs neither, and the process that does belongs to
//!   the ticket that starts one.
//! - **`versionAdvisory`.** Absent, and absent is not a lesser form of what
//!   upstream sends with update checks off. It would emit the field with
//!   `status: "unknown"` and four nulls — the `grok` entry in
//!   `fixtures/socket-wire/02-request-response.ndjson` shows exactly that — and
//!   `getProviderVersionAdvisoryPresentation` returns `null` for an `unknown`
//!   advisory and for a missing one alike. So the two render identically, and
//!   nothing here has a latest version or an update command to put in one.
//! - **Authentication.** No credential is read, so `auth.status` is `unknown`,
//!   which is the contract's own literal for exactly that. The UI renders
//!   "Installed and ready, but authentication could not be verified" and lets
//!   the instance be selected, which is the honest state of the world after this
//!   ticket.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::clock::now_iso;
use crate::config::{
    AuthStatus, ClaudeSettings, Provider, ProviderAuth, ProviderModel, ProviderState,
};
use crate::config_store::{ConfigChange, ConfigStore};
use crate::process::Search;

/// The routing key the client uses for the one provider instance v1 ships, and
/// the driver slug that selects the implementation behind it.
///
/// Both are `claudeAgent`, and neither is a closed literal in the contract —
/// `ProviderDriverKind` is a deliberately open slug so a fork can add a driver.
/// What makes this the only usable value is that the UI keys tables off it:
/// `DEFAULT_MODEL_BY_PROVIDER`, the settings map at
/// `settings.providers.claudeAgent`, and the driver's own label. A slug of our
/// own invention would decode and then miss every one of them.
pub const INSTANCE_ID: &str = "claudeAgent";

/// What the UI calls this provider. `displayName` is optional in the contract
/// and the client falls back to title-casing the driver slug, which would read
/// "Claude Agent"; the capture shows the reference server sending "Claude".
const DISPLAY_NAME: &str = "Claude";

/// The name to look for when the setting does not name a path. Also the setting's
/// own default — see [`ClaudeSettings::binary_path`].
const DEFAULT_NAME: &str = "claude";

/// How long the binary has to answer `--version`.
///
/// Ten seconds is not a performance budget, it is a deadlock guard: the only
/// thing being waited for is a version string, and a binary that has not
/// produced one by now is not going to. The cost of the timeout being too
/// generous is a probe thread that lives longer than it should; the cost of it
/// being too tight is a provider reported broken on a machine that was merely
/// busy, which is worse.
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// How often the probe looks to see whether the child has finished.
const PROBE_POLL: Duration = Duration::from_millis(20);

// ---------------------------------------------------------------------------
// Resolution
// ---------------------------------------------------------------------------

/// What a lookup for the agent binary found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Located {
    /// A path that can be started, and which strategy produced it.
    Binary { path: PathBuf, source: Source },
    /// The configured path names something this machine cannot start — a
    /// directory, or a file that is not a program.
    ///
    /// Its own case rather than folded into [`Located::Nothing`], because the
    /// fix is a different one: the file is *there*, and what is wrong is which
    /// file it is. Resolution stops here rather than falling back to `PATH`,
    /// which is the one place it does not — see [`resolve`].
    NotExecutable { configured: PathBuf },
    /// Nothing startable in any of the places that were looked.
    Nothing {
        /// The configured path, when the setting named a place. Absent when it
        /// named a bare command, which is a thing to look *for* rather than a
        /// place to look.
        configured: Option<PathBuf>,
        /// The name that was looked for in each of `directories`.
        name: String,
        directories: Vec<PathBuf>,
    },
}

impl Located {
    /// The binary and how it was found, or the sentence saying why there is
    /// none.
    ///
    /// The one place [`Located`]'s three variants are collapsed to two, so that
    /// no caller has to match a variant it has already excluded — and so that a
    /// fourth variant, if one is ever needed, is a compile error here rather
    /// than something a catch-all absorbs silently.
    ///
    /// The sentence is the ticket 09 diagnostic verbatim: it names the
    /// configured path when there was one, what was looked for, and every
    /// directory it was looked for in. Two callers want it — the provider
    /// snapshot the UI shows at rest, and a turn that went to start an agent and
    /// found none — and a second wording for the same fact would drift from the
    /// first.
    pub fn startable(self) -> Result<(PathBuf, Source), String> {
        match self {
            Located::Binary { path, source } => Ok((path, source)),
            // `installed: false` although the file exists, because what
            // `installed` claims is that there is an agent here — and a
            // directory or a text file is not one.
            Located::NotExecutable { configured } => Err(format!(
                "The configured Claude Code binary path {} exists but is not a program this \
                 machine can start. {} PATH was not searched, because a path was configured \
                 and something is there.",
                configured.display(),
                what_would_start(),
            )),
            Located::Nothing {
                configured,
                name,
                directories,
            } => Err(not_found(configured.as_deref(), &name, &directories)),
        }
    }
}

/// Which of the two strategies produced the binary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// `settings.providers.claudeAgent.binaryPath`, read as a path.
    Configured,
    /// Found on `PATH`.
    OnPath {
        /// A configured path that was tried first and was not there. `Some`
        /// means the developer asked for one binary and is getting another,
        /// which they are told about rather than left to discover.
        instead_of: Option<PathBuf>,
    },
}

/// Find the agent binary: the configured path first, then `PATH`.
///
/// A setting that contains a separator is a **place**; one that does not is a
/// **name**. That is why the default `claude` searches rather than resolving to
/// a file called `claude` in whatever directory the server happens to have been
/// started in — which would be a way to run a program nobody chose.
///
/// The fallback is asymmetric on purpose, and it is the one judgement call in
/// this function:
///
/// - A configured path that **is not there** falls through to `PATH`. A path
///   that has gone missing is usually a setting that outlived an install — the
///   CLI moved, or was reinstalled elsewhere — and `PATH` is where the current
///   one is. The developer is told which binary answered instead.
/// - A configured path that **is there and cannot be started** does not. That
///   is not a stale setting, it is a statement about a specific file, and
///   quietly running a different program because the named one is the wrong kind
///   of file would hide the mistake instead of reporting it. The ticket asks for
///   this case to be reported distinctly, and falling back is the one thing that
///   would make it indistinguishable.
pub fn resolve(configured: &str, search: &Search) -> Located {
    let configured = configured.trim();
    let explicit = names_a_path(configured).then(|| PathBuf::from(configured));

    if let Some(path) = &explicit {
        if let Some(startable) = search.startable(path) {
            return Located::Binary {
                path: startable,
                source: Source::Configured,
            };
        }
        if path.exists() {
            return Located::NotExecutable {
                configured: path.clone(),
            };
        }
    }

    // The name to look for on `PATH`: the configured path's own file name when
    // there was one, so a stale `C:\old\claude.exe` looks for `claude.exe`
    // rather than for something the developer never mentioned.
    let name = explicit
        .as_ref()
        .and_then(|path| path.file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| match configured.is_empty() {
            true => DEFAULT_NAME.to_string(),
            false => configured.to_string(),
        });

    match search.locate(&name) {
        Some(path) => Located::Binary {
            path,
            source: Source::OnPath {
                instead_of: explicit,
            },
        },
        None => Located::Nothing {
            configured: explicit,
            name,
            directories: search.directories().to_vec(),
        },
    }
}

/// Does this setting name a place on disk rather than a command to look up?
///
/// A separator is what decides, which is the same rule every shell uses. A bare
/// `claude` is a name; `.\claude.exe`, `/usr/local/bin/claude` and
/// `C:\Users\me\.local\bin\claude.exe` are places.
fn names_a_path(configured: &str) -> bool {
    configured.contains('/') || configured.contains('\\')
}

// ---------------------------------------------------------------------------
// The version probe
// ---------------------------------------------------------------------------

/// What running `<binary> --version` produced.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Probed {
    /// It ran, exited cleanly, and named a version.
    Version(String),
    /// It ran and exited cleanly, and nothing in the output was a version this
    /// server could read.
    Unreadable { output: String },
    /// It ran and exited non-zero. The version is whatever could still be read
    /// out of the output, which is often nothing.
    Failed {
        code: Option<i32>,
        version: Option<String>,
        output: String,
    },
    /// It could not be started at all — the file was resolved and then would not
    /// run.
    Unstartable { error: String },
    /// It never finished.
    TimedOut,
}

/// Ask the binary its version, waiting at most `patience`. Blocking.
///
/// `patience` is a parameter of a private function rather than the constant read
/// directly, so the timeout branch can be driven in milliseconds instead of ten
/// seconds. Every caller passes [`PROBE_TIMEOUT`].
fn probe(path: &Path, patience: Duration) -> Probed {
    let mut command = Command::new(path);
    command
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    crate::process::without_a_console(&mut command);

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return Probed::Unstartable {
                error: error.to_string(),
            }
        }
    };

    // Polled rather than waited on: `std::process::Child` has no bounded wait,
    // and a binary that never answers must not hold this thread for the life of
    // the process.
    //
    // Reading the output only *after* the child has finished is safe for exactly
    // one reason — `--version` writes one short line, far below a pipe buffer,
    // so it cannot block on a write nobody is draining. Anything chattier would
    // need a reader thread per stream, and the ticket that streams a turn builds
    // one.
    let deadline = Instant::now() + patience;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() >= deadline => {
                // Kills the child this server started, which for a `.cmd` on
                // Windows is `cmd.exe` and not the program behind it — std
                // spawns launcher scripts through the shell, and `TerminateProcess`
                // does not walk the tree. The real agent is a native
                // `claude.exe`, spawned directly, where this does what it says;
                // a wedged shim would leave a grandchild for the OS to reap when
                // the server exits. Not worth a job object for a `--version`.
                let _ = child.kill();
                let _ = child.wait();
                return Probed::TimedOut;
            }
            Ok(None) => std::thread::sleep(PROBE_POLL),
            Err(error) => {
                return Probed::Unstartable {
                    error: error.to_string(),
                }
            }
        }
    }

    let output = match child.wait_with_output() {
        Ok(output) => output,
        Err(error) => {
            return Probed::Unstartable {
                error: error.to_string(),
            }
        }
    };

    // Both streams, because a CLI is free to put its version on either and
    // upstream reads both for the same reason.
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout).trim(),
        String::from_utf8_lossy(&output.stderr).trim()
    );
    let text = text.trim().to_string();
    let version = version_in(&text);

    match (output.status.success(), version) {
        (true, Some(version)) => Probed::Version(version),
        (true, None) => Probed::Unreadable { output: text },
        (false, version) => Probed::Failed {
            code: output.status.code(),
            version,
            output: text,
        },
    }
}

/// The first three-part number in the output.
///
/// `claude --version` answers `2.1.220 (Claude Code)`, and the parse is as loose
/// as the reference server's `parseGenericCliVersion` for the same reason:
/// whatever the CLI decides to print around the number, the number is still
/// found. A leading `v` is tolerated where upstream's word-boundary regex would
/// refuse it — a deliberate, one-directional divergence, since the alternative is
/// reporting a working install as unreadable over a display convention.
fn version_in(output: &str) -> Option<String> {
    let text = output.as_bytes();
    for start in 0..text.len() {
        if !text[start].is_ascii_digit() {
            continue;
        }
        // Only the start of a number, so `2.1.220` is not also tried from its
        // `1` and its `220`.
        if start > 0 && (text[start - 1].is_ascii_digit() || text[start - 1] == b'.') {
            continue;
        }
        if let Some(length) = three_part_number(&text[start..]) {
            return Some(output[start..start + length].to_string());
        }
    }
    None
}

/// How many bytes of `text` are `<digits>.<digits>.<digits>`, if that is how it
/// begins. Trailing parts are left where they are, so a four-part version
/// reports its first three — which is what upstream's regex does too.
fn three_part_number(text: &[u8]) -> Option<usize> {
    let mut at = 0;
    for part in 0..3 {
        if part > 0 {
            if text.get(at) != Some(&b'.') {
                return None;
            }
            at += 1;
        }
        let digits = text[at..]
            .iter()
            .take_while(|byte| byte.is_ascii_digit())
            .count();
        if digits == 0 {
            return None;
        }
        at += digits;
    }
    Some(at)
}

// ---------------------------------------------------------------------------
// Models
// ---------------------------------------------------------------------------

/// A model the agent can be asked for, and the oldest CLI that knows the slug.
struct BuiltIn {
    slug: &'static str,
    name: &'static str,
    /// The first `claude` version that accepts this slug. `None` means every
    /// version old enough to be worth talking to does.
    since: Option<(u64, u64, u64)>,
}

/// `BUILT_IN_MODELS` from `ClaudeProvider.ts`, in the same order — the order is
/// load-bearing, because the UI's default model is the first non-custom entry a
/// provider reports.
///
/// **A table, because there is nothing to ask.** The CLI has no "list your
/// models" call; upstream hardcodes exactly this list, so a fork that wanted to
/// discover it would be inventing a protocol. What that costs is that the table
/// goes stale, and the version gate is what stops the staleness being
/// user-visible in the direction that matters: a slug the installed CLI has
/// never heard of is never offered.
const BUILT_IN_MODELS: &[BuiltIn] = &[
    BuiltIn {
        slug: "claude-fable-5",
        name: "Claude Fable 5",
        since: Some((2, 1, 169)),
    },
    BuiltIn {
        slug: "claude-opus-5",
        name: "Claude Opus 5",
        since: Some((2, 1, 219)),
    },
    BuiltIn {
        slug: "claude-opus-4-8",
        name: "Claude Opus 4.8",
        since: Some((2, 1, 154)),
    },
    BuiltIn {
        slug: "claude-opus-4-7",
        name: "Claude Opus 4.7",
        since: Some((2, 1, 111)),
    },
    BuiltIn {
        slug: "claude-opus-4-6",
        name: "Claude Opus 4.6",
        since: None,
    },
    BuiltIn {
        slug: "claude-opus-4-5",
        name: "Claude Opus 4.5",
        since: None,
    },
    BuiltIn {
        slug: "claude-sonnet-5",
        name: "Claude Sonnet 5",
        since: None,
    },
    BuiltIn {
        slug: "claude-sonnet-4-6",
        name: "Claude Sonnet 4.6",
        since: None,
    },
    BuiltIn {
        slug: "claude-haiku-4-5",
        name: "Claude Haiku 4.5",
        since: None,
    },
];

/// The models to offer for a CLI at `version`.
///
/// An unknown version — a probe that failed, or a driver switched off before one
/// ran — offers only the ungated models. That is the safe direction: those slugs
/// work on every version this server would find, so the picker is never populated
/// with a model the agent will reject.
///
/// **A declared divergence.** Upstream sends the *unfiltered* table in exactly
/// those cases (`ClaudeProvider.ts:795`, `:958`) and only filters once a version
/// has been read, so it will offer `claude-opus-5` for a CLI it has established
/// nothing about. Both are defensible — theirs keeps the picker full, this keeps
/// it truthful — and this one is chosen because a slug that cannot work is worse
/// than a shorter list.
fn models(version: Option<&str>, custom: &[String]) -> Vec<ProviderModel> {
    let installed = version.and_then(parse_version);
    let mut models: Vec<ProviderModel> = BUILT_IN_MODELS
        .iter()
        .filter(|model| match model.since {
            None => true,
            Some(since) => installed.is_some_and(|installed| installed >= since),
        })
        .map(|model| ProviderModel {
            slug: model.slug.to_string(),
            name: model.name.to_string(),
            is_custom: false,
            capabilities: None,
        })
        .collect();

    // The developer's own slugs, from `settings.providers.claudeAgent.customModels`
    // — ticket 22's "including model selection". **After** the built-in ones and
    // never version-gated, and both follow from what a custom model *is*: this
    // server has no table to check it against, so it is offered on the
    // developer's word, and the UI's default is the first non-custom entry — so
    // adding one at the front would silently change what a new conversation
    // starts with.
    models.extend(custom.iter().filter(|slug| !slug.trim().is_empty()).map(|slug| {
        ProviderModel {
            slug: slug.trim().to_string(),
            // No display name to give: the developer typed a slug, and inventing
            // a prettier version of it would show them something they did not
            // write.
            name: slug.trim().to_string(),
            is_custom: true,
            capabilities: None,
        }
    }));
    models
}

/// What to tell a developer whose CLI is too old for part of the table.
///
/// The gate above is silent by design — a model that cannot work is simply not
/// offered — and silence is the wrong answer on its own: the developer sees a
/// shorter list than the release notes promised and nothing saying why. Upstream
/// produces the same sentence (`ClaudeProvider.ts:890`), and dropping it while
/// keeping the filter would have been the one combination that leaves the user
/// with no way to find out.
///
/// Names the **newest** model out of reach and the version that reaches it, where
/// upstream's cascade names the *nearest* one — for a CLI at 2.1.100 it says
/// "too old for Claude Opus 4.7, upgrade to v2.1.111", which is a smaller ask
/// that then leaves three more models unmentioned. One sentence that clears the
/// whole table is worth more than the first of four.
fn version_advice(version: &str) -> Option<String> {
    let installed = parse_version(version)?;
    let (newest, (major, minor, patch)) = BUILT_IN_MODELS
        .iter()
        .filter_map(|model| model.since.map(|since| (model.name, since)))
        .filter(|(_, since)| installed < *since)
        .max_by_key(|(_, since)| *since)?;

    Some(format!(
        "Claude Code v{version} is too old for {newest}, so it is not offered. \
         Upgrade to v{major}.{minor}.{patch} or newer to use it."
    ))
}

/// A three-part version as numbers, for comparing.
///
/// Only ever fed [`version_in`]'s output, which is three groups of ASCII digits,
/// so the parse cannot fail on anything this server produced — the `Option` is
/// for the caller that has no version at all.
fn parse_version(version: &str) -> Option<(u64, u64, u64)> {
    let mut parts = version.split('.').map(|part| part.parse::<u64>().ok());
    match (parts.next(), parts.next(), parts.next()) {
        (Some(Some(major)), Some(Some(minor)), Some(Some(patch))) => Some((major, minor, patch)),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// The snapshot
// ---------------------------------------------------------------------------

/// Whether the binary was found. A named argument rather than a bare `bool`,
/// because at four call sites `false` reads as either "not installed" or "not
/// checked" and the payload cannot tell them apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Installed {
    Yes,
    No,
}

/// Resolve the binary, ask it its version, and say what the UI should show.
///
/// Blocking: a `PATH` walk and a child process.
pub fn describe(settings: &ClaudeSettings, search: &Search) -> Provider {
    if !settings.enabled {
        return disabled(settings);
    }

    match resolve(&settings.binary_path, search).startable() {
        Ok((path, source)) => {
            // stderr, because "which binary is it actually running" is the first
            // question a developer asks when a turn misbehaves, and the resolved
            // path is deliberately not on the wire — `ServerProvider` has no
            // field for it, and inventing one the client cannot decode would
            // cost the whole configuration payload.
            eprintln!("lightcode: agent binary {}", path.display());
            let probed = probe(&path, PROBE_TIMEOUT);
            describe_probe(settings, &path, &source, probed)
        }
        // The UI's headline is "Not found", with this sentence underneath saying
        // what *was* found, which is the pair a developer can act on.
        Err(why) => snapshot(
            settings,
            None,
            Installed::No,
            ProviderState::Error,
            Some(why),
        ),
    }
}

/// What the snapshot says once the binary has answered — or not.
fn describe_probe(
    settings: &ClaudeSettings,
    path: &Path,
    source: &Source,
    probed: Probed,
) -> Provider {
    match probed {
        // A working provider still owes the developer a sentence in two cases,
        // and the fallback wins when both apply: being handed a different binary
        // than the one configured is the more surprising of the two, and the
        // version advice will still be there on the next probe.
        Probed::Version(version) => {
            let message = match source {
                Source::OnPath {
                    instead_of: Some(configured),
                } => Some(format!(
                    "The configured Claude Code binary path {} does not exist, so {} was used \
                     instead, found on PATH.",
                    configured.display(),
                    path.display(),
                )),
                _ => version_advice(&version),
            };
            snapshot(
                settings,
                Some(version),
                Installed::Yes,
                ProviderState::Ready,
                message,
            )
        }
        // Installed, and something is wrong with it. Every one of these keeps
        // `installed: true` — the file is there and it ran, or tried to — so the
        // UI says "Unavailable" rather than "Not found", which are different
        // problems with different fixes.
        Probed::Unreadable { output } => snapshot(
            settings,
            None,
            Installed::Yes,
            ProviderState::Warning,
            Some(format!(
                "{} ran but did not report a version this server could read, so only the models \
                 every version supports are offered. It answered: {}",
                path.display(),
                first_line(&output),
            )),
        ),
        Probed::Failed {
            code,
            version,
            output,
        } => snapshot(
            settings,
            version,
            Installed::Yes,
            ProviderState::Error,
            Some(format!(
                "{} --version exited with {}. It answered: {}",
                path.display(),
                match code {
                    Some(code) => format!("status {code}"),
                    None => "no status, so something stopped it".to_string(),
                },
                first_line(&output),
            )),
        ),
        Probed::Unstartable { error } => snapshot(
            settings,
            None,
            Installed::Yes,
            ProviderState::Error,
            Some(format!(
                "{} was found but could not be started: {error}",
                path.display(),
            )),
        ),
        Probed::TimedOut => snapshot(
            settings,
            None,
            Installed::Yes,
            ProviderState::Error,
            Some(format!(
                "{} did not answer --version within {} seconds.",
                path.display(),
                PROBE_TIMEOUT.as_secs(),
            )),
        ),
    }
}

/// The diagnostic for a binary that is nowhere.
///
/// This is the sentence the ticket exists to produce, so it names every place
/// that was looked and nothing that was not: the configured path if the setting
/// named one, the command that was looked up, and the directories it was looked
/// up in. Enough to fix the problem without opening a log file, which is the
/// criterion.
fn not_found(configured: Option<&Path>, name: &str, directories: &[PathBuf]) -> String {
    let mut message = match configured {
        Some(path) => format!(
            "The configured Claude Code binary path {} does not exist. ",
            path.display()
        ),
        None => String::new(),
    };

    if directories.is_empty() {
        message.push_str(&format!(
            "There is no PATH set for this server to search, so {name} could not be looked up \
             either."
        ));
        return message;
    }

    message.push_str(&format!(
        "{name} was then looked for in {} PATH {}: {}.",
        directories.len(),
        match directories.len() {
            1 => "directory",
            _ => "directories",
        },
        directories
            .iter()
            .map(|directory| directory.display().to_string())
            .collect::<Vec<String>>()
            .join("; "),
    ));
    message
}

/// What counts as startable here, for the message that has to explain why a file
/// that exists was refused. The two platforms disagree, and a sentence about
/// mode bits on Windows would send a developer looking for something that is not
/// there.
fn what_would_start() -> &'static str {
    if cfg!(windows) {
        "A program on Windows is a file whose extension is one of PATHEXT — usually .exe."
    } else {
        "A program is a file with an execute bit set."
    }
}

/// The first line of a binary's output, for a message that goes in a banner.
/// Whatever a broken CLI printed, one line of it is what a developer can read at
/// a glance, and the rest is on their terminal if they want it.
fn first_line(output: &str) -> String {
    let line = output.lines().next().unwrap_or("").trim();
    match line.is_empty() {
        true => "nothing at all.".to_string(),
        false => line.to_string(),
    }
}

/// The provider instance with the driver switched off in settings.
///
/// `installed: false` although nothing looked: the developer turned the driver
/// off, so the server did not go looking, and claiming an install it did not
/// verify would be a guess. The UI reads `enabled` first and shows "Disabled"
/// either way.
fn disabled(settings: &ClaudeSettings) -> Provider {
    snapshot(
        settings,
        None,
        Installed::No,
        ProviderState::Disabled,
        Some("The Claude Code provider is switched off in settings.".to_string()),
    )
}

/// One provider snapshot, however it was arrived at.
///
/// **Every** field the UI decodes comes from here — there is no second
/// constructor, including for the disabled case — so the shape cannot differ
/// between "ready" and each of the six ways of not being, which is what
/// [`tests::every_outcome_produces_the_same_fields`] holds it to. `enabled` is
/// read off the settings rather than passed, because a provider that disagreed
/// with the settings it was built from would be a payload nobody could act on.
fn snapshot(
    settings: &ClaudeSettings,
    version: Option<String>,
    installed: Installed,
    status: ProviderState,
    message: Option<String>,
) -> Provider {
    Provider {
        instance_id: INSTANCE_ID.to_string(),
        driver: INSTANCE_ID.to_string(),
        display_name: DISPLAY_NAME.to_string(),
        enabled: settings.enabled,
        installed: installed == Installed::Yes,
        models: models(version.as_deref(), &settings.custom_models),
        version,
        status,
        message,
        auth: ProviderAuth {
            status: AuthStatus::Unknown,
        },
        checked_at: now_iso(),
        slash_commands: Vec::new(),
        skills: Vec::new(),
    }
}

/// Resolve the agent binary and publish what was found.
///
/// Blocking — see [`describe`] — so a caller runs it somewhere that may block.
/// The store's own ordering guarantee is what makes this safe to call from
/// anywhere: a change is stored before it is announced, so a subscriber that
/// opened mid-probe is either told about it or already sees it in its snapshot.
pub fn refresh(config: &ConfigStore, search: &Search) {
    let settings = config.current().settings.providers.claude_agent.clone();
    let provider = describe(&settings, search);
    if let Some(message) = &provider.message {
        eprintln!("lightcode: provider claudeAgent: {message}");
    }
    config.apply(ConfigChange::Providers(vec![provider]));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stand-in for the agent: a file this platform agrees is a program, which
    /// answers `--version` however the test needs it answered.
    ///
    /// The suite has to run on a machine with no `claude` installed — spec story
    /// 61, and the last criterion of ticket 09 — so a test that needs a binary
    /// makes one. `tests/harness/agent.rs` holds the same thing for the socket
    /// tests, which are a separate crate and cannot see this one; the split of
    /// *what* is driven where is set out in `socket_provider.rs`'s header.
    struct Fake {
        directory: tempfile::TempDir,
    }

    impl Fake {
        fn saying(script: &str) -> Fake {
            let fake = Fake {
                directory: tempfile::tempdir().expect("a temporary directory"),
            };
            let path = fake.path();
            if cfg!(windows) {
                std::fs::write(&path, format!("@echo off\r\n{script}\r\n"))
                    .expect("writes the batch file");
            } else {
                std::fs::write(&path, format!("#!/bin/sh\n{script}\n"))
                    .expect("writes the shell script");
                #[cfg(not(windows))]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                        .expect("sets the mode");
                }
            }
            fake
        }

        /// A binary that reports `version` the way the real one does.
        fn reporting(version: &str) -> Fake {
            Fake::saying(&format!("echo {version} (Claude Code)"))
        }

        /// A binary that takes about a second to say anything. `ping` is the
        /// platform's idiom for sleeping in a batch file without a console:
        /// `timeout` needs one, and `powershell -c Start-Sleep` costs more to
        /// start than it sleeps for.
        fn dawdling() -> Fake {
            Fake::saying(match cfg!(windows) {
                true => "ping -n 2 127.0.0.1 >nul",
                false => "sleep 1",
            })
        }

        fn path(&self) -> PathBuf {
            self.directory.path().join(match cfg!(windows) {
                true => "claude.cmd",
                false => "claude",
            })
        }

        fn on_path(&self) -> Search {
            Search::over(&[self.directory.path()])
        }
    }

    /// The default settings, as `ServerConfig::detect` builds them.
    fn settings() -> ClaudeSettings {
        crate::config::ServerConfig::detect()
            .settings
            .providers
            .claude_agent
    }

    /// A search that will not find anything, over a directory that exists.
    fn empty_search() -> (tempfile::TempDir, Search) {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let search = Search::over(&[directory.path()]);
        (directory, search)
    }

    // -- resolution ---------------------------------------------------------
    //
    // The rules, driven directly, because a failure here should name the case
    // rather than a JSON pointer. What the UI *sees* is `socket_provider.rs`.

    /// The default configuration is a bare name, and a bare name is looked up.
    #[test]
    fn the_binary_is_found_on_path_without_any_configuration() {
        let fake = Fake::reporting("2.1.220");

        assert_eq!(settings().binary_path, DEFAULT_NAME, "the default setting");
        assert_eq!(
            resolve(DEFAULT_NAME, &fake.on_path()),
            Located::Binary {
                path: fake.path(),
                source: Source::OnPath { instead_of: None },
            }
        );
    }

    /// An empty setting means the same thing as the default one — see
    /// [`ClaudeSettings::binary_path`], which documents it that way.
    #[test]
    fn an_empty_setting_looks_up_the_agents_own_name() {
        let fake = Fake::reporting("2.1.220");

        assert_eq!(
            resolve("   ", &fake.on_path()),
            Located::Binary {
                path: fake.path(),
                source: Source::OnPath { instead_of: None },
            }
        );
    }

    /// The precedence the ticket asks for, at the level of which file was chosen.
    #[test]
    fn an_explicitly_configured_path_beats_one_on_path() {
        let configured = Fake::reporting("1.0.0");
        let on_path = Fake::reporting("2.0.0");

        assert_eq!(
            resolve(&configured.path().to_string_lossy(), &on_path.on_path()),
            Located::Binary {
                path: configured.path(),
                source: Source::Configured,
            }
        );
    }

    /// A configured path that is not there falls back, and remembers what it
    /// replaced — which is what lets the snapshot say so.
    #[test]
    fn a_configured_path_that_is_gone_falls_back_to_path() {
        let fake = Fake::reporting("2.1.220");
        let stale = fake.directory.path().join("moved").join("claude.cmd");

        assert_eq!(
            resolve(&stale.to_string_lossy(), &fake.on_path()),
            Located::Binary {
                path: fake.path(),
                source: Source::OnPath {
                    instead_of: Some(stale),
                },
            }
        );
    }

    /// The name looked for on `PATH` after a configured path missed is that
    /// path's own file name, not the default — a developer who renamed their
    /// binary gets their binary.
    #[test]
    fn the_fallback_looks_for_the_name_the_configured_path_used() {
        let (directory, search) = empty_search();
        let configured = directory.path().join("gone").join("my-claude.exe");

        match resolve(&configured.to_string_lossy(), &search) {
            Located::Nothing { name, .. } => assert_eq!(name, "my-claude.exe"),
            other => panic!("expected nothing to be found, got {other:?}"),
        }
    }

    /// A file that is there and is not a program stops resolution rather than
    /// falling through — asserted against a search that *does* hold a working
    /// binary, so it fails if the fallback ever becomes symmetric.
    #[test]
    fn a_configured_path_that_cannot_be_started_does_not_fall_back() {
        let fake = Fake::reporting("2.1.220");
        let notes = fake.directory.path().join("notes.txt");
        std::fs::write(&notes, "not a program").expect("writes the file");

        assert_eq!(
            resolve(&notes.to_string_lossy(), &fake.on_path()),
            Located::NotExecutable {
                configured: notes.clone()
            },
            "a startable binary was on PATH and must not have been used"
        );

        let directory = fake.directory.path().join("a-directory");
        std::fs::create_dir(&directory).expect("creates the directory");
        assert_eq!(
            resolve(&directory.to_string_lossy(), &fake.on_path()),
            Located::NotExecutable {
                configured: directory
            }
        );
    }

    /// Every part of the sentence the ticket exists to produce, driven at the
    /// function that composes it. The socket test asserts it reaches the UI; this
    /// asserts what it says, including a case a real machine will not reach.
    #[test]
    fn the_missing_binary_diagnostic_names_everywhere_that_was_looked() {
        let configured = PathBuf::from("C:/old/claude.exe");
        let directories = vec![PathBuf::from("C:/bin"), PathBuf::from("C:/tools")];

        let searched = not_found(Some(&configured), "claude.exe", &directories);
        assert!(searched.contains("C:/old/claude.exe"), "{searched}");
        assert!(searched.contains("2 PATH directories"), "{searched}");
        assert!(searched.contains("C:/bin"), "{searched}");
        assert!(searched.contains("C:/tools"), "{searched}");

        // One directory is not "1 directories".
        let single = not_found(None, "claude", &directories[..1]);
        assert!(single.contains("1 PATH directory"), "{single}");
        assert!(
            !single.contains("configured"),
            "there was no configured path to name: {single}"
        );

        // And a machine with no PATH is told so, rather than shown an empty list
        // after "was looked for in 0 directories".
        let nowhere = not_found(None, "claude", &[]);
        assert!(nowhere.contains("no PATH set"), "{nowhere}");
        assert!(nowhere.contains("claude"), "{nowhere}");
    }

    // -- the probe ----------------------------------------------------------

    /// The three ways a binary that ran can answer, at the boundary where they
    /// are told apart. What each becomes in the payload is a socket test.
    #[test]
    fn a_binary_that_runs_is_read_as_a_version_a_silence_or_a_failure() {
        let reporting = Fake::reporting("2.1.220");
        assert_eq!(
            probe(&reporting.path(), PROBE_TIMEOUT),
            Probed::Version("2.1.220".to_string())
        );

        let quiet = Fake::saying("echo ready when you are");
        match probe(&quiet.path(), PROBE_TIMEOUT) {
            Probed::Unreadable { output } => {
                assert!(output.contains("ready when you are"), "{output}")
            }
            other => panic!("expected unreadable output, got {other:?}"),
        }

        let broken = Fake::saying(match cfg!(windows) {
            true => "exit /b 3",
            false => "exit 3",
        });
        match probe(&broken.path(), PROBE_TIMEOUT) {
            Probed::Failed { code, .. } => assert_eq!(code, Some(3)),
            other => panic!("expected a failure, got {other:?}"),
        }
    }

    /// The parse is deliberately loose, so the cases worth pinning are the ones
    /// at its edges: what the real binary prints, the two other conventions a CLI
    /// might use, and the near-misses that must *not* be read as a version — a
    /// two-part number, and a number that is simply a number.
    #[test]
    fn a_version_is_read_out_of_whatever_the_binary_prints_around_it() {
        let read = |output: &str| version_in(output).unwrap_or_default();

        // What the real binary answers, and the two other shapes a CLI uses.
        assert_eq!(read("2.1.220 (Claude Code)"), "2.1.220");
        assert_eq!(read("claude 2.1.220"), "2.1.220");
        assert_eq!(read("v2.1.220"), "2.1.220");
        // A four-part version reports its first three, as upstream's regex does.
        assert_eq!(read("1.2.3.4"), "1.2.3");
        // And a number has to be a version to count as one.
        assert_eq!(read("no version here"), "");
        assert_eq!(read("2.1"), "");
        assert_eq!(read("built from 12 commits"), "");
        assert_eq!(read(""), "");
    }

    /// A binary that never answers is given up on rather than holding the thread
    /// that asked. Driven with a patience of milliseconds — which is the reason
    /// `probe` takes one — against a binary that will not answer for a second.
    #[test]
    fn a_binary_that_does_not_answer_is_given_up_on() {
        let dawdling = Fake::dawdling();
        let started = Instant::now();

        assert_eq!(
            probe(&dawdling.path(), Duration::from_millis(50)),
            Probed::TimedOut
        );
        assert!(
            started.elapsed() < Duration::from_millis(900),
            "gave up after {:?}, so it waited for the child rather than for the deadline",
            started.elapsed()
        );
    }

    // -- models -------------------------------------------------------------

    /// The version is not decoration: it decides which slugs the UI may offer,
    /// so a CLI that has never heard of a model is never asked for one.
    #[test]
    fn the_models_offered_are_gated_on_the_version_that_answered() {
        let slugs = |version: Option<&str>| -> Vec<String> {
            models(version, &[]).into_iter().map(|model| model.slug).collect()
        };

        let old = slugs(Some("2.1.100"));
        assert!(!old.contains(&"claude-opus-5".to_string()), "{old:?}");
        assert!(!old.contains(&"claude-opus-4-7".to_string()), "{old:?}");
        assert!(old.contains(&"claude-sonnet-5".to_string()), "{old:?}");

        // The exact boundary, both sides of it.
        assert!(!slugs(Some("2.1.218")).contains(&"claude-opus-5".to_string()));
        assert!(slugs(Some("2.1.219")).contains(&"claude-opus-5".to_string()));

        let current = slugs(Some("2.1.220"));
        assert_eq!(
            current.len(),
            BUILT_IN_MODELS.len(),
            "a current CLI is offered the whole table: {current:?}"
        );
        // The first non-custom model is the UI's default, so the table's order is
        // behaviour rather than presentation.
        assert_eq!(current[0], "claude-fable-5");
    }

    /// An unknown version offers the models every version knows, rather than all
    /// of them or none. Both other answers are worse: one populates the picker
    /// with slugs that may be rejected, the other leaves the UI to fall back to
    /// its own hardcoded default.
    #[test]
    fn an_unknown_version_offers_only_the_models_that_need_no_version() {
        let offered = models(None, &[]);

        assert!(!offered.is_empty());
        assert!(offered.iter().all(|model| BUILT_IN_MODELS
            .iter()
            .any(|built_in| built_in.slug == model.slug && built_in.since.is_none())));
    }

    /// The gate is silent, so something has to say why the list is short. One
    /// sentence naming the newest model out of reach clears the whole table.
    #[test]
    fn the_version_advice_names_the_newest_model_out_of_reach() {
        let advice = version_advice("2.1.100").expect("a sentence");
        assert!(advice.contains("2.1.100"), "{advice}");
        assert!(advice.contains("Claude Opus 5"), "{advice}");
        assert!(advice.contains("v2.1.219"), "{advice}");

        // Between two gates, the one still out of reach is the one named.
        let nearly = version_advice("2.1.200").expect("a sentence");
        assert!(nearly.contains("Claude Opus 5"), "{nearly}");

        // And a CLI that can run everything is told nothing.
        assert_eq!(version_advice("2.1.219"), None);
        assert_eq!(version_advice("3.0.0"), None);
    }

    // -- the snapshot -------------------------------------------------------

    /// A refresh publishes into the store, which is how the UI hears about it
    /// without asking again. The store starts with no provider at all — that is
    /// what "checking" looks like on the wire — and ends with one.
    #[test]
    fn a_refresh_puts_the_resolved_provider_into_the_store() {
        let fake = Fake::reporting("2.1.220");
        let store = ConfigStore::new(crate::config::ServerConfig::detect());

        assert!(
            store.current().providers.is_empty(),
            "nothing has looked yet"
        );

        refresh(&store, &fake.on_path());

        let providers = &store.current().providers;
        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].version.as_deref(), Some("2.1.220"));
        assert_eq!(providers[0].status, ProviderState::Ready);
    }

    /// Nothing is spawned and nothing is searched for a driver the developer
    /// turned off.
    #[test]
    fn a_disabled_provider_is_not_looked_for() {
        let ready = Fake::reporting("2.1.220");
        let settings = ClaudeSettings {
            enabled: false,
            ..settings()
        };

        let provider = describe(&settings, &ready.on_path());

        assert_eq!(provider.status, ProviderState::Disabled);
        assert!(!provider.enabled);
        assert!(!provider.installed);
        assert_eq!(
            provider.version, None,
            "a version here would mean it was probed anyway"
        );
    }

    // -- the clock ----------------------------------------------------------
    //
    // The rendering itself moved to `crate::clock` with ticket 10, which stamps
    // every message and activity rather than one probe. What stays here is the
    // one assertion that is about *this* payload: two clocks meet in it.

    /// The payload carries two clocks — [`crate::clock`] and the registry's, which is
    /// SQLite's — and a client parses both with the same `new Date`. If they ever
    /// render differently, one of them stops being a date.
    #[test]
    fn the_clock_renders_the_way_the_registrys_does() {
        let database = crate::store::Database::in_memory().expect("an in-memory database");
        let from_sqlite = database.registry().expect("a registry").updated_at;

        let layout = |stamp: &str| -> String {
            stamp
                .chars()
                .map(|character| match character.is_ascii_digit() {
                    true => '#',
                    false => character,
                })
                .collect()
        };

        assert_eq!(layout(&now_iso()), layout(&from_sqlite));
    }
}
