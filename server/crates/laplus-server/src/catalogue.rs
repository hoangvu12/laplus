//! What the composer can offer: the agent's slash commands and the developer's
//! skills.
//!
//! Two lists on the provider snapshot, read by the two menus the composer opens
//! — `/` for commands and `$` for skills (`ChatComposer.tsx`,
//! `composerMenuItems`). Both were empty in every snapshot this server sent
//! until now, so both menus were empty, which is the whole of what this module
//! is for.
//!
//! Selecting from either menu only *types into the prompt*: a command becomes
//! `/name ` and a skill becomes `$name ` in the composer's text
//! (`onComposerMenuItemSelected`). So neither list is a capability this server
//! implements — the CLI is what acts on the text — and publishing one is a claim
//! about what the agent will recognise rather than about what this server will
//! do. That is why both halves below are read from where the CLI itself reads
//! them, and why neither is a table written here.
//!
//! ## The two halves have different sources, and no choice about it
//!
//! **Commands are asked of the running binary.** `/clear`, `/compact`,
//! `/model`, `/context` and the rest of the built-ins are compiled into `claude`
//! and exist nowhere on disk, so the filesystem scan that answers the skills half
//! cannot produce the one the developer is most likely to want. The CLI will
//! list them, over the control channel, in answer to the `initialize` request the
//! Agent SDK opens every session with — see [`commands`], which is a session
//! started to ask that one question.
//!
//! **Skills come from the filesystem.** They are on disk by construction — a
//! directory with a `SKILL.md` in it — and while the handshake lists them among
//! its commands, it lists them without the paths and scopes the `$` menu shows.
//! Upstream reaches the same conclusion and scans the same two places
//! (`ClaudeSkills.ts`, whose header says so).
//!
//! ## What this deliberately does not do
//!
//! **Project-scoped commands are missing, where project-scoped skills are not.**
//! The asymmetry is the probe's: a `claude` reads `<cwd>/.claude/commands` for
//! the project it was started in, and this server has no one project — it has a
//! registry of them, and a probe per project would be a `claude` per project on
//! every refresh. A skill costs a `readdir`, so every registered root is scanned;
//! a command costs a process, so the handshake happens once, wherever the server
//! is, and yields the built-ins and the developer's own user-scoped ones. A
//! project's `.claude/commands` therefore does not appear in the `/` menu.
//! Typing its name still works, because the CLI is what expands it.
//!
//! **Nothing here fails a provider.** Every failure is an empty list: a menu
//! with nothing in it is a menu, and a provider reported broken because a
//! `readdir` was refused would be this server turning a missing convenience into
//! a missing agent.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::config::ClaudeSettings;

/// The two lists, as the provider snapshot carries them.
#[derive(Debug, Default, PartialEq)]
pub struct Catalogue {
    pub slash_commands: Vec<Value>,
    pub skills: Vec<Value>,
}

/// How long the binary has to answer the handshake.
///
/// A deadlock guard rather than a budget, like [`crate::provider`]'s own probe,
/// and half its length because the two wait for different things. `--version` is
/// a whole process running to completion; this is one answer from a process that
/// has been asked nothing else, composed locally and before the CLI has spoken to
/// anything over a network. The cost of giving up early is a `/` menu with only
/// the composer's own three entries in it, not a provider reported broken — so
/// this can afford to be the tighter of the two.
const HELLO_TIMEOUT: Duration = Duration::from_secs(5);

/// Read both lists.
///
/// `binary` is `None` when the provider has no agent that answered, which is the
/// case where there is nothing to probe: the skills are still scanned, because
/// they are the developer's files and are there whether or not the agent is.
pub fn read(settings: &ClaudeSettings, binary: Option<&Path>, roots: &[PathBuf]) -> Catalogue {
    Catalogue {
        slash_commands: binary
            .map(|binary| commands(binary, HELLO_TIMEOUT))
            .unwrap_or_default(),
        skills: skills(settings, roots),
    }
}

// Commands
// ---------------------------------------------------------------------------

/// The handshake, as the CLI's control channel expects it.
///
/// `--input-format stream-json` is itself a control channel — the same one an
/// interrupt travels on ([`crate::protocol`]) — and `initialize` is the request
/// the Agent SDK opens every session with. The reply carries the commands.
///
/// The id is fixed rather than minted because this connection carries exactly
/// one request and is killed after the answer, so there is nothing for a unique
/// id to disambiguate.
const HANDSHAKE: &str =
    r#"{"type":"control_request","request_id":"laplus-initialize","request":{"subtype":"initialize"}}"#;

/// Every command the binary knows, as `ServerProviderSlashCommand`s.
///
/// **The session is asked rather than watched**, and that is the whole finding
/// behind this function. The obvious route is the `system/init` line, which
/// lists `slash_commands` and is the first thing a turn produces — but the CLI
/// does not write one until it has been given a prompt. Started and left alone
/// it emits its session hooks and then waits, with stdin held open or closed to
/// no effect; measured, not reasoned about. So a probe that waited for `init`
/// would wait for the developer to say something first, which is exactly the
/// moment it is too late to have populated the menu.
///
/// `initialize` is answered immediately and answers better: `init`'s
/// `slash_commands` is an array of bare names, and this is an array of objects
/// carrying the description and argument hint the menu shows. It is what
/// upstream reads too, one layer up — the Agent SDK sends this request, and
/// `probeClaudeCapabilities` reads the same three fields off its result.
fn commands(binary: &Path, patience: Duration) -> Vec<Value> {
    let Some(answer) = handshake(binary, patience) else {
        return Vec::new();
    };

    let mut listed: Vec<Value> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for command in answer
        .get("commands")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        let Some(name) = text(command.get("name")) else {
            continue;
        };
        // The menu keys off the name and the CLI is not case-sensitive about
        // one, so a duplicate would be two rows that type the same thing.
        // Upstream dedupes for the same reason (`dedupeSlashCommands`).
        let key = name.to_lowercase();
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);

        let mut listed_command = json!({ "name": name });
        if let Some(description) = text(command.get("description")) {
            listed_command["description"] = Value::String(description);
        }
        // The contract nests it, and the menu shows it as the row's subtitle when
        // there is no description. Absent rather than empty: `TrimmedNonEmptyString`
        // refuses `""`, and the CLI sends `""` for a command that takes nothing.
        if let Some(hint) = text(command.get("argumentHint")) {
            listed_command["input"] = json!({ "hint": hint });
        }
        listed.push(listed_command);
    }
    listed
}

/// A trimmed, non-empty string, or nothing. The contract's own rule for every
/// optional string here, applied before the value can reach a payload the client
/// would refuse to decode.
fn text(value: Option<&Value>) -> Option<String> {
    let text = value?.as_str()?.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// Start a session, ask it to introduce itself, and stop it.
///
/// Returns the answer's payload, or `None` if the binary did not produce one
/// before the patience ran out — which covers every way this can fail
/// deliberately: unstartable, wedged, exiting at once, refusing the request, or
/// a build whose handshake has moved. The caller's answer to all five is the
/// same empty menu.
///
/// The child is killed rather than asked to leave. It has taken no turn and
/// holds no session, so there is nothing to lose by it, and the alternative is
/// waiting on a shutdown for output nobody will read.
fn handshake(binary: &Path, patience: Duration) -> Option<Value> {
    let mut command = Command::new(binary);
    command
        // The subset of [`crate::agent`]'s flags that constitutes the protocol.
        // No `--model`, `--permission-mode` or `--resume`: this session will
        // never take a turn, and naming a model would make the probe fail over a
        // model the developer had mistyped rather than over the binary.
        .arg("--print")
        .arg("--input-format")
        .arg("stream-json")
        .arg("--output-format")
        .arg("stream-json")
        .arg("--verbose")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    crate::process::without_a_console(&mut command);

    let mut child = command.spawn().ok()?;
    let mut stdin = child.stdin.take()?;
    let stdout = child.stdout.take()?;

    // A thread, because a pipe has no bounded read: the reader blocks until a
    // line arrives and the deadline is enforced out here. It ends when the child
    // does or when nobody is listening, so killing the child below collects it.
    let (lines, output) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if lines.send(line).is_err() {
                return;
            }
        }
    });

    let asked = writeln!(stdin, "{HANDSHAKE}").and_then(|()| stdin.flush());
    let mut answer = None;
    if asked.is_ok() {
        let deadline = Instant::now() + patience;
        while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
            // The channel closes when the child's stdout does, which is how a
            // binary that is not this CLI at all — or is a version that refuses
            // these flags — ends this loop promptly rather than at the deadline.
            let Ok(line) = output.recv_timeout(remaining) else {
                break;
            };
            // Lines before the answer are routine: the CLI reports its session
            // hooks first, which `fixtures/claude-cli/07` shows it doing.
            let Ok(said) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if said.get("type") != Some(&json!("control_response")) {
                continue;
            }
            let response = &said["response"];
            // Matched on carrying commands rather than on the request id: this
            // connection has exactly one request outstanding, and a `subtype` of
            // `error` — the shape a CLI that does not know this request answers
            // with — has no commands and so ends the loop with `None`.
            if response["response"].get("commands").is_some() {
                answer = Some(response["response"].clone());
            }
            break;
        }
    }

    // Before the child, so it sees the pipe close if the kill loses a race with
    // its own exit.
    drop(stdin);
    let _ = child.kill();
    let _ = child.wait();
    answer
}

// Skills
// ---------------------------------------------------------------------------

/// Every skill the agent would load, as `ServerProviderSkill`s.
///
/// The two scopes the CLI reads, in the order that makes the more specific one
/// win: the developer's own, and then each project's, so a project skill
/// replaces a user skill of the same name. Upstream resolves collisions the same
/// way and for the same reason.
pub fn skills(settings: &ClaudeSettings, roots: &[PathBuf]) -> Vec<Value> {
    let mut found: Vec<Value> = Vec::new();

    collect(&config_dir(settings).join("skills"), "user", &mut found);
    for root in roots {
        collect(&root.join(".claude").join("skills"), "project", &mut found);
    }

    found
}

/// Read one skills directory into the list, replacing any same-named skill
/// already there.
///
/// Best-effort at every step: a root that is not there, an entry that is not a
/// directory, a `SKILL.md` that cannot be read or has no frontmatter — each is
/// skipped rather than reported, because a broken skill must not cost the
/// working ones beside it.
fn collect(directory: &Path, scope: &str, found: &mut Vec<Value>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };

    // Sorted, because `read_dir` has no defined order and this list becomes a
    // menu: a picker whose rows moved between refreshes would be one nobody
    // could learn.
    let mut names: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    names.sort();

    for path in names {
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let manifest = path.join("SKILL.md");
        let Ok(contents) = std::fs::read_to_string(&manifest) else {
            continue;
        };
        let (declared, description) = frontmatter(&contents);
        // The directory's name, not the frontmatter's: the directory is what the
        // agent is invoked with, and a `name:` that disagreed with it would put a
        // label in the menu that types the wrong thing into the composer. The
        // declared name is offered as the display name instead, which is what it
        // can honestly be.
        let mut skill = json!({
            "name": name,
            "path": manifest.display().to_string(),
            "scope": scope,
            "enabled": true,
        });
        if let Some(description) = description {
            skill["description"] = Value::String(description);
        }
        if let Some(declared) = declared.filter(|declared| declared != name) {
            skill["displayName"] = Value::String(declared);
        }

        match found.iter().position(|held| held["name"] == skill["name"]) {
            Some(held) => found[held] = skill,
            None => found.push(skill),
        }
    }
}

/// The `name` and `description` out of a `SKILL.md`'s YAML frontmatter.
///
/// A reader for the two scalar keys this needs rather than a YAML parser, and
/// that is a deliberate trade: the alternative is a dependency in a project with
/// a size target, to read two strings out of a five-line header. What it gives up
/// is every YAML feature a skill author could reach for — block scalars, quoted
/// keys, anchors — and the cost of meeting one is a missing description on one
/// row, not a failure.
///
/// Both values are unwrapped from matching quotes, because a description
/// containing a colon has to be quoted and the quotes are not part of it.
fn frontmatter(contents: &str) -> (Option<String>, Option<String>) {
    let mut lines = contents.lines();
    // The opening fence has to be the first line: a `---` further down is a
    // horizontal rule in the body, and reading the prose after one as fields
    // would put a paragraph in the menu.
    if lines.next().map(str::trim) != Some("---") {
        return (None, None);
    }

    let (mut name, mut description) = (None, None);
    for line in lines {
        if line.trim() == "---" {
            break;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = unquoted(value.trim());
        if value.is_empty() {
            continue;
        }
        match key.trim() {
            "name" => name = Some(value.to_string()),
            "description" => description = Some(value.to_string()),
            _ => {}
        }
    }
    (name, description)
}

fn unquoted(value: &str) -> &str {
    for quote in ['"', '\''] {
        if let Some(inner) = value.strip_prefix(quote).and_then(|v| v.strip_suffix(quote)) {
            return inner;
        }
    }
    value
}

/// The directory the CLI keeps the developer's configuration in, by the same
/// precedence the spawned agent will see: the provider's own `homePath` first,
/// then `CLAUDE_CONFIG_DIR` from this process's environment, then `~/.claude`.
///
/// It has to agree with what the agent reads or this scans a directory nobody
/// loads skills from — upstream's `resolveClaudeConfigDirPath` makes the same
/// point about the same three.
pub(crate) fn config_dir(settings: &ClaudeSettings) -> PathBuf {
    let configured = settings.home_path.trim();
    if !configured.is_empty() {
        return PathBuf::from(configured);
    }
    if let Some(from_environment) = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
    {
        return from_environment;
    }
    home_dir().join(".claude")
}

/// The developer's home directory, by the variables the CLI itself would read.
///
/// A relative `.claude` is the last resort rather than a panic: it resolves
/// against wherever the server was started, finds nothing, and yields an empty
/// list — which is the same outcome as every other way this can come up short.
fn home_dir() -> PathBuf {
    for variable in ["USERPROFILE", "HOME"] {
        if let Some(home) = std::env::var_os(variable).filter(|home| !home.is_empty()) {
            return PathBuf::from(home);
        }
    }
    PathBuf::from(".")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings_in(home: &Path) -> ClaudeSettings {
        ClaudeSettings {
            enabled: true,
            binary_path: "claude".to_string(),
            home_path: home.display().to_string(),
            launch_args: String::new(),
            custom_models: Vec::new(),
        }
    }

    /// Write a skill the way the CLI expects to find one.
    fn skill(root: &Path, name: &str, manifest: &str) {
        let directory = root.join(name);
        std::fs::create_dir_all(&directory).expect("a skill directory");
        std::fs::write(directory.join("SKILL.md"), manifest).expect("a manifest");
    }

    /// The fields the `$` menu reads by name, from the place the agent reads
    /// them.
    #[test]
    fn a_skill_is_found_where_the_agent_would_find_it() {
        let home = tempfile::tempdir().expect("a home");
        skill(
            &home.path().join("skills"),
            "web-animation-design",
            "---\nname: Web animation design\ndescription: Motion that feels deliberate.\n---\n\nBody.\n",
        );

        let found = skills(&settings_in(home.path()), &[]);

        assert_eq!(found.len(), 1, "{found:#?}");
        assert_eq!(found[0]["name"], "web-animation-design");
        assert_eq!(found[0]["description"], "Motion that feels deliberate.");
        assert_eq!(found[0]["displayName"], "Web animation design");
        assert_eq!(found[0]["scope"], "user");
        assert_eq!(found[0]["enabled"], json!(true));
        assert!(found[0]["path"]
            .as_str()
            .expect("a path")
            .ends_with("SKILL.md"));
    }

    /// The name is the directory's, because the directory is what gets typed
    /// into the composer. A `name:` that disagrees is a label and nothing more.
    #[test]
    fn the_name_is_the_directory_rather_than_what_the_manifest_claims() {
        let home = tempfile::tempdir().expect("a home");
        skill(
            &home.path().join("skills"),
            "tdd",
            "---\nname: Something Else Entirely\n---\n",
        );

        let found = skills(&settings_in(home.path()), &[]);
        assert_eq!(found[0]["name"], "tdd");
        assert_eq!(found[0]["displayName"], "Something Else Entirely");
    }

    /// A project's skill beats the developer's own of the same name, which is
    /// the CLI's own most-specific-wins rule.
    #[test]
    fn a_project_skill_replaces_a_user_skill_of_the_same_name() {
        let home = tempfile::tempdir().expect("a home");
        let project = tempfile::tempdir().expect("a project");
        skill(
            &home.path().join("skills"),
            "review",
            "---\ndescription: The user's own.\n---\n",
        );
        skill(
            &project.path().join(".claude").join("skills"),
            "review",
            "---\ndescription: This repository's.\n---\n",
        );

        let found = skills(
            &settings_in(home.path()),
            &[project.path().to_path_buf()],
        );

        assert_eq!(found.len(), 1, "{found:#?}");
        assert_eq!(found[0]["description"], "This repository's.");
        assert_eq!(found[0]["scope"], "project");
    }

    /// Every way a skill can be malformed, and none of them costs the working
    /// skill beside it. The menu is a convenience; a broken row must not be able
    /// to empty it.
    #[test]
    fn a_broken_skill_is_skipped_rather_than_breaking_the_list() {
        let home = tempfile::tempdir().expect("a home");
        let skills_dir = home.path().join("skills");
        // No manifest at all.
        std::fs::create_dir_all(skills_dir.join("empty-directory")).expect("a directory");
        // A manifest with no frontmatter, which is still a skill — the agent
        // loads it, so it belongs in the menu without a description.
        skill(&skills_dir, "no-frontmatter", "Just a body.\n");
        // A `---` that is a horizontal rule rather than a fence.
        skill(&skills_dir, "late-fence", "Body first.\n---\nname: Nope\n");
        // A file where a directory should be.
        std::fs::write(skills_dir.join("stray.md"), "not a skill").expect("a stray file");
        skill(&skills_dir, "working", "---\ndescription: Fine.\n---\n");

        let found = skills(&settings_in(home.path()), &[]);
        let named: Vec<&str> = found
            .iter()
            .map(|skill| skill["name"].as_str().unwrap_or_default())
            .collect();

        assert_eq!(named, vec!["late-fence", "no-frontmatter", "working"]);
        assert_eq!(found[0]["description"], Value::Null, "{:#?}", found[0]);
        assert_eq!(found[2]["description"], "Fine.");
    }

    /// A root that is not there, which is the ordinary case for a project with
    /// no skills of its own.
    #[test]
    fn nowhere_to_look_is_an_empty_list_rather_than_a_failure() {
        let home = tempfile::tempdir().expect("a home");
        let found = skills(
            &settings_in(&home.path().join("does-not-exist")),
            &[PathBuf::from("also-not-there")],
        );
        assert!(found.is_empty(), "{found:#?}");
    }

    /// Quoting is the author's business and not part of the value. A description
    /// containing a colon *has* to be quoted, so this is the common case rather
    /// than an exotic one.
    #[test]
    fn a_quoted_value_arrives_without_its_quotes() {
        let (name, description) = frontmatter("---\nname: 'tdd'\ndescription: \"Red, green: refactor.\"\n---\n");
        assert_eq!(name.as_deref(), Some("tdd"));
        assert_eq!(description.as_deref(), Some("Red, green: refactor."));
    }
}
