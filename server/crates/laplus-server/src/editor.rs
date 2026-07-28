//! Handing a file off to the developer's own editor.
//!
//! One method, `shell.openInEditor`, and one thing it contributes to
//! [`crate::config`]: the list of editors the UI is allowed to offer. Those are
//! two halves of one feature — a picker that offered an editor the machine does
//! not have would produce a failure the user could do nothing about — so they
//! live together.
//!
//! ## The target is a path, not a working directory
//!
//! `LaunchEditorInput` names its field `cwd`, and it is not one. The UI sends
//! whatever the user asked to open (`editorPreferences.ts` passes its
//! `targetPath` straight through), which is a file as often as a folder, and it
//! may carry a `:line` or `:line:column` suffix. The field name is upstream's
//! and is kept because it is on the wire; what it *means* is the target, and
//! this module treats it that way.
//!
//! ## v1 does not launch what it cannot find
//!
//! The table below is `EDITORS` from `t3code/packages/contracts/src/editor.ts`,
//! trimmed to the commands and the argument style, because the client decodes
//! `EditorId` against that exact list of literals — an id laplus invented
//! would fail to decode and cost the user the whole configuration payload.
//!
//! Two things upstream does are deliberately not ported: launching a *browser*
//! (the preview subsystem is out of scope) and the WSL detection around it
//! (WSL is out of scope).

use serde::Deserialize;
use serde_json::{json, Value};

use crate::process::Search;

/// Open a path in the developer's editor.
pub const OPEN_IN_EDITOR: &str = "shell.openInEditor";

/// How an editor wants to be told about a line and column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Style {
    /// Takes the path and nothing else. A `:12` suffix goes along for the ride,
    /// which is what upstream does — some of these editors parse it themselves.
    DirectPath,
    /// VS Code and its relatives: `--goto path:line:column`.
    Goto,
    /// JetBrains: `--line 12 --column 3 path`.
    LineColumn,
}

/// One editor the UI may offer, and how to start it.
struct Editor {
    /// The contract's `EditorId`. The client decodes against these literals.
    id: &'static str,
    /// Tried in order; the first one on `PATH` wins. Empty means the platform's
    /// file manager rather than a program with a name.
    commands: &'static [&'static str],
    style: Style,
    /// Arguments before the target, for the one editor that needs a subcommand.
    base_args: &'static [&'static str],
}

/// `EDITORS` from the contracts package, in the same order.
const EDITORS: &[Editor] = &[
    Editor { id: "cursor", commands: &["cursor"], style: Style::Goto, base_args: &[] },
    Editor { id: "trae", commands: &["trae"], style: Style::Goto, base_args: &[] },
    Editor { id: "kiro", commands: &["kiro"], style: Style::Goto, base_args: &["ide"] },
    Editor { id: "vscode", commands: &["code"], style: Style::Goto, base_args: &[] },
    Editor {
        id: "vscode-insiders",
        commands: &["code-insiders"],
        style: Style::Goto,
        base_args: &[],
    },
    Editor { id: "vscodium", commands: &["codium"], style: Style::Goto, base_args: &[] },
    Editor {
        id: "zed",
        commands: &["zed", "zeditor"],
        style: Style::DirectPath,
        base_args: &[],
    },
    Editor { id: "antigravity", commands: &["agy"], style: Style::Goto, base_args: &[] },
    Editor { id: "idea", commands: &["idea"], style: Style::LineColumn, base_args: &[] },
    Editor { id: "aqua", commands: &["aqua"], style: Style::LineColumn, base_args: &[] },
    Editor { id: "clion", commands: &["clion"], style: Style::LineColumn, base_args: &[] },
    Editor { id: "datagrip", commands: &["datagrip"], style: Style::LineColumn, base_args: &[] },
    Editor { id: "dataspell", commands: &["dataspell"], style: Style::LineColumn, base_args: &[] },
    Editor { id: "goland", commands: &["goland"], style: Style::LineColumn, base_args: &[] },
    Editor { id: "phpstorm", commands: &["phpstorm"], style: Style::LineColumn, base_args: &[] },
    Editor { id: "pycharm", commands: &["pycharm"], style: Style::LineColumn, base_args: &[] },
    Editor { id: "rider", commands: &["rider"], style: Style::LineColumn, base_args: &[] },
    Editor { id: "rubymine", commands: &["rubymine"], style: Style::LineColumn, base_args: &[] },
    Editor { id: "rustrover", commands: &["rustrover"], style: Style::LineColumn, base_args: &[] },
    Editor { id: "webstorm", commands: &["webstorm"], style: Style::LineColumn, base_args: &[] },
    // Always available: every platform has a file manager, and it is the
    // fallback for a machine with no editor on PATH at all.
    Editor { id: "file-manager", commands: &[], style: Style::DirectPath, base_args: &[] },
];

/// The editors this machine can actually start, for `server.getConfig`.
///
/// Resolved once at startup rather than per call. It is a `PATH` lookup per
/// candidate — twenty-odd of them — and the answer only changes when the user
/// installs an editor, which they will not do without restarting the app to see
/// it offered anyway.
pub fn available() -> Vec<String> {
    let search = Search::from_environment();
    EDITORS
        .iter()
        .filter(|editor| resolve(editor, &search).is_some())
        .map(|editor| editor.id.to_string())
        .collect()
}

/// The command this editor would be started with, if the machine has it.
///
/// The *name*, not the resolved path: it goes to `Command::new`, which does its
/// own lookup, and an editor installed twice should be started the way the user's
/// own shell would start it. The lookup here is only to decide whether to offer
/// it at all.
fn resolve(editor: &Editor, search: &Search) -> Option<String> {
    if editor.commands.is_empty() {
        return Some(file_manager().to_string());
    }
    editor
        .commands
        .iter()
        .find(|command| search.locate(command).is_some())
        .map(|command| (*command).to_string())
}

fn file_manager() -> &'static str {
    if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    }
}

// ---------------------------------------------------------------------------
// shell.openInEditor
// ---------------------------------------------------------------------------

/// A validated `shell.openInEditor` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenInEditor {
    target: String,
    editor: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OpenInEditorPayload {
    /// Upstream's field name. It is the target path — see the module docs.
    cwd: String,
    editor: String,
}

impl OpenInEditor {
    pub fn read(payload: &Value) -> Result<OpenInEditor, Value> {
        let read: OpenInEditorPayload = serde_json::from_value(payload.clone())
            .map_err(|_| unknown_editor(""))?;

        let target = read.cwd.trim().to_string();
        if target.is_empty() {
            // None of the five errors this method declares describes "the
            // request named no path", so the nearest one is used and the
            // detail is lost. Reachable only by a client that is not the UI —
            // `editorPreferences.ts` always sends a target — and the
            // alternative is an error the client cannot decode at all, which
            // would cost the connection rather than the call.
            return Err(unknown_editor(&read.editor));
        }

        Ok(OpenInEditor {
            target,
            editor: read.editor.trim().to_string(),
        })
    }

    /// Start the editor and let go of it.
    ///
    /// Blocking, and called from a blocking task: finding the command is a
    /// `PATH` walk. The child is deliberately **not** waited on — an editor
    /// runs for hours and the call has to answer now — and its standard streams
    /// go nowhere, so a chatty editor cannot fill a pipe nobody is draining and
    /// wedge itself.
    pub fn run(self) -> Result<Value, Value> {
        let Some(editor) = EDITORS.iter().find(|editor| editor.id == self.editor) else {
            return Err(unknown_editor(&self.editor));
        };

        let Some(command) = resolve(editor, &Search::from_environment()) else {
            // The UI only offers what `available()` reported, so reaching this
            // means the editor was uninstalled while the app was running. It
            // is still the client's own declared error rather than a crash.
            return Err(json!({
                "_tag": "ExternalLauncherCommandNotFoundError",
                "editor": self.editor,
                "command": editor.commands.first().copied().unwrap_or(file_manager()),
            }));
        };

        let args = arguments(editor, &self.target);
        match spawn(&command, &args) {
            // **`null`, not `{}`.** `WsShellOpenInEditorRpc` declares no
            // `success`, and `Rpc.make` defaults that to `Schema.Void`
            // (`effect/unstable/rpc/Rpc.ts`); `Schema.Void`'s JSON codec is
            // `undefinedToNull` (`effect/SchemaAST.ts`), so `null` is what the
            // reference server puts on the wire. Void's parser is `fromConst`
            // and would accept anything, so `{}` would have worked by accident
            // — which is not a reason to send it.
            Ok(()) => Ok(Value::Null),
            Err(error) => Err(json!({
                "_tag": "ExternalLauncherEditorSpawnError",
                "editor": self.editor,
                "target": self.target,
                "command": command,
                "args": args,
                // Required, not optional: `ExternalLauncherSpawnFields` types
                // it as a bare `Schema.Defect()`, so an error without it does
                // not decode — and this is the one path that reports a real
                // failure to start something.
                "cause": error.to_string(),
            })),
        }
    }
}

/// The one error this module reaches for when it cannot name anything better.
///
/// **No `message` field, and that is deliberate.** Unlike `ProjectReadFileError`
/// — whose captured payload in `fixtures/socket-wire/03-typed-error.ndjson`
/// carries one because its schema declares one — every `ExternalLauncher*`
/// class defines `message` as an override *getter* over its structured fields.
/// The client computes the sentence itself, so a `message` sent from here would
/// be an extra property the reference server never sends and the UI never
/// reads.
fn unknown_editor(editor: &str) -> Value {
    json!({"_tag": "ExternalLauncherUnknownEditorError", "editor": editor})
}

/// The arguments for one editor and one target, in that editor's own style.
fn arguments(editor: &Editor, target: &str) -> Vec<String> {
    let mut args: Vec<String> = editor.base_args.iter().map(|arg| arg.to_string()).collect();

    match (editor.style, position(target)) {
        (Style::Goto, Some(_)) => {
            args.push("--goto".to_string());
            args.push(target.to_string());
        }
        (Style::LineColumn, Some((path, line, column))) => {
            args.push("--line".to_string());
            args.push(line.to_string());
            if let Some(column) = column {
                args.push("--column".to_string());
                args.push(column.to_string());
            }
            args.push(path.to_string());
        }
        // Either the editor takes a bare path, or no position was given and
        // there is nothing to translate.
        _ => args.push(target.to_string()),
    }
    args
}

/// Split `path:line` or `path:line:column`, if that is what this is.
///
/// Deliberately fussy about what counts: a Windows path begins `C:\`, and a
/// rule that treated every colon as a position marker would turn every absolute
/// path on the platform v1 ships into a line number. Only trailing all-digit
/// groups count.
fn position(target: &str) -> Option<(&str, &str, Option<&str>)> {
    let (head, last) = target.rsplit_once(':')?;
    if !is_number(last) {
        return None;
    }

    match head.rsplit_once(':') {
        Some((path, middle)) if is_number(middle) && !path.is_empty() => {
            Some((path, middle, Some(last)))
        }
        _ if !head.is_empty() => Some((head, last, None)),
        _ => None,
    }
}

fn is_number(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())
}

/// Start the child and forget it.
fn spawn(command: &str, args: &[String]) -> std::io::Result<()> {
    let mut child = std::process::Command::new(command);
    child
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    crate::process::without_a_console(&mut child);

    // `spawn` and never `wait`. The child is reparented when this process
    // exits, which is what "open my editor" means — closing laplus must not
    // close the window the user switched to.
    child.spawn().map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor(id: &str) -> &'static Editor {
        EDITORS
            .iter()
            .find(|editor| editor.id == id)
            .expect("a known editor")
    }

    /// Each family is told about a line in the way it understands. Getting this
    /// wrong does not fail loudly — the editor opens at the top of the file, or
    /// opens a file named `main.rs:12` — so it is worth pinning per style.
    #[test]
    fn each_editor_family_is_given_the_position_in_its_own_dialect() {
        assert_eq!(
            arguments(editor("vscode"), "/repo/src/main.rs:12:5"),
            ["--goto", "/repo/src/main.rs:12:5"]
        );
        assert_eq!(
            arguments(editor("webstorm"), "/repo/src/main.rs:12:5"),
            ["--line", "12", "--column", "5", "/repo/src/main.rs"]
        );
        assert_eq!(
            arguments(editor("webstorm"), "/repo/src/main.rs:12"),
            ["--line", "12", "/repo/src/main.rs"]
        );
        assert_eq!(
            arguments(editor("zed"), "/repo/src/main.rs:12:5"),
            ["/repo/src/main.rs:12:5"]
        );
        // The one editor with a subcommand keeps it in front.
        assert_eq!(
            arguments(editor("kiro"), "/repo/src/main.rs:12"),
            ["ide", "--goto", "/repo/src/main.rs:12"]
        );
    }

    /// A plain path is a plain path, whichever style the editor uses.
    #[test]
    fn a_target_with_no_position_is_passed_through_untouched() {
        for id in ["vscode", "webstorm", "zed", "file-manager"] {
            assert_eq!(
                arguments(editor(id), "/repo/src"),
                ["/repo/src"],
                "{id}"
            );
        }
    }

    /// The rule that matters on the platform v1 ships: a drive letter is not a
    /// line number.
    #[test]
    fn a_windows_drive_letter_is_not_mistaken_for_a_position() {
        assert_eq!(position(r"C:\repo\src\main.rs"), None);
        assert_eq!(
            arguments(editor("webstorm"), r"C:\repo\src\main.rs"),
            [r"C:\repo\src\main.rs"]
        );

        // And a real position on a Windows path is still found.
        assert_eq!(
            position(r"C:\repo\src\main.rs:12:5"),
            Some((r"C:\repo\src\main.rs", "12", Some("5")))
        );
        assert_eq!(
            arguments(editor("webstorm"), r"C:\repo\src\main.rs:12:5"),
            ["--line", "12", "--column", "5", r"C:\repo\src\main.rs"]
        );
    }

    #[test]
    fn a_position_is_only_recognised_when_it_is_digits() {
        assert_eq!(position("/repo/main.rs"), None);
        assert_eq!(position("/repo/main.rs:"), None);
        assert_eq!(position("/repo/main.rs:abc"), None);
        assert_eq!(position(":12"), None);
        assert_eq!(position("/repo/main.rs:12"), Some(("/repo/main.rs", "12", None)));
    }

    /// The ids on the wire are the contract's literals. A client decodes
    /// `EditorId` against exactly this list, so an invented one would fail the
    /// whole `server.getConfig` payload rather than just the editor picker.
    #[test]
    fn every_advertised_editor_is_one_the_contract_declares() {
        let declared = [
            "cursor", "trae", "kiro", "vscode", "vscode-insiders", "vscodium", "zed",
            "antigravity", "idea", "aqua", "clion", "datagrip", "dataspell", "goland",
            "phpstorm", "pycharm", "rider", "rubymine", "rustrover", "webstorm",
            "file-manager",
        ];

        assert_eq!(
            EDITORS.iter().map(|editor| editor.id).collect::<Vec<&str>>(),
            declared
        );
        for id in available() {
            assert!(declared.contains(&id.as_str()), "{id} is not in the contract");
        }
    }

    /// Whatever else this machine has, it has somewhere to show a folder — so
    /// the UI always has at least one thing to offer.
    #[test]
    fn the_file_manager_is_always_available() {
        assert!(available().contains(&"file-manager".to_string()));
    }

    /// An editor the server does not know about fails its own call rather than
    /// starting something. The client sends an `EditorId`, so this is a
    /// contract violation or a stale client — either way, not a process to run.
    ///
    /// The refusal carries `editor` and nothing else, because that is all the
    /// contract's `ExternalLauncherUnknownEditorError` declares — its `message`
    /// is a getter the client computes for itself.
    #[test]
    fn an_unknown_editor_is_refused_without_spawning_anything() {
        let error = OpenInEditor::read(&json!({"cwd": "/repo", "editor": "emacs"}))
            .expect("a well-formed payload")
            .run()
            .expect_err("not an editor this server knows");

        assert_eq!(
            error,
            json!({"_tag": "ExternalLauncherUnknownEditorError", "editor": "emacs"})
        );
    }

    #[test]
    fn a_payload_without_a_target_is_refused() {
        for payload in [json!({"editor": "vscode"}), json!({"cwd": "  ", "editor": "vscode"})] {
            let error = OpenInEditor::read(&payload).expect_err("nothing to open");
            assert_eq!(error["_tag"], "ExternalLauncherUnknownEditorError");
            assert!(error["editor"].is_string(), "{error}");
        }
    }

    /// The file manager is the one editor every machine has, so it is the one
    /// this can actually start — and starting it is the only way to know the
    /// spawn path works at all.
    ///
    /// Skipped unless asked for: it opens a window on the developer's desktop,
    /// which a test suite has no business doing without being told to.
    #[test]
    fn the_file_manager_can_be_started() {
        if std::env::var_os("LAPLUS_TEST_LAUNCH_EDITOR").is_none() {
            eprintln!(
                "skipped: set LAPLUS_TEST_LAUNCH_EDITOR=1 to let this open a real window"
            );
            return;
        }

        let directory = tempfile::tempdir().expect("a temporary directory");
        OpenInEditor::read(&json!({
            "cwd": directory.path().to_string_lossy(),
            "editor": "file-manager",
        }))
        .expect("a well-formed payload")
        .run()
        .expect("the file manager starts");
    }
}
