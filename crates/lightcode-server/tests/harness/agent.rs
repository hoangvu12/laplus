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

/// A place where the agent stops until the server writes it a line, and records
/// what it was written.
///
/// The real CLI does this at exactly one point: having asked permission, it
/// waits. Nothing else it sends needs a reply, so nothing else in a script needs
/// this — and [`ScriptedAgent::replaying`] inserts one after every
/// `control_request` in a recording for that reason.
///
/// **The line is not required.** If the server closes stdin instead of answering,
/// the script carries on with the rest of the recording rather than stopping,
/// which is what the CLI itself does: the permission stream closes, the tool
/// comes back as an abort, and the turn finishes. See
/// `fixtures/claude-cli/09-permission-unanswered.ndjson`, where that is the
/// recorded behaviour being replayed.
pub const AWAIT_ANSWER: &str = "<answer>";

/// A file every scripted agent writes into whatever directory it was started
/// in, on every turn.
///
/// The only honest way to observe "the agent runs with the project directory as
/// its working directory", which is a criterion of ticket 10. The alternative —
/// having the agent print its own path in its output — would need the path JSON
/// escaped by a batch file, and a Windows path is mostly backslashes.
pub const WORKING_DIRECTORY_MARKER: &str = "lightcode-agent-was-here";

/// What a scripted agent does when it is asked to continue a conversation.
enum OnResume {
    /// What a healthy CLI does: resume, announce a session, take the turn.
    Continue,
    /// What one whose stored conversation has gone does: complain on stderr and
    /// exit without a line of NDJSON. Ticket 11's "the underlying agent session
    /// is no longer available", and no recording contains it.
    Refuse,
}

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

        ScriptedAgent::emitting(&replayable(&recorded))
    }

    /// Replay a capture for the first turn, then a script of your own for every
    /// turn after it.
    ///
    /// For what a single recording cannot show: that the conversation *continues*
    /// after whatever the recording was of. Replaying the same capture twice
    /// would have the agent ask permission again on the follow-up, which is a
    /// second question rather than an answer to "did the session survive the
    /// first one".
    pub fn replaying_then(capture: &str, later: &[&str]) -> ScriptedAgent {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/claude-cli")
            .join(format!("{capture}.ndjson"));
        let recorded = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()));

        ScriptedAgent::per_turn(&[replayable(&recorded), later.to_vec()])
    }

    /// Replay lines written for the occasion, for every turn.
    ///
    /// For the cases no recording contains, because a healthy CLI does not
    /// produce them: deltas that disagree with the buffered message, an errored
    /// result, a session that says nothing at all.
    pub fn emitting(lines: &[&str]) -> ScriptedAgent {
        ScriptedAgent::written(&[lines], OnResume::Continue)
    }

    /// Replay a *different* script for each turn: the first turn gets the first,
    /// the second the second, and any turn past the end gets the last again.
    ///
    /// The counter is a variable inside the process, which is what makes this the
    /// discriminator ticket 11's continuity needs. A server that re-spawned the
    /// agent per turn would reset it and answer the second turn with the first
    /// script — so a reply that only the second script contains is proof that one
    /// session took both turns, and that is exactly what "a follow-up retains
    /// prior context" rests on: the context is the agent's, and the agent is the
    /// same one.
    pub fn per_turn<'a>(turns: &[impl AsRef<[&'a str]>]) -> ScriptedAgent {
        ScriptedAgent::written(turns, OnResume::Continue)
    }

    /// An agent that will not continue a conversation it is asked to resume.
    ///
    /// A fresh start behaves normally; one carrying `--resume` writes its reason
    /// to stderr and exits without producing anything, which is what the CLI does
    /// when the session id names nothing it still holds.
    pub fn refusing_to_resume<'a>(turns: &[impl AsRef<[&'a str]>]) -> ScriptedAgent {
        ScriptedAgent::written(turns, OnResume::Refuse)
    }

    fn written<'a>(turns: &[impl AsRef<[&'a str]>], on_resume: OnResume) -> ScriptedAgent {
        assert!(!turns.is_empty(), "an agent has to say something");
        let agent = ScriptedAgent {
            directory: tempfile::tempdir().expect("a temporary directory"),
        };

        // Each turn is split into segments at a marker, so a script can stop in
        // the middle of a turn as well as differ from one turn to the next. The
        // marker is kept with the segment it precedes, because *which* stop it
        // was decides what the script does there — sleep, or wait to be answered.
        let scripted: Vec<Vec<Segment<'_>>> =
            turns.iter().map(|lines| segments(lines.as_ref())).collect();
        for (turn, segments) in scripted.iter().enumerate() {
            for (index, segment) in segments.iter().enumerate() {
                let mut text = segment.lines.join("\n");
                text.push('\n');
                std::fs::write(agent.directory.path().join(segment_name(turn, index)), text)
                    .expect("writes a script segment");
            }
        }

        std::fs::write(agent.path(), agent.script(&scripted, on_resume))
            .expect("writes the agent");
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
        self.logged(STARTS_LOG).len()
    }

    /// The arguments each start was given, one entry per process.
    ///
    /// How a test observes what the server asked the CLI for without reaching
    /// into the server: the argv is the contract between the two, and
    /// `--resume` is the whole of ticket 11's continuity mechanism.
    pub fn arguments(&self) -> Vec<String> {
        self.logged(ARGUMENTS_LOG)
    }

    /// Every line the server wrote to this agent at an [`AWAIT_ANSWER`], in
    /// order.
    ///
    /// The only way to see what the *agent* was told from outside the server, and
    /// the assertion the approval and rejection tests turn on: an approval row in
    /// the work log says what the developer clicked, and this says what the agent
    /// received. A server that published the first and sent the second wrongly
    /// would pass every other assertion in the suite.
    pub fn answers(&self) -> Vec<String> {
        self.logged(ANSWERS_LOG)
    }

    /// The sessions this agent was asked to resume, in the order it was asked.
    ///
    /// Empty when every start was a fresh conversation, which is what a single
    /// run of the server produces — a long-lived child needs no resuming.
    pub fn resumed(&self) -> Vec<String> {
        self.arguments()
            .iter()
            .filter_map(|argv| {
                let mut words = argv.split_whitespace();
                words.find(|word| *word == "--resume")?;
                words.next().map(str::to_string)
            })
            .collect()
    }

    fn logged(&self, name: &str) -> Vec<String> {
        match std::fs::read_to_string(self.directory.path().join(name)) {
            Ok(log) => log
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_string)
                .collect(),
            // Nothing has run yet, which is an empty log rather than a failure —
            // a test may reasonably assert that.
            Err(_) => Vec::new(),
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
    ///
    /// The turn counter lives in the process, so which script a turn gets is also
    /// an answer to "was this the same process as last time". See
    /// [`ScriptedAgent::per_turn`].
    fn script(&self, turns: &[Vec<Segment<'_>>], on_resume: OnResume) -> String {
        match cfg!(windows) {
            true => self.batch(turns, on_resume),
            false => self.shell(turns, on_resume),
        }
    }

    fn batch(&self, turns: &[Vec<Segment<'_>>], on_resume: OnResume) -> String {
        // Every redirection here leads its command. `>>file echo %*` rather than
        // `echo %*>>file`, because a `%*` ending in a digit turns the digit into
        // a file-descriptor number and `--resume abc1>>log` would redirect fd 1
        // and lose the "1" out of the text.
        let refusal = match on_resume {
            OnResume::Continue => String::new(),
            OnResume::Refuse => format!(
                "echo %* | findstr /C:\"--resume\" >nul\r\n\
                 if not errorlevel 1 (\r\n\
                 \x20 >&2 echo {REFUSAL}\r\n\
                 \x20 exit /b 1\r\n\
                 )\r\n"
            ),
        };

        // One `if` per turn, and the last one catches every turn past the end so
        // a conversation longer than the script keeps answering.
        let mut dispatch = String::new();
        let mut bodies = String::new();
        for turn in 1..=turns.len() {
            dispatch.push_str(&match turn == turns.len() {
                true => format!("if %TURN% GEQ {turn} call :turn-{turn}\r\n"),
                false => format!("if \"%TURN%\"==\"{turn}\" call :turn-{turn}\r\n"),
            });
            bodies.push_str(&format!("\r\n:turn-{turn}\r\n"));
            for (index, segment) in turns[turn - 1].iter().enumerate() {
                match segment.after {
                    // A second, in the platform's idiom for sleeping without a
                    // console: `timeout` needs one, and `powershell -c
                    // Start-Sleep` costs more to start than it sleeps for.
                    Some(Stop::Pause) => bodies.push_str("ping -n 2 127.0.0.1 >nul\r\n"),
                    // `set /p` leaves the variable undefined at EOF, which is how
                    // "the server closed stdin instead of answering" arrives here.
                    // The script carries on either way — see [`AWAIT_ANSWER`].
                    Some(Stop::Answer) => bodies.push_str(&format!(
                        "set \"ANSWER=\"\r\n\
                         set /p ANSWER=\r\n\
                         if defined ANSWER >>\"%~dp0{ANSWERS_LOG}\" echo %ANSWER%\r\n"
                    )),
                    None => {}
                }
                bodies.push_str(&format!("type \"%~dp0{}\"\r\n", segment_name(turn - 1, index)));
            }
            bodies.push_str("goto :eof\r\n");
        }

        format!(
            "@echo off\r\n\
             if not \"%~1\"==\"--version\" goto started\r\n\
             echo 2.1.220 ^(Claude Code^)\r\n\
             exit /b 0\r\n\
             :started\r\n\
             rem The argv of this start, so a test can see what the server asked\r\n\
             rem for — `--resume` above all.\r\n\
             >>\"%~dp0{ARGUMENTS_LOG}\" echo %*\r\n\
             {refusal}\
             rem One line per process, beside the script rather than in the\r\n\
             rem project — this counts starts, and the project is where the\r\n\
             rem working-directory marker goes.\r\n\
             >>\"%~dp0{STARTS_LOG}\" echo started\r\n\
             set TURN=0\r\n\
             :turns\r\n\
             rem `set /p` leaves the variable undefined when stdin has\r\n\
             rem closed, which is how this loop hears that the server is\r\n\
             rem finished with it.\r\n\
             set \"LINE=\"\r\n\
             set /p LINE=\r\n\
             if not defined LINE exit /b 0\r\n\
             rem A relative path, so it lands wherever the agent was started —\r\n\
             rem which is the whole point of writing it.\r\n\
             echo.>\"{WORKING_DIRECTORY_MARKER}\"\r\n\
             set /a TURN+=1\r\n\
             {dispatch}\
             goto turns\r\n\
             {bodies}"
        )
    }

    fn shell(&self, turns: &[Vec<Segment<'_>>], on_resume: OnResume) -> String {
        let refusal = match on_resume {
            OnResume::Continue => String::new(),
            OnResume::Refuse => format!(
                "case \" $* \" in\n\
                 \x20 *\" --resume \"*)\n\
                 \x20   echo \"{REFUSAL}\" >&2\n\
                 \x20   exit 1\n\
                 \x20   ;;\n\
                 esac\n"
            ),
        };

        let mut cases = String::new();
        for turn in 1..=turns.len() {
            // The wildcard is the last turn's, so a conversation longer than the
            // script keeps answering with the last thing it had to say.
            cases.push_str(&match turn == turns.len() {
                true => "    *)\n".to_string(),
                false => format!("    {turn})\n"),
            });
            for (index, segment) in turns[turn - 1].iter().enumerate() {
                match segment.after {
                    Some(Stop::Pause) => cases.push_str("      sleep 1\n"),
                    // `read` fails at EOF, which is how "the server closed stdin
                    // instead of answering" arrives here. The script carries on
                    // either way — see [`AWAIT_ANSWER`].
                    Some(Stop::Answer) => cases.push_str(&format!(
                        "      if IFS= read -r answer; then\n\
                         \x20       printf '%s\\n' \"$answer\" >> \"$here/{ANSWERS_LOG}\"\n\
                         \x20     fi\n"
                    )),
                    None => {}
                }
                cases.push_str(&format!(
                    "      cat \"$here/{}\"\n",
                    segment_name(turn - 1, index)
                ));
            }
            cases.push_str("      ;;\n");
        }

        format!(
            "#!/bin/sh\n\
             if [ \"$1\" = \"--version\" ]; then\n\
             \x20 echo \"2.1.220 (Claude Code)\"\n\
             \x20 exit 0\n\
             fi\n\
             here=$(dirname \"$0\")\n\
             echo \"$*\" >> \"$here/{ARGUMENTS_LOG}\"\n\
             {refusal}\
             echo started >> \"$here/{STARTS_LOG}\"\n\
             turn=0\n\
             while IFS= read -r line; do\n\
             \x20 : > \"{WORKING_DIRECTORY_MARKER}\"\n\
             \x20 turn=$((turn + 1))\n\
             \x20 case \"$turn\" in\n\
             {cases}\
             \x20 esac\n\
             done\n"
        )
    }
}

fn segment_name(turn: usize, index: usize) -> String {
    format!("turn-{turn}-segment-{index}.ndjson")
}

/// One run of lines the agent prints without stopping, and what it does *before*
/// printing them.
///
/// The stop is attached to the segment that follows it rather than to the one
/// before, because that is the direction the script reads: reach this segment,
/// first do the thing, then print.
struct Segment<'a> {
    after: Option<Stop>,
    lines: &'a [&'a str],
}

/// The two kinds of stop a script can contain.
#[derive(Debug, Clone, Copy)]
enum Stop {
    Pause,
    Answer,
}

/// Split one turn's lines at the markers, keeping which marker each split was.
fn segments<'a>(lines: &'a [&'a str]) -> Vec<Segment<'a>> {
    let mut segments = Vec::new();
    let mut start = 0;
    let mut after = None;
    for (index, line) in lines.iter().enumerate() {
        let stop = match *line {
            PAUSE => Stop::Pause,
            AWAIT_ANSWER => Stop::Answer,
            _ => continue,
        };
        segments.push(Segment {
            after,
            lines: &lines[start..index],
        });
        after = Some(stop);
        start = index + 1;
    }
    segments.push(Segment {
        after,
        lines: &lines[start..],
    });
    segments
}

/// A recording, as a script.
///
/// A recording is a monologue with one exception: where the CLI asked permission
/// it stopped, and everything after that line is what happened *because of the
/// answer*. So the replay stops there too — emitting the rest without waiting
/// would have the agent react to a decision that had not been made.
fn replayable(recorded: &str) -> Vec<&str> {
    let mut lines: Vec<&str> = Vec::new();
    for line in recorded.lines() {
        lines.push(line);
        if asks_permission(line) {
            lines.push(AWAIT_ANSWER);
        }
    }
    lines
}

/// Is this recorded line the CLI asking permission?
///
/// Read rather than matched on as a string: `control_request` appears inside a
/// recorded `tool_result` in at least one capture, and a stop inserted there
/// would wait for an answer nobody was going to send.
fn asks_permission(line: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(line)
        .is_ok_and(|event| event["type"] == "control_request")
}

/// One line per process the agent script has started, beside the script itself.
/// Read by [`ScriptedAgent::starts`].
const STARTS_LOG: &str = "starts.log";

/// One line per process, holding the arguments it was given. Read by
/// [`ScriptedAgent::arguments`].
const ARGUMENTS_LOG: &str = "args.log";

/// One line per answer the server wrote at an [`AWAIT_ANSWER`]. Read by
/// [`ScriptedAgent::answers`].
const ANSWERS_LOG: &str = "answers.log";

/// What an agent says when it will not resume a conversation.
///
/// Close to the real CLI's wording, because the server quotes it into the
/// conversation verbatim and the point of quoting it is that the agent's own
/// words are more useful than anything the server could infer.
pub const REFUSAL: &str = "No conversation found with session ID.";
