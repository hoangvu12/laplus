//! The `claude` subprocess: how it is started, driven and reaped.
//!
//! One child per session, alive across turns, spoken to in newline-delimited
//! JSON on stdin and read the same way on stdout. That shape is settled — the
//! STEP 1 spike proved it and `spike-claude-protocol/README.md` records the
//! flags — so nothing here is exploratory; what this module owns is the I/O the
//! spike did by hand.
//!
//! Deliberately ignorant of what it is carrying. Lines go out, lines come back,
//! and [`crate::protocol`] is the only thing that knows what they mean. That is
//! the same split the crate keeps everywhere: a format change has a blast radius
//! of one pure file, and the process handling is not in it.
//!
//! ## Why the output is read by a task rather than by whoever wants it
//!
//! A child that is writing has to be read, or it blocks on a pipe nobody is
//! draining, and it will do that whether or not the server is currently
//! interested. So both output streams get a reader from the moment the child
//! exists: stdout into a bounded channel the session drains, stderr straight to
//! the server's own stderr with a prefix.
//!
//! The bound on the stdout channel is real back-pressure and is the correct
//! behaviour rather than a compromise. If the session's consumer stalls, the
//! reader stops reading, the pipe fills, and the agent slows down — which is
//! what should happen. An unbounded channel would instead let a wedged consumer
//! turn a long turn into unbounded memory.
//!
//! ## Termination is two steps, and the first one usually suffices
//!
//! Closing stdin is how this protocol says "no more turns": the CLI reads to EOF
//! and exits on its own, flushing whatever it still owed. So [`Agent::stop`]
//! drops the writer first and only kills if the child has not gone by the
//! deadline. Killing first would be simpler and would lose the tail of the
//! output for no reason.
//!
//! `kill_on_drop` is set as well. It is not the mechanism — [`Agent::stop`] is —
//! but a panic unwinding past an `Agent` must not leave a `claude` running, and
//! that is a path no amount of care in the happy case covers.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::mpsc;

/// How many NDJSON lines may be waiting for the session before the reader stops
/// reading and the agent feels it.
///
/// Sized for a burst of token deltas rather than for a whole turn: the session
/// drains this in a tight loop, so it only ever has to absorb the difference
/// between the agent's rate and the fold's, and the fold is arithmetic on a
/// string.
const OUTPUT_QUEUE: usize = 256;

/// How long a closed stdin has to end the child before it is killed.
///
/// Not a performance budget. The CLI exits promptly on EOF; this is the guard
/// against the case where it does not, and the cost of it being generous is a
/// shutdown that takes a moment longer.
const EXIT_GRACE: Duration = Duration::from_secs(2);

/// How often a stopping agent looks to see whether the child has gone.
const EXIT_POLL: Duration = Duration::from_millis(20);

/// Everything that varies between one agent and the next.
///
/// A struct rather than four arguments because every field is a string and three
/// of them are optional, which is exactly the shape a positional call gets wrong
/// silently.
#[derive(Debug, Clone)]
pub struct Launch {
    /// The resolved binary, from [`crate::provider::resolve`]. An absolute path
    /// rather than a name: what to start is a question this module is answered
    /// rather than one it asks.
    pub binary: PathBuf,
    /// The project's workspace root. The agent's working directory *is* the
    /// project — it is what makes a relative path in the transcript mean what
    /// the developer thinks it means.
    pub cwd: String,
    /// The `--model` slug, from the thread's model selection.
    pub model: Option<String>,
    /// The CLI's own permission-mode literal, from
    /// [`permission_mode_for`].
    pub permission_mode: Option<&'static str>,
    /// The `claude` session to continue, when this conversation has one.
    ///
    /// `Some` is what makes a conversation survive a restart, and it is the whole
    /// of the mechanism: the context is in the agent's own store, so continuity
    /// is a flag rather than a replay of the transcript into the prompt. The id
    /// is the agent's own, read off a previous run's `init` line — see
    /// [`crate::threads::Thread::agent_session_id`].
    pub resume: Option<String>,
}

/// The CLI's `--permission-mode` for a thread's runtime mode.
///
/// Upstream's table (`ClaudeAdapter.ts:3510`) verbatim, including its one
/// omission: `approval-required` maps to nothing, because upstream expresses it
/// by *not* passing the flag and answering the CLI's permission callback
/// instead. lightcode has no such callback until ticket 13, so this server sends
/// no flag for it either and the CLI's own default applies — which is right for
/// a turn that uses no tools, and is ticket 13's to make right for one that does.
pub fn permission_mode_for(runtime_mode: &str) -> Option<&'static str> {
    match runtime_mode {
        "auto-accept-edits" => Some("acceptEdits"),
        "auto" => Some("auto"),
        "full-access" => Some("bypassPermissions"),
        _ => None,
    }
}

/// A running `claude`, and the two ends of its conversation.
#[derive(Debug)]
pub struct Agent {
    child: Child,
    /// `None` once stdin has been closed, which is how a session says there will
    /// be no more turns. The writer is *taken* rather than flagged, so closing
    /// it drops the handle — which is what the child reads as EOF — instead of
    /// leaving an open pipe beside a boolean saying it is shut.
    stdin: Option<ChildStdin>,
    output: mpsc::Receiver<String>,
    /// The last thing the agent said on stderr.
    ///
    /// Kept because there is one failure whose only account of itself is here: a
    /// `--resume` the CLI will not honour writes its reason to stderr and exits,
    /// producing no NDJSON at all. Without this, the server could only report
    /// that the agent said nothing — see [`crate::turn`], which turns it into a
    /// sentence in the conversation.
    ///
    /// One line rather than a log: the point is to have the CLI's own words for
    /// a specific failure, not to become a second place the agent's output lives.
    complaint: Arc<Mutex<Option<String>>>,
    /// The task reading stderr, so [`Agent::stop`] can wait for it.
    ///
    /// Without the wait, "the last thing the agent said" would mean "whatever had
    /// arrived by the time somebody asked": stdout and stderr are drained by
    /// separate tasks, so the end of the output and the writing of the reason for
    /// it are not ordered. That is precisely the case
    /// [`Agent::complaint`] exists for.
    stderr: Option<tokio::task::JoinHandle<()>>,
}

impl Agent {
    /// Start the agent. Returns as soon as the child exists — nothing is read
    /// and nothing is waited for, because the first thing this server owes the
    /// developer is an acknowledgement rather than an answer.
    pub async fn start(launch: &Launch) -> std::io::Result<Agent> {
        let mut command = Command::new(&launch.binary);
        command
            // The flags that constitute the protocol, from the spike's write-up.
            // `--verbose` is required alongside `--print` for stream-json output,
            // and `--include-partial-messages` is what yields the token-level
            // deltas without which the UI is dead for the length of a turn.
            .arg("--print")
            .arg("--input-format")
            .arg("stream-json")
            .arg("--output-format")
            .arg("stream-json")
            .arg("--include-partial-messages")
            .arg("--verbose")
            .current_dir(&launch.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // A backstop, not the mechanism — see the module documentation.
            .kill_on_drop(true);

        if let Some(model) = &launch.model {
            command.arg("--model").arg(model);
        }
        if let Some(mode) = launch.permission_mode {
            command.arg("--permission-mode").arg(mode);
        }
        // Continuity, in one flag. The CLI keeps the conversation itself, so this
        // is what makes a follow-up after a restart a follow-up rather than a
        // first message — and it is the reason this server does not replay its
        // transcript into the prompt.
        if let Some(session) = &launch.resume {
            command.arg("--resume").arg(session);
        }
        crate::process::without_a_console(command.as_std_mut());

        let mut child = command.spawn()?;

        // `take` rather than `expect` on each: all three were piped a few lines
        // above, so their absence is this function's own bug and not something a
        // running server should discover.
        let stdin = child.stdin.take().ok_or_else(missing_pipe)?;
        let stdout = child.stdout.take().ok_or_else(missing_pipe)?;
        let stderr = child.stderr.take().ok_or_else(missing_pipe)?;

        let (lines, output) = mpsc::channel(OUTPUT_QUEUE);
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            // `next_line` splits on `\n` and strips a trailing `\r`, which is
            // what a CRLF-writing child on Windows needs. A read error ends the
            // loop the same way EOF does: either way there is no more output,
            // and the session hears about it when the channel closes.
            while let Ok(Some(line)) = reader.next_line().await {
                if lines.send(line).await.is_err() {
                    // Nobody is listening any more, which means the session has
                    // gone. Stopping here lets the pipe fill and the child feel
                    // it, rather than reading a dead turn to completion.
                    return;
                }
            }
        });

        // Still not a channel: nothing in this server *acts* on the agent's
        // stderr, and giving it one would be inventing a consumer. It goes to the
        // developer's terminal, prefixed, because a line with no prefix reads as
        // lightcode's own — and the most recent one is kept, for the one failure
        // whose only account of itself is there. See [`Agent::complaint`].
        let complaint = Arc::new(Mutex::new(None));
        let latest = Arc::clone(&complaint);
        let reading_stderr = tokio::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                eprintln!("lightcode: claude: {line}");
                if !line.trim().is_empty() {
                    *lock(&latest) = Some(line.trim().to_string());
                }
            }
        });

        Ok(Agent {
            child,
            stdin: Some(stdin),
            output,
            complaint,
            stderr: Some(reading_stderr),
        })
    }

    /// Send one user turn.
    ///
    /// The whole of the outbound protocol: one JSON object on one line. Flushed
    /// rather than left to the buffer, because the agent is waiting for it and
    /// a prompt that sat in a write buffer would look exactly like a hang.
    pub async fn send(&mut self, text: &str) -> std::io::Result<()> {
        let stdin = self.stdin.as_mut().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "the agent's input has already been closed",
            )
        })?;
        let mut line = crate::protocol::user_message_line(text);
        line.push('\n');
        stdin.write_all(line.as_bytes()).await?;
        stdin.flush().await
    }

    /// Say there will be no more turns, without waiting for the agent to
    /// finish the one it is on.
    ///
    /// The half of [`Agent::stop`] that a driver wants on its own: closing stdin
    /// is what makes the agent's output end, and a driver that is still draining
    /// that output cannot give the agent up yet.
    pub fn close_input(&mut self) {
        drop(self.stdin.take());
    }

    /// The next line the agent produced, or `None` once it has stopped
    /// producing — which is either the child exiting or the reader giving up on
    /// a consumer that had gone.
    pub async fn next_line(&mut self) -> Option<String> {
        self.output.recv().await
    }

    /// Close stdin, wait, kill if waiting was not enough, and hand back the last
    /// thing the agent said on stderr.
    ///
    /// Always ends with the child reaped: every branch either waits or kills and
    /// then waits, so there is no path out of here that leaves a zombie.
    ///
    /// The complaint comes back from *here* rather than being readable at any
    /// moment, and that is the point. stdout and stderr are drained by separate
    /// tasks, so "the agent's output ended" and "the agent wrote why" are not
    /// ordered against each other — a caller that asked the moment the output
    /// ended would get whatever had happened to arrive. Once the child has gone
    /// its stderr is closed, the reader is finishing, and joining it is what makes
    /// the answer final. That matters for exactly one failure, and it is the one
    /// with no NDJSON to it: a `--resume` the CLI will not honour.
    pub async fn stop(mut self) -> Option<String> {
        // EOF. The CLI's own way of being told the conversation is over.
        drop(self.stdin.take());

        let deadline = tokio::time::Instant::now() + EXIT_GRACE;
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => return self.last_words().await,
                Err(error) => {
                    eprintln!("lightcode: cannot wait for the agent: {error}");
                    break;
                }
                Ok(None) if tokio::time::Instant::now() >= deadline => break,
                Ok(None) => tokio::time::sleep(EXIT_POLL).await,
            }
        }

        // On Windows this kills the process this server started. That is the
        // real `claude.exe` in production — `crate::provider` resolves a native
        // binary — so the kill lands where it says it does. A `.cmd` shim, which
        // is what the suite's stand-ins are, is started through `cmd.exe` and
        // would leave a grandchild for the OS to reap at exit; the same
        // reasoning as `provider::probe`'s timeout, and the same conclusion,
        // which is that a job object is not worth it here.
        let _ = self.child.kill().await;
        self.last_words().await
    }

    /// Let the stderr reader finish, then take what it last saw.
    ///
    /// Bounded, because the reader ends when the pipe closes and the pipe closes
    /// when the last holder of it exits — which for the `.cmd` stand-ins the suite
    /// uses is a grandchild a kill does not reach. The same case, and the same
    /// bound, as the exit grace above.
    async fn last_words(&mut self) -> Option<String> {
        if let Some(reader) = self.stderr.take() {
            let _ = tokio::time::timeout(EXIT_GRACE, reader).await;
        }
        lock(&self.complaint).clone()
    }
}

fn missing_pipe() -> std::io::Error {
    std::io::Error::other("the agent was started without one of its pipes")
}

/// A poisoned lock means the stderr reader panicked mid-line. What is behind it
/// is one `Option<String>` with no invariant a panic could have broken, so
/// refusing to use it would turn one panic into a session that cannot report why
/// it failed.
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The permission-mode table, both halves. The mapped three are what the
    /// CLI is actually told; the unmapped one is a deliberate omission, so it is
    /// pinned rather than left to look like a gap.
    #[test]
    fn a_runtime_mode_becomes_the_permission_mode_the_cli_knows() {
        assert_eq!(permission_mode_for("auto-accept-edits"), Some("acceptEdits"));
        assert_eq!(permission_mode_for("auto"), Some("auto"));
        assert_eq!(
            permission_mode_for("full-access"),
            Some("bypassPermissions")
        );
        assert_eq!(
            permission_mode_for("approval-required"),
            None,
            "upstream expresses this by omitting the flag, and so does this"
        );
        assert_eq!(permission_mode_for("something-later"), None);
    }

    /// A binary that is not there fails at the spawn rather than hanging or
    /// panicking, so the turn can report it as a session error.
    #[tokio::test]
    async fn an_agent_that_cannot_be_started_says_so() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let missing = directory.path().join("not-here");

        let failure = Agent::start(&Launch {
            binary: missing,
            cwd: directory.path().to_string_lossy().into_owned(),
            model: None,
            permission_mode: None,
            resume: None,
        })
        .await
        .expect_err("nothing is there to start");

        assert!(
            !failure.to_string().is_empty(),
            "the failure has to carry something a developer can read"
        );
    }
}
