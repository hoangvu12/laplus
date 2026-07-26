//! A stand-in for the `claude` binary.
//!
//! The suite has to run offline, for free, and on a machine that has never had
//! Claude Code installed — spec story 61, and the last criterion of ticket 09 in
//! so many words. So every test that needs an agent binary writes one: a file
//! this platform agrees is a program, which answers `--version` however the test
//! needs it answered.
//!
//! `provider.rs` has a smaller copy of this for its own unit tests, because an
//! integration test is a separate crate and cannot see the library's
//! `#[cfg(test)]` items. That much duplication is the language's rather than a
//! choice — but *duplicated tests* would not be, so the two files divide the work
//! rather than covering the same ground twice: the rules are driven in
//! `provider.rs`, what the UI observes is driven in `socket_provider.rs`, and its
//! header says which is which.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

/// A fake agent binary in a directory of its own, so the directory can be handed
/// to a lookup as if it were on `PATH`.
pub struct FakeAgent {
    directory: tempfile::TempDir,
}

impl FakeAgent {
    /// A binary that reports `version` the way the real one does —
    /// `2.1.220 (Claude Code)`, the version followed by the product name.
    pub fn reporting(version: &str) -> FakeAgent {
        FakeAgent::saying(&format!("echo {version} (Claude Code)"))
    }

    /// A binary that exits non-zero, like an install whose runtime is broken.
    pub fn failing() -> FakeAgent {
        FakeAgent::saying(match cfg!(windows) {
            true => "exit /b 1",
            false => "exit 1",
        })
    }

    /// A binary running one line of the platform's own script language.
    pub fn saying(script: &str) -> FakeAgent {
        let agent = FakeAgent {
            directory: tempfile::tempdir().expect("a temporary directory"),
        };
        let path = agent.path();

        if cfg!(windows) {
            // A `.cmd` rather than an `.exe`, because a test cannot compile one.
            // `std::process::Command` runs a batch file through `cmd.exe`, which
            // is precisely why this server does not need upstream's npm-shim
            // resolution — see `provider`'s module docs.
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
        agent
    }

    /// Where the binary is, under the name the resolver looks for.
    pub fn path(&self) -> PathBuf {
        self.directory.path().join(match cfg!(windows) {
            true => "claude.cmd",
            false => "claude",
        })
    }

    /// The path as the `binaryPath` setting would spell it.
    pub fn configured(&self) -> String {
        self.path().to_string_lossy().into_owned()
    }

    /// The directory to put on a lookup's `PATH`.
    pub fn directory(&self) -> &Path {
        self.directory.path()
    }

    /// A path inside this directory that does not exist — a `binaryPath` setting
    /// that has outlived its install.
    pub fn stale_path(&self) -> String {
        self.directory
            .path()
            .join("moved-away")
            .join(match cfg!(windows) {
                true => "claude.cmd",
                false => "claude",
            })
            .to_string_lossy()
            .into_owned()
    }
}

/// A stand-in `claude` that holds a whole conversation.
///
/// [`FakeAgent`] answers `--version` and nothing else, which is all ticket 09
/// needed. This one is the ticket 10 half: it reads a turn on stdin the way the
/// real CLI does under `--input-format stream-json`, replays a canned NDJSON
/// script to stdout, and waits for the next one. That is the whole of the
/// protocol from the server's side, so a turn driven against this exercises the
/// real driver, the real fold and the real socket — everything except the model.
///
/// It reaches the server through `settings.providers.claudeAgent.binaryPath`,
/// which is the path a developer configures for real use. The spec asks for
/// exactly that and says why: "no test-only seam is added to production code".
///
/// The suite therefore runs offline, for free, and on a machine that has never
/// had Claude Code installed — spec story 61.
pub struct ScriptedAgent {
    directory: tempfile::TempDir,
}

/// A place in a script where the agent stops for about a second.
///
/// What it buys is the difference between "the deltas were published" and "the
/// deltas were published *before the turn ended*", which is the second criterion
/// of ticket 10 and is not observable against an agent that answers
/// instantaneously.
pub const PAUSE: &str = "<pause>";

/// A file every scripted agent writes into whatever directory it was started
/// in, on every turn.
///
/// The only honest way to observe "the agent runs with the project directory as
/// its working directory", which is a criterion of ticket 10. The alternative —
/// having the agent print its own path in its output — would need the path JSON
/// escaped by a batch file, and a Windows path is mostly backslashes.
pub const WORKING_DIRECTORY_MARKER: &str = "lightcode-agent-was-here";

impl ScriptedAgent {
    /// Replay a committed capture — one of `fixtures/claude-cli/*.ndjson`, the
    /// same files the protocol module's golden tests are held to.
    ///
    /// A recording of a real turn, so a test driven against it is a test against
    /// what the CLI actually said rather than against what this project believes
    /// it says.
    pub fn replaying(capture: &str) -> ScriptedAgent {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/claude-cli")
            .join(format!("{capture}.ndjson"));
        let recorded = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()));

        ScriptedAgent::emitting(&recorded.lines().collect::<Vec<&str>>())
    }

    /// Replay lines written for the occasion.
    ///
    /// For the cases no recording contains, because a healthy CLI does not
    /// produce them: deltas that disagree with the buffered message, an errored
    /// result, a session that says nothing at all.
    pub fn emitting(lines: &[&str]) -> ScriptedAgent {
        let agent = ScriptedAgent {
            directory: tempfile::tempdir().expect("a temporary directory"),
        };

        let segments: Vec<&[&str]> = lines.split(|line| *line == PAUSE).collect();
        for (index, segment) in segments.iter().enumerate() {
            let mut text = segment.join("\n");
            text.push('\n');
            std::fs::write(agent.directory.path().join(segment_name(index)), text)
                .expect("writes a script segment");
        }

        std::fs::write(agent.path(), agent.script(segments.len())).expect("writes the agent");
        #[cfg(not(windows))]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(agent.path(), std::fs::Permissions::from_mode(0o755))
                .expect("sets the mode");
        }
        agent
    }

    /// The path as the `binaryPath` setting would spell it — which is how the
    /// server is told to use this instead of the developer's own install.
    pub fn configured(&self) -> String {
        self.path().to_string_lossy().into_owned()
    }

    /// How many times this agent has been *started*.
    ///
    /// The only way to tell "one process served two turns" from "two processes
    /// served one each": the live-agent gauge reads 1 either way, because a
    /// re-spawn is a decrement and an increment between two reads of it. The
    /// script appends a line before it begins its loop, so this counts
    /// processes rather than turns.
    pub fn starts(&self) -> usize {
        match std::fs::read_to_string(self.directory.path().join(STARTS_LOG)) {
            Ok(log) => log.lines().filter(|line| !line.trim().is_empty()).count(),
            // Nothing has run yet, which is zero starts rather than a failure —
            // a test may reasonably assert that.
            Err(_) => 0,
        }
    }

    fn path(&self) -> PathBuf {
        self.directory.path().join(match cfg!(windows) {
            true => "claude.cmd",
            false => "claude",
        })
    }

    /// The replay loop, in the platform's own script language.
    ///
    /// One turn in, one script out, and back to waiting — which is the shape the
    /// real CLI has under `--input-format stream-json` and the reason the server
    /// may keep one process for a whole conversation. The loop ends when stdin
    /// closes, so closing it is what stops the agent, exactly as it is for the
    /// real one.
    fn script(&self, segments: usize) -> String {
        // A relative path, so it lands wherever the agent was started — which is
        // the whole point of writing it.
        let mut replay = match cfg!(windows) {
            true => format!("echo.>\"{WORKING_DIRECTORY_MARKER}\"\r\n"),
            false => format!("  : > \"{WORKING_DIRECTORY_MARKER}\"\n"),
        };
        for index in 0..segments {
            if index > 0 {
                // A second, in the platform's idiom for sleeping without a
                // console: `timeout` needs one, and `powershell -c Start-Sleep`
                // costs more to start than it sleeps for.
                replay.push_str(match cfg!(windows) {
                    true => "ping -n 2 127.0.0.1 >nul\r\n",
                    false => "  sleep 1\n",
                });
            }
            replay.push_str(&match cfg!(windows) {
                true => format!("type \"%~dp0{}\"\r\n", segment_name(index)),
                false => format!("  cat \"$here/{}\"\n", segment_name(index)),
            });
        }

        if cfg!(windows) {
            format!(
                "@echo off\r\n\
                 if not \"%~1\"==\"--version\" goto started\r\n\
                 echo 2.1.220 ^(Claude Code^)\r\n\
                 exit /b 0\r\n\
                 :started\r\n\
                 rem One line per process, beside the script rather than in the\r\n\
                 rem project — this counts starts, and the project is where the\r\n\
                 rem working-directory marker goes.\r\n\
                 echo started>>\"%~dp0{STARTS_LOG}\"\r\n\
                 :turns\r\n\
                 rem `set /p` leaves the variable undefined when stdin has\r\n\
                 rem closed, which is how this loop hears that the server is\r\n\
                 rem finished with it.\r\n\
                 set \"LINE=\"\r\n\
                 set /p LINE=\r\n\
                 if not defined LINE exit /b 0\r\n\
                 {replay}goto turns\r\n"
            )
        } else {
            format!(
                "#!/bin/sh\n\
                 if [ \"$1\" = \"--version\" ]; then\n\
                 \x20 echo \"2.1.220 (Claude Code)\"\n\
                 \x20 exit 0\n\
                 fi\n\
                 here=$(dirname \"$0\")\n\
                 echo started >> \"$here/{STARTS_LOG}\"\n\
                 while IFS= read -r line; do\n\
                 {replay}done\n"
            )
        }
    }
}

fn segment_name(index: usize) -> String {
    format!("segment-{index}.ndjson")
}

/// One line per process the agent script has started, beside the script itself.
/// Read by [`ScriptedAgent::starts`].
const STARTS_LOG: &str = "starts.log";
