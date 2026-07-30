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
    ///
    /// **Quoted for `sh`, bare for `cmd`.** A bare `(` in a `#!/bin/sh` script
    /// opens a subshell rather than printing, and dash refuses the line outright
    /// — `Syntax error: "(" unexpected`, exit status 2. Every test built on this
    /// fake was therefore exercising a *failing* binary on Linux while claiming
    /// to exercise a reporting one, which is a worse outcome than failing. `cmd`
    /// has no such rule and would print the quotes as text; the two other places
    /// in this file that write this string already escape it each way.
    pub fn reporting(version: &str) -> FakeAgent {
        FakeAgent::saying(&match cfg!(windows) {
            true => format!("echo {version} (Claude Code)"),
            false => format!("echo \"{version} (Claude Code)\""),
        })
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
/// The real CLI stops for the server at exactly one point — having asked
/// permission, it waits — and there is one more point where the *recording*
/// has to: the CLI's answer to something the server asked *it*. Both are the
/// same thing from a script's side, which is "do not go on until you have been
/// written to", so both use this marker and [`ScriptedAgent::replaying`] inserts
/// it at both.
///
/// **The line is not required.** If the server closes stdin instead of writing,
/// the script carries on with the rest of the recording rather than stopping,
/// which is what the CLI itself does: the permission stream closes, the tool
/// comes back as an abort, and the turn finishes. See
/// `fixtures/claude-cli/09-permission-unanswered.ndjson`, where that is the
/// recorded behaviour being replayed.
pub const AWAIT_ANSWER: &str = "<answer>";

/// A place where the agent stops until the server *asks* it something, and
/// records the question.
///
/// The mirror of [`AWAIT_ANSWER`] and the newer of the two: ticket 76 gave the
/// server a question of its own — how full is the context window — so a
/// recording can now contain a line the CLI said *because it was asked*, with
/// the asking absent because it travelled on stdin.
///
/// The difference between the two is what each one skips. A stop waiting for an
/// answer must not be satisfied by the server's question, and a stop waiting for
/// the question must not skip it. See [`SKIPPED`].
pub const AWAIT_QUESTION: &str = "<question>";

/// What a stand-in agent reads past rather than acting on.
///
/// **This is the whole of how the double demultiplexes its stdin.** The real CLI
/// reads the control channel and the turn channel out of one pipe and tells them
/// apart by parsing; a batch file cannot parse, so it matches substrings — the
/// subtypes of the requests this server sends on its own initiative.
///
/// Without it, the question the driver asks when a session announces itself is
/// read by the turn loop as a *turn*, and every scripted agent answers a prompt
/// nobody sent. That is not a hypothetical: adding the question broke a third of
/// the suite until this went in.
///
/// Narrow on purpose, and a list rather than a pattern for the same reason. A
/// double that skipped every `control_request` would go on passing when the
/// server started sending a *new* one, which is the opposite of what a test
/// double in a drift-detecting suite is for — so each subtype earns its entry
/// here, and the day a fourth appears the suite says so.
const SKIPPED: &[&str] = &["get_context_usage", "set_permission_mode", "set_model"];

/// How a batch script asks whether the line it just read is one of those,
/// spelled out because the two obvious ways are both wrong.
///
/// `echo %LINE% | findstr …` is wrong **for this content**: cmd decides where
/// the pipe is while the JSON's quotes are still in the line, mis-parses, and
/// prints the line to *stdout* — which is the agent's output, so the server reads
/// back the question it had just asked and counts it as an event it does not
/// recognise. It presented as two unrelated drift tests each gaining one.
///
/// Writing the line to a scratch file and running `findstr` over that is
/// correct but **not concurrency-safe here**: the scripts of one `ScriptedAgent`
/// share a directory, `%RANDOM%` is seeded from the clock, and two agent
/// processes started in the same tick draw the same "unique" name and race on
/// it. A lost line is a turn the agent never answers, so it presents as a hang —
/// `socket_concurrency.rs` went from two seconds to three tests timing out.
///
/// So: no file and no child process. The quotes are deleted from a copy, which
/// makes the value safe to put either side of an `if`, and the two copies differ
/// exactly when the needle was in it.
///
/// A `&` or a `>` in the line would still break this, and would equally break
/// the `echo %ANSWER%` that logs it — the script has never been able to carry
/// one, and nothing the server writes in a test contains one.
///
/// `log` is where the skipped line goes before the jump, for the one caller that
/// wants it: the requests the server makes on its own initiative are otherwise
/// invisible from outside the server, and a mode push is exactly the thing
/// ticket 11 needs a test to be able to see. The main loop logs; the wait for a
/// permission decision does not, because there the skip is incidental.
fn skips_the_servers_own_question(variable: &str, target: &str, log: Option<&str>) -> String {
    let mut skipping = format!("set PROBE=%{variable}:\"=%\r\n");
    for needle in SKIPPED {
        skipping.push_str(&format!("set REST=%PROBE:{needle}=%\r\n"));
        skipping.push_str(&match log {
            Some(log) => format!(
                "if not \"%PROBE%\"==\"%REST%\" (\r\n\
                 \x20 >>\"%~dp0{log}\" echo %{variable}%\r\n\
                 \x20 goto {target}\r\n\
                 )\r\n"
            ),
            None => format!("if not \"%PROBE%\"==\"%REST%\" goto {target}\r\n"),
        });
    }
    skipping
}

/// The same test, in `sh`, as one `case` pattern.
fn shell_skip_patterns() -> String {
    SKIPPED
        .iter()
        .map(|needle| format!("*{needle}*"))
        .collect::<Vec<_>>()
        .join("|")
}

/// A place where the agent *changes the project*, the way a real one does when
/// it edits a file.
///
/// Ticket 20's, and it is the only marker here that is not a stop: everything
/// else a script can do is talk, and a diff of a turn that only talked is empty
/// by definition. Built with [`writes`] and [`deletes`] rather than written
/// literally, because a marker carries a path and a file's contents.
///
/// The path is **relative**, so it lands wherever the agent was started — which
/// is the project's own folder, for the same reason
/// [`WORKING_DIRECTORY_MARKER`] does. Directories are made as needed.
const WRITES: &str = "<writes>";
const DELETES: &str = "<deletes>";

/// The field separator inside a [`WRITES`] marker. A control character, so that
/// nothing a test would put in a path or a file collides with it.
const FIELD: char = '\u{1}';

/// A script line that has the agent create or overwrite a file mid-turn.
pub fn writes(path: &str, contents: &str) -> String {
    format!("{WRITES}{path}{FIELD}{contents}")
}

/// A script line that has the agent delete a file mid-turn.
pub fn deletes(path: &str) -> String {
    format!("{DELETES}{path}")
}

/// A place where the agent stops being a process at all: it complains on stderr
/// and exits, in the middle of whatever it was saying.
///
/// The one failure mode with no recording and no possible one — a CLI that
/// crashes has, by definition, not finished writing the capture. Everything
/// after this marker in a turn's script is never printed, which is the point:
/// what the server sees is output that stops, and the turn it was in the middle
/// of will never end on its own.
pub const DIES: &str = "<dies>";

/// What a scripted agent says on stderr on its way out.
///
/// Real, in shape: a `claude` that dies mid-turn has said something about why,
/// and the server quotes it into the conversation for the same reason it quotes
/// a refused resume — the agent's own words beat anything this server could
/// infer.
pub const LAST_WORDS: &str = "FATAL ERROR: the agent went away";

/// A file every scripted agent writes into whatever directory it was started
/// in, on every turn.
///
/// The only honest way to observe "the agent runs with the project directory as
/// its working directory", which is a criterion of ticket 10. The alternative —
/// having the agent print its own path in its output — would need the path JSON
/// escaped by a batch file, and a Windows path is mostly backslashes.
pub const WORKING_DIRECTORY_MARKER: &str = "laplus-agent-was-here";

/// What a scripted agent does when it is asked to continue a conversation.
enum OnResume {
    /// What a healthy CLI does: resume, announce a session, take the turn.
    Continue,
    /// What one whose stored conversation has gone does: complain on stderr and
    /// exit without a line of NDJSON. Ticket 11's "the underlying agent session
    /// is no longer available", and no recording contains it.
    Refuse,
    /// The same as [`OnResume::Continue`], except that the scripts belong to the
    /// **conversation** rather than to the process: a start carrying `--resume`
    /// begins at the second script, because a previous process already played
    /// the first.
    ///
    /// What ticket 15's restart needs, and the only thing that can express it.
    /// A process that died halfway through turn one is replaced by one that has
    /// to answer turn two — and the replacement's own turn counter starts at
    /// zero, so without this it would replay the death.
    PickUpWhereTheLastOneStopped,
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

    /// An agent whose scripts belong to the conversation rather than to the
    /// process: a start carrying `--resume` skips the first script, because a
    /// previous process already played it.
    ///
    /// For the one thing a per-process counter cannot express — a session that
    /// died in the middle of turn one and a *replacement* that has to answer
    /// turn two. See [`OnResume::PickUpWhereTheLastOneStopped`].
    pub fn resuming_after_a_death<'a>(turns: &[impl AsRef<[&'a str]>]) -> ScriptedAgent {
        ScriptedAgent::written(turns, OnResume::PickUpWhereTheLastOneStopped)
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
                // What the agent will copy into the project when it reaches this
                // segment. Beside the script and named after the segment, so the
                // script needs nothing but `%~dp0` to find it.
                if let Some(Stop::Write { contents, .. }) = &segment.after {
                    std::fs::write(
                        agent.directory.path().join(edit_name(turn, index)),
                        contents,
                    )
                    .expect("writes an edit's contents");
                }
            }
        }

        std::fs::write(
            agent.directory.path().join(HANDSHAKE_ANSWER),
            format!("{HANDSHAKE_COMMANDS}\n"),
        )
        .expect("writes the handshake answer");

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

    /// Every request the server made of this agent *on its own initiative*, in
    /// order — the lines the turn loop reads past rather than taking for a turn.
    ///
    /// The mirror of [`ScriptedAgent::answers`], and the only sight a test gets
    /// of the control channel this server uses to move a live child: a runtime
    /// mode or a model changed mid-conversation reaches the agent here and
    /// nowhere else, so without this a server that published the change and sent
    /// nothing would pass every other assertion in the suite. Ticket 11's.
    pub fn requests(&self) -> Vec<String> {
        self.logged(REQUESTS_LOG)
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
            OnResume::Continue | OnResume::PickUpWhereTheLastOneStopped => String::new(),
            OnResume::Refuse => format!(
                "echo %* | findstr /C:\"--resume\" >nul\r\n\
                 if not errorlevel 1 (\r\n\
                 \x20 >&2 echo {REFUSAL}\r\n\
                 \x20 exit /b 1\r\n\
                 )\r\n"
            ),
        };

        // Where the turn counter starts. Zero for a process that is the whole
        // conversation; one for a replacement that is picking up after a turn
        // somebody else already played.
        let first_turn = match on_resume {
            OnResume::PickUpWhereTheLastOneStopped => {
                "echo %* | findstr /C:\"--resume\" >nul\r\nif not errorlevel 1 set TURN=1\r\n"
                    .to_string()
            }
            OnResume::Continue | OnResume::Refuse => String::new(),
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
                match &segment.after {
                    // A second, in the platform's idiom for sleeping without a
                    // console: `timeout` needs one, and `powershell -c
                    // Start-Sleep` costs more to start than it sleeps for.
                    Some(Stop::Pause) => bodies.push_str("ping -n 2 127.0.0.1 >nul\r\n"),
                    // `set /p` leaves the variable undefined at EOF, which is how
                    // "the server closed stdin instead of answering" arrives here.
                    // The script carries on either way — see [`AWAIT_ANSWER`].
                    //
                    // The loop is what makes this a wait for an *answer*: the
                    // server's own question may be sitting in the pipe ahead of
                    // it, and reading that as the decision would leave the agent
                    // acting on a line nobody meant as one. See [`SKIPPED`].
                    Some(Stop::Answer) => {
                        let again = format!("answer-{}-{index}", turn - 1);
                        bodies.push_str(&format!(
                            ":{again}\r\n\
                             set \"ANSWER=\"\r\n\
                             set /p ANSWER=\r\n\
                             if not defined ANSWER goto {again}-done\r\n\
                             {skip}\
                             >>\"%~dp0{ANSWERS_LOG}\" echo %ANSWER%\r\n\
                             :{again}-done\r\n",
                            skip = skips_the_servers_own_question("ANSWER", &again, None),
                        ))
                    }
                    // The mirror, and the one place the skipped line is the line
                    // being waited for.
                    Some(Stop::Question) => bodies.push_str(&format!(
                        "set \"ASKED=\"\r\n\
                         set /p ASKED=\r\n\
                         if defined ASKED >>\"%~dp0{ANSWERS_LOG}\" echo %ASKED%\r\n"
                    )),
                    // `exit` rather than `exit /b`: this is inside a `call`, and
                    // `/b` would return from the subroutine and carry on reading
                    // stdin — which is a turn that went quiet, not an agent that
                    // died.
                    Some(Stop::Die) => {
                        bodies.push_str(&format!(">&2 echo {LAST_WORDS}\r\nexit 3\r\n"))
                    }
                    // Relative, so it lands in the project the agent was started
                    // in. `type` copies the bytes the harness wrote, which is
                    // the only way to get an arbitrary string past `echo`.
                    Some(Stop::Write { path, .. }) => {
                        if let Some(parent) = parent_of(path) {
                            bodies.push_str(&format!(
                                "if not exist \"{parent}\\\" mkdir \"{parent}\"\r\n"
                            ));
                        }
                        bodies.push_str(&format!(
                            ">\"{}\" type \"%~dp0{}\"\r\n",
                            native(path),
                            edit_name(turn - 1, index)
                        ));
                    }
                    Some(Stop::Delete { path }) => {
                        let path = native(path);
                        bodies
                            .push_str(&format!("if exist \"{path}\" del /f /q \"{path}\"\r\n"));
                    }
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
             rem A catalogue probe rather than a session: it asks what commands\r\n\
             rem this agent knows and never sends a turn. Told apart by the flag\r\n\
             rem only a driven session is given — `crate::catalogue` deliberately\r\n\
             rem passes no permission prompt tool, having nothing to ask about.\r\n\
             rem Answered before the start is counted, because this is not one:\r\n\
             rem a test asserting how many sessions ran must not see probes.\r\n\
             echo %* | findstr /C:\"--permission-prompt-tool\" >nul\r\n\
             if errorlevel 1 (\r\n\
             \x20 type \"%~dp0{HANDSHAKE_ANSWER}\"\r\n\
             \x20 exit /b 0\r\n\
             )\r\n\
             {refusal}\
             rem One line per process, beside the script rather than in the\r\n\
             rem project — this counts starts, and the project is where the\r\n\
             rem working-directory marker goes.\r\n\
             >>\"%~dp0{STARTS_LOG}\" echo started\r\n\
             set TURN=0\r\n\
             {first_turn}\
             :turns\r\n\
             rem `set /p` leaves the variable undefined when stdin has\r\n\
             rem closed, which is how this loop hears that the server is\r\n\
             rem finished with it.\r\n\
             set \"LINE=\"\r\n\
             set /p LINE=\r\n\
             if not defined LINE exit /b 0\r\n\
             rem Not every line the server writes is a turn. It asks how full\r\n\
             rem the context window is whenever a session announces itself,\r\n\
             rem and counting that as a prompt would answer a turn nobody\r\n\
             rem sent — see [`skips_the_servers_own_question`].\r\n\
             {skip}\
             rem A relative path, so it lands wherever the agent was started —\r\n\
             rem which is the whole point of writing it.\r\n\
             echo.>\"{WORKING_DIRECTORY_MARKER}\"\r\n\
             set /a TURN+=1\r\n\
             {dispatch}\
             goto turns\r\n\
             {bodies}",
            skip = skips_the_servers_own_question("LINE", "turns", Some(REQUESTS_LOG)),
        )
    }

    fn shell(&self, turns: &[Vec<Segment<'_>>], on_resume: OnResume) -> String {
        let refusal = match on_resume {
            OnResume::Continue | OnResume::PickUpWhereTheLastOneStopped => String::new(),
            OnResume::Refuse => format!(
                "case \" $* \" in\n\
                 \x20 *\" --resume \"*)\n\
                 \x20   echo \"{REFUSAL}\" >&2\n\
                 \x20   exit 1\n\
                 \x20   ;;\n\
                 esac\n"
            ),
        };

        let first_turn = match on_resume {
            OnResume::PickUpWhereTheLastOneStopped => {
                "case \" $* \" in\n  *\" --resume \"*) turn=1 ;;\nesac\n".to_string()
            }
            OnResume::Continue | OnResume::Refuse => String::new(),
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
                match &segment.after {
                    Some(Stop::Pause) => cases.push_str("      sleep 1\n"),
                    // `read` fails at EOF, which is how "the server closed stdin
                    // instead of answering" arrives here. The script carries on
                    // either way — see [`AWAIT_ANSWER`]. The server's own
                    // question is read past rather than taken for the decision;
                    // see [`SKIPPED`].
                    Some(Stop::Answer) => cases.push_str(&format!(
                        "      while IFS= read -r answer; do\n\
                         \x20       case \"$answer\" in {skipped}) continue ;; esac\n\
                         \x20       printf '%s\\n' \"$answer\" >> \"$here/{ANSWERS_LOG}\"\n\
                         \x20       break\n\
                         \x20     done\n",
                        skipped = shell_skip_patterns(),
                    )),
                    // The mirror, and the one place the skipped line is the line
                    // being waited for.
                    Some(Stop::Question) => cases.push_str(&format!(
                        "      if IFS= read -r asked; then\n\
                         \x20       printf '%s\\n' \"$asked\" >> \"$here/{ANSWERS_LOG}\"\n\
                         \x20     fi\n"
                    )),
                    Some(Stop::Die) => {
                        cases.push_str(&format!("      echo \"{LAST_WORDS}\" >&2\n      exit 3\n"))
                    }
                    Some(Stop::Write { path, .. }) => {
                        if let Some(parent) = parent_of(path) {
                            cases.push_str(&format!("      mkdir -p \"{parent}\"\n"));
                        }
                        cases.push_str(&format!(
                            "      cat \"$here/{}\" > \"{path}\"\n",
                            edit_name(turn - 1, index)
                        ));
                    }
                    Some(Stop::Delete { path }) => {
                        cases.push_str(&format!("      rm -f \"{path}\"\n"));
                    }
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
             # A catalogue probe rather than a session — see the batch script.\n\
             case \" $* \" in\n\
             \x20 *\" --permission-prompt-tool \"*) ;;\n\
             \x20 *)\n\
             \x20   cat \"$here/{HANDSHAKE_ANSWER}\"\n\
             \x20   exit 0\n\
             \x20   ;;\n\
             esac\n\
             {refusal}\
             echo started >> \"$here/{STARTS_LOG}\"\n\
             turn=0\n\
             {first_turn}\
             while IFS= read -r line; do\n\
             \x20 # Not every line the server writes is a turn — see [`SKIPPED`].\n\
             \x20 # The ones that are not are logged, because they are the only\n\
             \x20 # sight a test gets of what the server asked of a live agent.\n\
             \x20 case \"$line\" in\n\
             \x20   {skipped})\n\
             \x20     printf '%s\\n' \"$line\" >> \"$here/{REQUESTS_LOG}\"\n\
             \x20     continue\n\
             \x20     ;;\n\
             \x20 esac\n\
             \x20 : > \"{WORKING_DIRECTORY_MARKER}\"\n\
             \x20 turn=$((turn + 1))\n\
             \x20 case \"$turn\" in\n\
             {cases}\
             \x20 esac\n\
             done\n",
            skipped = shell_skip_patterns(),
        )
    }
}

fn segment_name(turn: usize, index: usize) -> String {
    format!("turn-{turn}-segment-{index}.ndjson")
}

fn edit_name(turn: usize, index: usize) -> String {
    format!("turn-{turn}-edit-{index}.txt")
}

/// A relative path as this platform's shell wants it spelled.
///
/// `cmd`'s own builtins — `mkdir`, `del` — reject forward slashes even though
/// every Win32 call underneath them accepts both, so a path written the way a
/// test would write it has to be turned round before it reaches one.
fn native(path: &str) -> String {
    match cfg!(windows) {
        true => path.replace('/', "\\"),
        false => path.to_string(),
    }
}

/// The directory part of a relative path, if it has one.
fn parent_of(path: &str) -> Option<String> {
    let native = native(path);
    let separator = match cfg!(windows) {
        true => '\\',
        false => '/',
    };
    native
        .rfind(separator)
        .map(|at| native[..at].to_string())
        .filter(|parent| !parent.is_empty())
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

/// The kinds of interruption a script can contain. `Die` is not one the script
/// comes back from, and the last two are not stops at all — they are the agent
/// doing something to the project rather than waiting.
#[derive(Debug, Clone)]
enum Stop {
    Pause,
    Answer,
    /// Wait for the server to ask something. See [`AWAIT_QUESTION`].
    Question,
    Die,
    /// Create or overwrite a file. The contents travel to the agent the same way
    /// its lines do — in a file beside the script, which the script copies —
    /// because a batch file cannot be trusted to `echo` an arbitrary string
    /// back unchanged. See [`WRITES`].
    Write { path: String, contents: String },
    Delete { path: String },
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
            AWAIT_QUESTION => Stop::Question,
            DIES => Stop::Die,
            edit => match edit.strip_prefix(WRITES) {
                Some(rest) => {
                    let (path, contents) = rest.split_once(FIELD).unwrap_or((rest, ""));
                    Stop::Write {
                        path: path.to_string(),
                        contents: contents.to_string(),
                    }
                }
                None => match edit.strip_prefix(DELETES) {
                    Some(path) => Stop::Delete {
                        path: path.to_string(),
                    },
                    None => continue,
                },
            },
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
/// A recording is a monologue with two exceptions, and both are places where the
/// server's own line is missing from it:
///
/// - **Where the CLI asked permission it stopped**, and everything after that
///   line is what happened *because of the answer*. So the replay stops after
///   it — emitting the rest without waiting would have the agent react to a
///   decision that had not been made.
/// - **Where the CLI acknowledged something the server asked it**, the request
///   came first and is not in the recording, because it travelled on stdin. So
///   the replay stops *before* it. Playing an interrupt's acknowledgement
///   straight through would have the agent answer a request nobody had sent, and
///   then abort a turn nobody had stopped.
///
/// The asymmetry is the direction of the missing line: after a question the
/// server has to answer, before an answer the server had to ask for.
///
/// The second case splits in two, because the server now asks two different
/// things and the stops differ in what they read past. A stop before a *context
/// reading* is waiting for the question that produced it; a stop before an
/// interrupt's acknowledgement is waiting for the interrupt, and must not be
/// satisfied by a context question that happens to be in the pipe already — the
/// driver asks one when the session announces itself, long before any turn is
/// stopped. See [`SKIPPED`].
fn replayable(recorded: &str) -> Vec<&str> {
    let mut lines: Vec<&str> = Vec::new();
    for line in recorded.lines() {
        match exchange(line) {
            Some(Exchange::Asks) => {
                lines.push(line);
                lines.push(AWAIT_ANSWER);
            }
            Some(Exchange::Answers { asked }) => {
                lines.push(match asked {
                    Asked::HowFullTheWindowIs => AWAIT_QUESTION,
                    Asked::SomethingElse => AWAIT_ANSWER,
                });
                lines.push(line);
            }
            None => lines.push(line),
        }
    }
    lines
}

/// A recorded line that is half of a conversation rather than a statement, and
/// which half.
enum Exchange {
    /// The CLI asking the server something. The server's line comes next.
    Asks,
    /// The CLI answering the server. The server's line came first, and is not in
    /// the recording.
    Answers { asked: Asked },
}

/// Which of the server's questions a recorded answer is answering.
///
/// Read off what the answer *carries* rather than off the id it names, the same
/// way `crate::protocol::Acknowledgement::reading` does: a reading has a window
/// in it and an interrupt's `{"still_queued": []}` does not.
enum Asked {
    HowFullTheWindowIs,
    SomethingElse,
}

/// Which half of an exchange this line is, if it is one.
///
/// Read rather than matched on as a string: `control_request` appears inside a
/// recorded `tool_result` in at least one capture, and a stop inserted there
/// would wait for a line nobody was going to send.
fn exchange(line: &str) -> Option<Exchange> {
    let event: serde_json::Value = serde_json::from_str(line).ok()?;
    match event["type"].as_str()? {
        "control_request" => Some(Exchange::Asks),
        "control_response" => Some(Exchange::Answers {
            asked: match event["response"]["response"]["totalTokens"].is_number() {
                true => Asked::HowFullTheWindowIs,
                false => Asked::SomethingElse,
            },
        }),
        _ => None,
    }
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

/// One line per request the server made on its own initiative and the turn loop
/// therefore read past. Read by [`ScriptedAgent::requests`].
const REQUESTS_LOG: &str = "requests.log";

/// The canned answer to the `initialize` control request, beside the script.
const HANDSHAKE_ANSWER: &str = "handshake.ndjson";

/// The commands this stand-in claims to know, and what it answers the handshake
/// with.
///
/// `crate::catalogue` opens a session of its own and asks it to introduce
/// itself, which is a thing the real CLI does and so a thing this has to do —
/// a stand-in that ignored the request would leave every provider refresh
/// waiting out the probe's full patience, and would make the `/` menu
/// untestable.
///
/// The three are chosen to cover what the payload can carry rather than to
/// imitate a real install: one with a description and an argument hint, one with
/// a description alone, and one with neither. `clear` is in it because it is the
/// built-in that started all this.
pub const HANDSHAKE_COMMANDS: &str = concat!(
    r#"{"type":"control_response","response":{"subtype":"success","request_id":"#,
    r#""laplus-initialize","response":{"commands":["#,
    r#"{"name":"clear","description":"Clear conversation history","argumentHint":""},"#,
    r#"{"name":"compact","description":"Compact the conversation","argumentHint":"instructions"},"#,
    r#"{"name":"context"}"#,
    r#"]}}}"#,
);

/// What an agent says when it will not resume a conversation.
///
/// Close to the real CLI's wording, because the server quotes it into the
/// conversation verbatim and the point of quoting it is that the agent's own
/// words are more useful than anything the server could infer.
pub const REFUSAL: &str = "No conversation found with session ID.";
