//! The agent this server drives: where its binary is, whether it will run, and
//! what the UI is told about it.
//!
//! **Nothing here starts an agent that takes a turn.** What this establishes is
//! that the driver *can* be started — the binary is located, it answers
//! the driver-specific probe, and the answer is published as a provider instance
//! the UI can select. [`crate::turn`] starts Claude for a real session; Codex
//! turns begin with the next ticket.
//!
//! It does now start children that take *no* turn: Claude is asked what commands
//! it offers, while Codex is asked for its account, models and workspace skills.
//! Each probe is killed after its answer. [`crate::catalogue`] owns Claude's
//! command and skill catalogue; [`crate::codex`] owns the Codex probe.
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
//!   batch file. laplus drives the CLI itself and hands
//!   `std::process::Command` a resolved absolute path, and std runs a `.cmd`
//!   through `cmd.exe` on our behalf; on the platform v1 ships the executable is
//!   a native `claude.exe` in any case. The ticket calls it dead weight, and it
//!   is.
//! - **Claude's `homePath` and `launchArgs`.** Both describe the environment the
//!   *agent* runs in. Claude's `--version` probe needs neither. Codex's probe is
//!   an app-server and therefore does honour both of its corresponding settings.
//! - **`versionAdvisory`.** Absent, and absent is not a lesser form of what
//!   upstream sends with update checks off. It would emit the field with
//!   `status: "unknown"` and four nulls — the `grok` entry in
//!   `fixtures/socket-wire/02-request-response.ndjson` shows exactly that — and
//!   `getProviderVersionAdvisoryPresentation` returns `null` for an `unknown`
//!   advisory and for a missing one alike. So the two render identically, and
//!   nothing here has a latest version or an update command to put in one.
//! - **Claude authentication.** No Claude credential is read, so its
//!   `auth.status` is `unknown`, which is the contract's own literal for exactly
//!   that. Codex answers `account/read`, so its authenticated and unauthenticated
//!   states are reported explicitly.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::catalogue;
use crate::clock::now_iso;
use crate::config::{
    AuthStatus, ClaudeSettings, CodexSettings, Provider, ProviderAuth, ProviderModel, ProviderState,
};
use crate::config::Settings;
use crate::config_store::{ConfigStore, ProviderProbe};
use crate::process::Search;

/// The routing key for the Claude provider instance.
pub const CLAUDE_INSTANCE_ID: &str = "claudeAgent";

/// The slug selecting the Claude driver implementation. It currently has the
/// same spelling as its instance id, but it is a separate concept and registry
/// field: another instance of this driver would keep this slug and get its own
/// routing key.
pub const CLAUDE_DRIVER: &str = "claudeAgent";
pub const CODEX_INSTANCE_ID: &str = "codex";
pub const CODEX_DRIVER: &str = "codex";
pub const REFRESH: &str = "server.refreshProviders";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriverKind {
    Claude,
    Codex,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Registration {
    pub instance_id: &'static str,
    pub driver: &'static str,
    pub kind: DriverKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderIdentity {
    pub instance_id: String,
    pub driver: String,
}

/// Opaque continuation data, bound to the provider instance that minted it.
/// Only that instance's driver may interpret `value`.
#[derive(Debug, Clone, PartialEq)]
pub struct ResumeCursor {
    pub provider: ProviderIdentity,
    pub value: serde_json::Value,
}

impl Registration {
    pub fn identity(self) -> ProviderIdentity {
        ProviderIdentity {
            instance_id: self.instance_id.to_string(),
            driver: self.driver.to_string(),
        }
    }
}

/// The drivers this build knows how to route. Settings may know about a driver
/// before it joins this registry; a registered driver can publish a provider and
/// be selected by a conversation even when its turn implementation lands later.
pub const REGISTRY: &[Registration] = &[
    Registration {
        instance_id: CLAUDE_INSTANCE_ID,
        driver: CLAUDE_DRIVER,
        kind: DriverKind::Claude,
    },
    Registration {
        instance_id: CODEX_INSTANCE_ID,
        driver: CODEX_DRIVER,
        kind: DriverKind::Codex,
    },
];

pub fn registration(instance_id: &str) -> Option<Registration> {
    REGISTRY
        .iter()
        .copied()
        .find(|registered| registered.instance_id == instance_id)
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClaudeInstance {
    pub identity: ProviderIdentity,
    pub display_name: String,
    pub settings: ClaudeSettings,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CodexInstance {
    pub identity: ProviderIdentity,
    pub display_name: String,
    pub settings: CodexSettings,
}

/// Resolve the settings and durable routing identity for a Claude instance.
/// The instance id doubles as its continuation namespace, so two configurations
/// of the same driver cannot accidentally resume each other's conversations.
pub fn claude_instance(settings: &Settings, instance_id: &str) -> Option<ClaudeInstance> {
    let explicit = settings.provider_instances.get(instance_id);
    if instance_id == CLAUDE_INSTANCE_ID && explicit.is_none() {
        return Some(ClaudeInstance {
            identity: claude_registration().identity(),
            display_name: DISPLAY_NAME.to_string(),
            settings: settings.providers.claude_agent.clone(),
        });
    }
    let envelope = explicit?.as_object()?;
    if envelope.get("driver")?.as_str()? != CLAUDE_DRIVER {
        return None;
    }
    let config = envelope.get("config")?.as_object()?;
    Some(ClaudeInstance {
        identity: ProviderIdentity {
            instance_id: instance_id.to_string(),
            driver: CLAUDE_DRIVER.to_string(),
        },
        display_name: envelope.get("displayName")?.as_str()?.to_string(),
        settings: ClaudeSettings {
            enabled: envelope
                .get("enabled")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(true),
            binary_path: config.get("binaryPath")?.as_str()?.to_string(),
            home_path: config.get("homePath")?.as_str()?.to_string(),
            launch_args: config.get("launchArgs")?.as_str()?.to_string(),
            custom_models: config
                .get("customModels")?
                .as_array()?
                .iter()
                .map(|model| model.as_str().map(str::to_string))
                .collect::<Option<Vec<_>>>()?,
        },
    })
}

pub fn codex_instance(settings: &Settings, instance_id: &str) -> Option<CodexInstance> {
    let explicit = settings.provider_instances.get(instance_id);
    if instance_id == CODEX_INSTANCE_ID && explicit.is_none() {
        return Some(CodexInstance {
            identity: codex_registration().identity(),
            display_name: "Codex".to_string(),
            settings: settings.providers.codex.clone(),
        });
    }
    let envelope = explicit?.as_object()?;
    if envelope.get("driver")?.as_str()? != CODEX_DRIVER {
        return None;
    }
    let config = envelope.get("config")?.as_object()?;
    Some(CodexInstance {
        identity: ProviderIdentity {
            instance_id: instance_id.to_string(),
            driver: CODEX_DRIVER.to_string(),
        },
        display_name: envelope.get("displayName")?.as_str()?.to_string(),
        settings: CodexSettings {
            enabled: envelope.get("enabled").and_then(serde_json::Value::as_bool).unwrap_or(true),
            binary_path: config.get("binaryPath")?.as_str()?.to_string(),
            home_path: config.get("homePath")?.as_str()?.to_string(),
            launch_args: config.get("launchArgs")?.as_str()?.to_string(),
            custom_models: config.get("customModels")?.as_array()?.iter()
                .map(|model| model.as_str().map(str::to_string)).collect::<Option<Vec<_>>>()?,
        },
    })
}

pub fn identity(settings: &Settings, instance_id: &str) -> Option<ProviderIdentity> {
    claude_instance(settings, instance_id).map(|instance| instance.identity)
        .or_else(|| codex_instance(settings, instance_id).map(|instance| instance.identity))
}

pub fn driver_kind(settings: &Settings, instance_id: &str) -> Option<DriverKind> {
    claude_instance(settings, instance_id).map(|_| DriverKind::Claude)
        .or_else(|| codex_instance(settings, instance_id).map(|_| DriverKind::Codex))
}

fn claude_registration() -> Registration {
    registration(CLAUDE_INSTANCE_ID).expect("the Claude driver is registered")
}

fn codex_registration() -> Registration {
    registration(CODEX_INSTANCE_ID).expect("the Codex driver is registered")
}

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
        self.startable_for("Claude Code")
    }

    pub(crate) fn startable_codex(self) -> Result<(PathBuf, Source), String> {
        self.startable_for("Codex")
    }

    fn startable_for(self, product: &str) -> Result<(PathBuf, Source), String> {
        match self {
            Located::Binary { path, source } => Ok((path, source)),
            // `installed: false` although the file exists, because what
            // `installed` claims is that there is an agent here — and a
            // directory or a text file is not one.
            Located::NotExecutable { configured } => Err(format!(
                "The configured {product} binary path {} exists but is not a program this \
                 machine can start. {} PATH was not searched, because a path was configured \
                 and something is there.",
                configured.display(),
                what_would_start(),
            )),
            Located::Nothing {
                configured,
                name,
                directories,
            } => Err(not_found_for(product, configured.as_deref(), &name, &directories)),
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
    resolve_named(configured, DEFAULT_NAME, search)
}

pub(crate) fn resolve_codex(configured: &str, search: &Search) -> Located {
    resolve_named(configured, "codex", search)
}

fn resolve_named(configured: &str, default_name: &str, search: &Search) -> Located {
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
            true => default_name.to_string(),
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
            is_default: None,
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
            is_default: None,
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
pub fn describe(settings: &ClaudeSettings, search: &Search, roots: &[PathBuf]) -> Provider {
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
            eprintln!("laplus: agent binary {}", path.display());
            let probed = probe(&path, PROBE_TIMEOUT);
            describe_probe(settings, &path, &source, probed, roots)
        }
        // The UI's headline is "Not found", with this sentence underneath saying
        // what *was* found, which is the pair a developer can act on.
        //
        // The skills are read anyway. They are the developer's own files, they
        // are there whether or not an agent is, and a machine that is a `PATH`
        // entry away from working should not also come back with an empty picker
        // once it does.
        Err(why) => snapshot(
            settings,
            None,
            Installed::No,
            ProviderState::Error,
            Some(why),
            catalogue::read(settings, None, roots),
        ),
    }
}

fn describe_configured(instance: &ClaudeInstance, search: &Search, roots: &[PathBuf]) -> Provider {
    let mut provider = describe(&instance.settings, search, roots);
    provider.instance_id = instance.identity.instance_id.clone();
    provider.driver = instance.identity.driver.clone();
    provider.display_name = instance.display_name.clone();
    provider
}

/// What the snapshot says once the binary has answered — or not.
fn describe_probe(
    settings: &ClaudeSettings,
    path: &Path,
    source: &Source,
    probed: Probed,
    roots: &[PathBuf],
) -> Provider {
    // Only a binary that answered is asked to say hello. Every other arm here is
    // one that could not run, would not run, or did not finish, and starting a
    // session with it would spend the catalogue's whole patience discovering what
    // `--version` has just established. Their skills are read either way, being
    // the developer's own files rather than the agent's.
    let catalogue = match probed {
        Probed::Version(_) => catalogue::read(settings, Some(path), roots),
        _ => catalogue::read(settings, None, roots),
    };

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
                catalogue,
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
            catalogue,
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
            catalogue,
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
            catalogue,
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
            catalogue,
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
#[cfg(test)]
fn not_found(configured: Option<&Path>, name: &str, directories: &[PathBuf]) -> String {
    not_found_for("Claude Code", configured, name, directories)
}

fn not_found_for(
    product: &str,
    configured: Option<&Path>,
    name: &str,
    directories: &[PathBuf],
) -> String {
    let mut message = match configured {
        Some(path) => format!(
            "The configured {product} binary path {} does not exist. ",
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
        // Nothing looked, here too: a driver the developer switched off is one
        // this server does not go to the filesystem about, and a picker full of
        // skills for a provider that cannot be selected would be furniture.
        catalogue::Catalogue::default(),
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
    catalogue: catalogue::Catalogue,
) -> Provider {
    let registered = claude_registration();
    Provider {
        instance_id: registered.instance_id.to_string(),
        driver: registered.driver.to_string(),
        display_name: DISPLAY_NAME.to_string(),
        enabled: settings.enabled,
        installed: installed == Installed::Yes,
        models: models(version.as_deref(), &settings.custom_models),
        version,
        status,
        message,
        auth: unknown_auth(),
        checked_at: now_iso(),
        slash_commands: catalogue.slash_commands,
        skills: catalogue.skills,
    }
}

/// Resolve the agent binary and publish what was found.
///
/// Blocking — see [`describe`] — so a caller runs it somewhere that may block.
/// The store's own ordering guarantee is what makes this safe to call from
/// anywhere: a change is stored before it is announced, so a subscriber that
/// opened mid-probe is either told about it or already sees it in its snapshot.
/// Re-read the skills of every provider already published.
///
/// What a project being registered or removed changes. The `$` menu is built
/// from a snapshot taken before it, so without this a developer adds a project
/// and its skills appear on the next restart — which reads as the feature not
/// working rather than as a stale cache.
///
/// Claude's skills are files and cost only a scan. Codex exposes them through
/// `skills/list`, so its existing provider is re-probed with the new roots; one
/// app-server still answers version, account, models and skills together.
pub(crate) struct ProbeReservations {
    claude: Option<ProviderProbe>,
    codex: Option<ProviderProbe>,
}

pub(crate) fn reserve_probes(config: &ConfigStore) -> ProbeReservations {
    ProbeReservations {
        claude: Some(config.begin_provider_probe(CLAUDE_INSTANCE_ID)),
        codex: Some(config.begin_provider_probe(CODEX_INSTANCE_ID)),
    }
}

pub(crate) fn reserve_skill_rescan(config: &ConfigStore) -> ProbeReservations {
    let current = config.current();
    let present = |instance_id| {
        current
            .providers
            .iter()
            .any(|provider| provider.instance_id == instance_id)
    };
    ProbeReservations {
        claude: (claude_instance(&current.settings, CLAUDE_INSTANCE_ID)
            .is_some_and(|instance| instance.settings.enabled)
            && present(CLAUDE_INSTANCE_ID))
        .then(|| config.begin_provider_probe(CLAUDE_INSTANCE_ID)),
        codex: codex_instance(&current.settings, CODEX_INSTANCE_ID)
            .is_some_and(|instance| instance.settings.enabled)
            .then(|| config.begin_provider_probe(CODEX_INSTANCE_ID)),
    }
}

pub fn rescan_skills(config: &ConfigStore, roots: &[PathBuf]) {
    let probes = reserve_skill_rescan(config);
    rescan_skills_reserved(config, roots, probes);
}

pub(crate) fn rescan_skills_reserved(
    config: &ConfigStore,
    roots: &[PathBuf],
    probes: ProbeReservations,
) {
    let current = config.current();
    if let Some(probe) = probes.claude {
        let instance = claude_instance(&current.settings, CLAUDE_INSTANCE_ID)
            .expect("the default Claude instance");
        if let Some(mut provider) = current
            .providers
            .iter()
            .find(|provider| provider.instance_id == CLAUDE_INSTANCE_ID)
            .cloned()
        {
            let skills = catalogue::skills(&instance.settings, roots);
            provider.skills = skills;
            publish_one(
                config,
                probe,
                expected_claude(&current.settings, instance.settings),
                provider,
            );
        }
    }

    if let Some(probe) = probes.codex {
        let instance = codex_instance(&current.settings, CODEX_INSTANCE_ID)
            .expect("the default Codex instance");
        let lifetime = config.provider_process_lifetime();
        let provider = describe_codex(
            &instance,
            &Search::from_environment(),
            roots,
            &lifetime,
        );
        publish_one(
            config,
            probe,
            expected_codex(&current.settings, instance.settings),
            provider,
        );
    }
}

fn describe_codex(
    instance: &CodexInstance,
    search: &Search,
    roots: &[PathBuf],
    lifetime: &crate::config_store::ProviderProcessLifetime,
) -> Provider {
    let settings = &instance.settings;
    if !settings.enabled {
        return codex_snapshot(
            instance,
            settings,
            None,
            Installed::No,
            ProviderState::Disabled,
            Some("The Codex provider is switched off in settings.".to_string()),
            unknown_auth(),
            crate::codex_protocol::custom_models(&settings.custom_models),
            Vec::new(),
        );
    }

    let (path, source) = match resolve_codex(&settings.binary_path, search)
        .startable_for("Codex CLI")
    {
        Ok(found) => found,
        Err(why) => {
            return codex_snapshot(
                instance,
                settings,
                None,
                Installed::No,
                ProviderState::Error,
                Some(why),
                unknown_auth(),
                crate::codex_protocol::custom_models(&settings.custom_models),
                Vec::new(),
            )
        }
    };

    eprintln!("laplus: codex binary {}", path.display());
    match crate::codex::probe(&path, settings, roots, lifetime) {
        Ok(probed) => {
            let (status, auth_message) = match probed.auth.status {
                AuthStatus::Unauthenticated => (
                    ProviderState::Error,
                    Some("Codex CLI is not authenticated. Run `codex login` and try again.".to_string()),
                ),
                AuthStatus::Authenticated => (ProviderState::Ready, None),
                AuthStatus::Unknown => (
                    ProviderState::Warning,
                    Some("Codex account status could not be verified.".to_string()),
                ),
            };
            let fallback_message = match source {
                Source::OnPath {
                    instead_of: Some(configured),
                } => Some(format!(
                    "The configured Codex CLI binary path {} does not exist, so {} was used \
                     instead, found on PATH.",
                    configured.display(),
                    path.display(),
                )),
                _ => None,
            };
            let message = match (fallback_message, auth_message) {
                (Some(fallback), Some(auth)) => Some(format!("{fallback} {auth}")),
                (Some(message), None) | (None, Some(message)) => Some(message),
                (None, None) => None,
            };
            codex_snapshot(
                instance,
                settings,
                probed.version,
                Installed::Yes,
                status,
                message,
                probed.auth,
                probed.models,
                probed.skills,
            )
        }
        Err(why) => codex_snapshot(
            instance,
            settings,
            None,
            Installed::Yes,
            ProviderState::Error,
            Some(format!("Codex app-server provider probe failed: {why}.")),
            unknown_auth(),
            crate::codex_protocol::custom_models(&settings.custom_models),
            Vec::new(),
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn codex_snapshot(
    instance: &CodexInstance,
    settings: &CodexSettings,
    version: Option<String>,
    installed: Installed,
    status: ProviderState,
    message: Option<String>,
    auth: ProviderAuth,
    models: Vec<ProviderModel>,
    skills: Vec<serde_json::Value>,
) -> Provider {
    Provider {
        instance_id: instance.identity.instance_id.clone(),
        driver: instance.identity.driver.clone(),
        display_name: instance.display_name.clone(),
        enabled: settings.enabled,
        installed: installed == Installed::Yes,
        version,
        status,
        message,
        auth,
        checked_at: now_iso(),
        models,
        slash_commands: Vec::new(),
        skills,
    }
}

fn unknown_auth() -> ProviderAuth {
    ProviderAuth {
        status: AuthStatus::Unknown,
        r#type: None,
        label: None,
        email: None,
    }
}

pub fn refresh(config: &ConfigStore, search: &Search, roots: &[PathBuf]) {
    let probes = reserve_probes(config);
    refresh_reserved(config, search, roots, probes);
}

pub(crate) fn refresh_reserved(
    config: &ConfigStore,
    search: &Search,
    roots: &[PathBuf],
    probes: ProbeReservations,
) {
    let current_settings = config.current().settings.clone();
    let claude_instance = claude_instance(&current_settings, CLAUDE_INSTANCE_ID)
        .expect("the default Claude instance");
    let claude = describe_configured(&claude_instance, search, roots);
    if let Some(message) = &claude.message {
        eprintln!("laplus: provider claudeAgent: {message}");
    }
    publish_one(
        config,
        probes.claude.expect("a full refresh reserves Claude"),
        expected_claude(&current_settings, claude_instance.settings),
        claude,
    );

    let lifetime = config.provider_process_lifetime();
    let codex_instance = codex_instance(&current_settings, CODEX_INSTANCE_ID)
        .expect("the default Codex instance");
    let codex = describe_codex(
        &codex_instance,
        search,
        roots,
        &lifetime,
    );
    if let Some(message) = &codex.message {
        eprintln!("laplus: provider codex: {message}");
    }
    publish_one(
        config,
        probes.codex.expect("a full refresh reserves Codex"),
        expected_codex(&current_settings, codex_instance.settings),
        codex,
    );

    for instance_id in current_settings.provider_instances.keys()
        .filter(|id| id.as_str() != CLAUDE_INSTANCE_ID && id.as_str() != CODEX_INSTANCE_ID)
    {
        refresh_configured(config, instance_id, search, roots);
    }
}

pub fn refresh_claude(config: &ConfigStore, search: &Search, roots: &[PathBuf]) {
    let probe = config.begin_provider_probe(CLAUDE_INSTANCE_ID);
    let current = config.current();
    let instance = claude_instance(&current.settings, CLAUDE_INSTANCE_ID)
        .expect("the default Claude instance");
    let provider = describe_configured(&instance, search, roots);
    publish_one(config, probe, expected_claude(&current.settings, instance.settings), provider);
}

pub fn refresh_codex(config: &ConfigStore, search: &Search, roots: &[PathBuf]) {
    let probe = config.begin_provider_probe(CODEX_INSTANCE_ID);
    let current = config.current();
    let instance = codex_instance(&current.settings, CODEX_INSTANCE_ID)
        .expect("the default Codex instance");
    let lifetime = config.provider_process_lifetime();
    let provider = describe_codex(&instance, search, roots, &lifetime);
    publish_one(config, probe, expected_codex(&current.settings, instance.settings), provider);
}

pub fn refresh_configured(
    config: &ConfigStore,
    instance_id: &str,
    search: &Search,
    roots: &[PathBuf],
) {
    let probe = config.begin_provider_probe(instance_id);
    let current = config.current();
    let expected = current
        .settings
        .provider_instances
        .get(instance_id)
        .cloned();
    let provider = if let Some(instance) = claude_instance(&current.settings, instance_id) {
        describe_configured(&instance, search, roots)
    } else if let Some(instance) = codex_instance(&current.settings, instance_id) {
        let lifetime = config.provider_process_lifetime();
        describe_codex(&instance, search, roots, &lifetime)
    } else {
        return;
    };
    publish_one(
        config,
        probe,
        ExpectedSettings::Configured(instance_id.to_string(), expected),
        provider,
    );
}

pub fn refresh_call(
    payload: &serde_json::Value,
    config: &ConfigStore,
    roots: &[PathBuf],
) -> Result<serde_json::Value, serde_json::Value> {
    let instance_id = payload.get("instanceId").and_then(serde_json::Value::as_str);
    if payload.get("instanceId").is_some() && instance_id.is_none() {
        return Err(crate::rpc::declared(
            "EnvironmentAuthorizationError",
            "'instanceId' has to be a provider instance id.",
        ));
    }
    let search = Search::from_environment();
    match instance_id {
        None => refresh(config, &search, roots),
        Some(CLAUDE_INSTANCE_ID) => refresh_claude(config, &search, roots),
        Some(CODEX_INSTANCE_ID) => refresh_codex(config, &search, roots),
        Some(instance_id) if claude_instance(&config.current().settings, instance_id).is_some()
            || codex_instance(&config.current().settings, instance_id).is_some() => {
            refresh_configured(config, instance_id, &search, roots)
        }
        Some(instance_id) => {
            return Err(crate::rpc::declared(
                "EnvironmentAuthorizationError",
                format!("Provider instance '{instance_id}' is not configured."),
            ))
        }
    }
    Ok(serde_json::json!({"providers": config.current().providers}))
}

enum ExpectedSettings {
    Claude(ClaudeSettings),
    Codex(CodexSettings),
    Configured(String, Option<serde_json::Value>),
}

fn expected_claude(settings: &Settings, legacy: ClaudeSettings) -> ExpectedSettings {
    match settings.provider_instances.get(CLAUDE_INSTANCE_ID).cloned() {
        Some(value) => ExpectedSettings::Configured(CLAUDE_INSTANCE_ID.to_string(), Some(value)),
        None => ExpectedSettings::Claude(legacy),
    }
}

fn expected_codex(settings: &Settings, legacy: CodexSettings) -> ExpectedSettings {
    match settings.provider_instances.get(CODEX_INSTANCE_ID).cloned() {
        Some(value) => ExpectedSettings::Configured(CODEX_INSTANCE_ID.to_string(), Some(value)),
        None => ExpectedSettings::Codex(legacy),
    }
}

fn publish_one(
    config: &ConfigStore,
    probe: ProviderProbe,
    expected: ExpectedSettings,
    provider: Provider,
) {
    config.apply_providers_if_current(
        probe,
        move |current| match &expected {
            ExpectedSettings::Claude(expected) => {
                current.settings.providers.claude_agent == *expected
            }
            ExpectedSettings::Codex(expected) => current.settings.providers.codex == *expected,
            ExpectedSettings::Configured(instance_id, expected) => {
                current.settings.provider_instances.get(instance_id) == expected.as_ref()
            }
        },
        move |current| {
            let mut providers = current.to_vec();
            match providers
                .iter()
                .position(|held| held.instance_id == provider.instance_id)
            {
                Some(index) => providers[index] = provider,
                None => providers.push(provider),
            }
            providers.sort_by_key(|provider| {
                REGISTRY
                    .iter()
                    .position(|registered| registered.instance_id == provider.instance_id)
                    .unwrap_or(usize::MAX)
            });
            providers
        },
    );
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
        ///
        /// **The parentheses have to be quoted for `sh` and must not be for
        /// `cmd`.** `(Claude Code)` is part of what the real CLI prints, so it
        /// cannot just be dropped — and left bare in a `#!/bin/sh` script it is
        /// not output at all but a subshell, which dash refuses outright:
        /// `Syntax error: "(" unexpected`, exit status 2. Every test using this
        /// fake therefore saw a *failing* binary rather than a reporting one.
        /// `cmd` has no such rule and would print the quotes as text.
        fn reporting(version: &str) -> Fake {
            Fake::saying(&match cfg!(windows) {
                true => format!("echo {version} (Claude Code)"),
                false => format!("echo \"{version} (Claude Code)\""),
            })
        }

        /// A binary that takes about a second to say anything, and then says
        /// something unmistakable. `ping` is the platform's idiom for sleeping
        /// in a batch file without a console: `timeout` needs one, and
        /// `powershell -c Start-Sleep` costs more to start than it sleeps for.
        ///
        /// It prints a version *after* the sleep so that waiting for it has a
        /// visible consequence: a probe that sees this through to the end
        /// returns `Version("9.9.9")`, which no correct probe can do within a
        /// patience of milliseconds. That is what lets
        /// [`a_binary_that_does_not_answer_is_given_up_on`] assert on a value
        /// instead of on a stopwatch.
        fn dawdling() -> Fake {
            Fake::saying(match cfg!(windows) {
                true => "ping -n 2 127.0.0.1 >nul\r\necho 9.9.9 (Claude Code)",
                // Quoted for the same reason [`Fake::reporting`] quotes.
                false => "sleep 1\necho \"9.9.9 (Claude Code)\"",
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
        // Named the way this platform names the binary — `Fake::path` already
        // knows, and hardcoding `claude.cmd` meant the fallback searched `PATH`
        // for a file by that name and, off Windows, correctly failed to find it.
        let stale = fake
            .directory
            .path()
            .join("moved")
            .join(fake.path().file_name().expect("a file name"));

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
    ///
    /// **The returned value is the whole assertion, and that is deliberate.**
    /// `Probed::TimedOut` is produced in exactly one place — the deadline arm of
    /// the poll loop — so getting it back *is* the proof that the deadline is
    /// what ended the wait. A probe that instead waited for the child would have
    /// broken out of that loop and read its output, and come back
    /// `Version("9.9.9")`; the fake prints one so that the wrong behaviour has a
    /// name rather than being inferred.
    ///
    /// This test used to also assert `started.elapsed() < 900ms`. That assertion
    /// could not fail without this one failing first, since the only route to
    /// `TimedOut` already excludes waiting for the child — so all it added was a
    /// measurement of how long this machine takes to spawn a process, which is
    /// why it went red under load against correct code. Ticket 29; see the
    /// convention there before adding a wall-clock assertion anywhere in this
    /// repo.
    #[test]
    fn a_binary_that_does_not_answer_is_given_up_on() {
        let dawdling = Fake::dawdling();

        assert_eq!(
            probe(&dawdling.path(), Duration::from_millis(50)),
            Probed::TimedOut,
            "anything else means the probe waited for the child rather than for its deadline",
        );

        // And the other half: given patience, the same fake answers. Without
        // this the test above could pass against a fake that says nothing at
        // all — `TimedOut` for the wrong reason — and an assertion that cannot
        // distinguish the two behaviours is not testing either of them. This is
        // the second worth spending; it is what makes the first assertion mean
        // "gave up early" rather than "did not get a version".
        assert_eq!(
            probe(&dawdling.path(), PROBE_TIMEOUT),
            Probed::Version("9.9.9".to_string()),
            "the fake must be able to answer, or timing out proves nothing",
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

        refresh(&store, &fake.on_path(), &[]);

        let providers = &store.current().providers;
        assert_eq!(providers.len(), REGISTRY.len());
        assert_eq!(providers[0].version.as_deref(), Some("2.1.220"));
        assert_eq!(providers[0].status, ProviderState::Ready);
        assert_eq!(providers[1].instance_id, CODEX_INSTANCE_ID);
    }

    #[test]
    fn a_workspace_change_supersedes_an_initial_codex_probe_before_it_publishes() {
        let store = ConfigStore::new(crate::config::ServerConfig::detect());
        let initial = reserve_probes(&store)
            .codex
            .expect("the initial Codex probe is reserved");
        let rescan = reserve_skill_rescan(&store);

        assert!(
            rescan.codex.is_some(),
            "an enabled Codex provider needs a new probe even before its first snapshot lands"
        );
        assert!(!store.apply_providers_if_current(initial, |_| true, |_| Vec::new()));
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

        let provider = describe(&settings, &ready.on_path(), &[]);

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
