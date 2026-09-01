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
//! ## Three kinds of line go out, and each is a different kind of thing to say
//!
//! [`Agent::send`] is a turn. [`Agent::answer`] is a permission decision, and it
//! is different in kind rather than in content: the agent has *stopped* until it
//! arrives. The third kind is a **request**: it carries an id and the CLI answers
//! it on stdout, and there are four — [`Agent::interrupt`],
//! [`Agent::measure_context`], [`Agent::set_permission_mode`] and
//! [`Agent::set_model`]. Every one is one line and every one is flushed, and the
//! reason for flushing is the same in each case and merely more urgent in the
//! ones something is waiting on.
//!
//! The last two are the only lines here that change what the child *is* rather
//! than what it is doing. They exist because `--permission-mode` and `--model`
//! are read once, at launch, so a developer who changed either mid-conversation
//! was answered by a process still running under the old one — ticket 11 of
//! `thread-lifecycle`. The alternative was replacing the session, which costs a
//! fresh `init` and the CLI's warm context window; a push costs a line.
//!
//! What makes an answer possible at all is [`PERMISSION_PROMPT_TOOL`], which is
//! passed on every session and tells the CLI to ask this stdio pair rather than
//! an MCP server. An interrupt needs no flag: `--input-format stream-json` is
//! itself a control channel, and the CLI reads a `control_request` on it
//! whether or not it is currently mid-turn.
//!
//! ## Interrupting is not stopping
//!
//! The two are deliberately different operations on this type, because they are
//! different things to want. [`Agent::stop`] ends the *session* — it closes
//! stdin and reaps the child, and the conversation is over.
//! [`Agent::interrupt`] ends the *turn* and leaves the child running, which is
//! what makes a correction sent a moment later a correction rather than the
//! first message of a new conversation.
//!
//! `fixtures/claude-cli/12-interrupt-then-continue.ndjson` is the recording that
//! settles it: an interrupt, an aborted `result`, and then a second turn
//! answered normally by the same process.
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
//! It is also what unwedges an agent stopped on a permission nobody answered.
//! Closing stdin closes the permission stream with it, and the CLI abandons the
//! request rather than waiting on it — the tool comes back as
//! `Tool permission request failed: AbortError: Tool permission stream closed`,
//! the turn finishes, and the child exits on its own.
//! `fixtures/claude-cli/09-permission-unanswered.ndjson` is that, recorded. So an
//! unanswered request costs a tool call and never a process.
//!
//! `kill_on_drop` is set as well. It is not the mechanism — [`Agent::stop`] is —
//! but a panic unwinding past an `Agent` must not leave a `claude` running, and
//! that is a path no amount of care in the happy case covers.
//!
//! **Neither of them covers an exit this process does not get to run code
//! after**, and `kill_on_drop` least of all: it needs a tokio runtime that a
//! `taskkill /F` has already taken away. So the child is also joined to a job
//! object at spawn — [`crate::process::bound_to_this_server`] is what that
//! guarantees, what it does not, and the three days of orphaned processes that
//! were the argument for it.

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

/// The `--permission-prompt-tool` value that routes permission prompts onto this
/// stdio pair instead of into an MCP server.
///
/// The flag is documented as taking "an MCP tool to use for permission prompts",
/// and `stdio` is the reserved name that means "ask me, here" — it is what the
/// Agent SDK passes when a host supplies a `canUseTool` callback, and it is
/// hidden from `--help`. So this is the one string in the crate read off the
/// binary rather than off documentation, and
/// `fixtures/claude-cli/07-permission-approved.ndjson` is the recording that
/// proves it does what it says.
///
/// Passed on every session rather than only for the mode that asks. What the flag
/// selects is *where a prompt goes*, not whether there is one — that is
/// `--permission-mode`'s job, and under `bypassPermissions` the CLI never asks
/// and this channel stays silent. Making it conditional would mean a mode the
/// developer changed mid-conversation could leave a running agent unable to ask.
const PERMISSION_PROMPT_TOOL: &str = "stdio";

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
    /// the owning driver's provider resume cursor.
    pub resume: Option<String>,
}

/// The CLI's `--permission-mode` for a thread's runtime mode.
///
/// Upstream's table (`ClaudeAdapter.ts:3510`) verbatim, including its one
/// omission: `approval-required` maps to nothing, because upstream expresses it
/// by *not* passing the flag and answering the CLI's permission callback
/// instead. This server now does the same — the callback is
/// [`PERMISSION_PROMPT_TOOL`] — so the CLI's own default applies and its default
/// is to ask. Which is what `approval-required` means.
pub fn permission_mode_for(runtime_mode: &str) -> Option<&'static str> {
    match runtime_mode {
        "auto-accept-edits" => Some("acceptEdits"),
        "auto" => Some("auto"),
        "full-access" => Some("bypassPermissions"),
        _ => None,
    }
}

/// The CLI's permission mode for a thread's runtime mode, as a *push* to a
/// running child.
///
/// The same question as [`permission_mode_for`] asked at a different moment, and
/// the answers differ in exactly one place — which is why these are two functions
/// rather than one. A launch expresses `approval-required` by omitting the flag;
/// a push has no such option, because there is no request that means "go back to
/// whatever you would have done". So this translation is **total**, and
/// `approval-required` becomes the CLI's `default`, whose behaviour is to ask.
/// That is also upstream's convention for the no-flag case (`?? "default"` in
/// `ClaudeAdapter.ts`) rather than an invention here.
///
/// The launch table is deliberately left lossy. Passing `--permission-mode
/// default` for `approval-required` would be a different change and a wrong one:
/// it would override a developer's own configured default with the CLI's.
///
/// A mode this build does not know falls to `default` for the same reason,
/// though nothing can reach it — the contract's closed set is checked in
/// [`crate::orchestration`] before a mode is written to a thread.
pub fn pushed_permission_mode_for(runtime_mode: &str) -> &'static str {
    permission_mode_for(runtime_mode).unwrap_or("default")
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
            // What a subagent says, on the same wire. Without it a subagent is
            // only ever a row that says how far along it is; with it the row can
            // say what the thing is actually doing, which is the question a
            // developer watching one has.
            //
            // Not a setting, because there is no version of this application
            // where watching an agent work is opt-in — and it costs nothing when
            // no subagent is running, since the lines only exist if one is.
            //
            // Safe to pass only because `parent_tool_use_id` is read: the
            // forwarded lines are ordinary `assistant` and `user` envelopes, and
            // a build that did not tell them apart would file a subagent's words
            // in the developer's transcript as the agent's own.
            .arg("--forward-subagent-text")
            // Where a permission prompt goes. See [`PERMISSION_PROMPT_TOOL`].
            .arg("--permission-prompt-tool")
            .arg(PERMISSION_PROMPT_TOOL)
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
        // Before the pipes are taken, so that the window in which this `claude`
        // is unsupervised is as short as this function can make it. What it
        // covers is mostly not `claude` itself — see
        // `crate::process::bound_to_this_server`, and the dev servers its Bash
        // tool starts, which no kill of this handle has ever reached.
        crate::process::bound_to_this_server_async(&child);

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
        // laplus's own — and the most recent one is kept, for the one failure
        // whose only account of itself is there. See [`Agent::complaint`].
        let complaint = Arc::new(Mutex::new(None));
        let latest = Arc::clone(&complaint);
        let reading_stderr = tokio::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                eprintln!("laplus: claude: {line}");
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
    pub async fn send(&mut self, content: &[impl serde::Serialize]) -> std::io::Result<()> {
        self.write_line(crate::protocol::user_message_line(content))
            .await
    }

    /// Answer a permission request the agent is waiting on.
    ///
    /// The other half of the outbound protocol, and the half with a deadline: the
    /// agent has stopped mid-turn and will not go on until this lands, so a write
    /// that failed here is a wedged conversation rather than a lost message. The
    /// caller reports it; this only says so.
    pub async fn answer(
        &mut self,
        request_id: &str,
        answer: &crate::protocol::Answer,
    ) -> std::io::Result<()> {
        self.write_line(crate::protocol::control_response_line(request_id, answer))
            .await
    }

    /// Stop the turn in flight, without ending the session.
    ///
    /// The one line this server sends that is a *question*: the CLI answers it
    /// with a `control_response` naming the same `request_id`, which is why the
    /// caller mints one and keeps it. A write that failed here is a stop button
    /// that did nothing, so — as with a decision — the caller reports it.
    ///
    /// Nothing is waited for. The agent's account of what the interrupt did
    /// arrives on stdout like everything else it says: an acknowledgement, then
    /// whatever the turn had already buffered, then the turn's `result`.
    pub async fn interrupt(&mut self, request_id: &str) -> std::io::Result<()> {
        self.write_line(crate::protocol::interrupt_line(request_id))
            .await
    }

    /// Ask how full the context window is.
    ///
    /// The second question this server asks, and the one with no deadline on
    /// either side: nothing is waiting for the answer and nothing breaks if it
    /// never comes. A CLI too old to know the request answers with an error, a
    /// CLI that is busy answers late, and in both cases the meter goes on being
    /// filled from the token counts — which is why the caller does not report a
    /// failure here into the conversation the way it reports a failed interrupt.
    ///
    /// Nothing is waited for, the same as an interrupt: the answer arrives on
    /// stdout as a `control_response` naming the same `request_id`, and
    /// [`crate::protocol::SessionState::reduce`] folds it where it lands.
    pub async fn measure_context(&mut self, request_id: &str) -> std::io::Result<()> {
        self.write_line(crate::protocol::context_usage_line(request_id))
            .await
    }

    /// Move this agent to another permission mode without replacing it.
    ///
    /// The third question this server asks, and the first that changes what the
    /// child *is* rather than what it is doing: `--permission-mode` is read once
    /// at launch, so before this a mode the developer changed mid-conversation
    /// reached the picker, the database and the next turn's request and never
    /// reached the process serving them.
    ///
    /// Nothing is waited for, as with the other two. The CLI answers on stdout
    /// with a `control_response` naming this id, and the caller keys the outcome
    /// off that — a refusal has to be reported, because a mode that did not land
    /// is a session running under rules the developer thinks they changed.
    pub async fn set_permission_mode(
        &mut self,
        request_id: &str,
        mode: &str,
    ) -> std::io::Result<()> {
        self.write_line(crate::protocol::set_permission_mode_line(request_id, mode))
            .await
    }

    /// Move this agent to another model without replacing it.
    ///
    /// [`Agent::set_permission_mode`]'s twin, down to the reason it exists: the
    /// model is a launch flag too, and a developer who switched model
    /// mid-conversation was answered by the one they started with.
    pub async fn set_model(&mut self, request_id: &str, model: &str) -> std::io::Result<()> {
        self.write_line(crate::protocol::set_model_line(request_id, model))
            .await
    }

    /// One JSON object on one line, which is the whole of what this server ever
    /// says to the agent.
    ///
    /// Flushed rather than left to the buffer, because the agent is waiting for
    /// it and a line that sat in a write buffer would look exactly like a hang.
    async fn write_line(&mut self, mut line: String) -> std::io::Result<()> {
        let stdin = self.stdin.as_mut().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "the agent's input has already been closed",
            )
        })?;
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
                    eprintln!("laplus: cannot wait for the agent: {error}");
                    break;
                }
                Ok(None) if tokio::time::Instant::now() >= deadline => break,
                Ok(None) => tokio::time::sleep(EXIT_POLL).await,
            }
        }

        // **The tree, not the handle.** This used to kill only the process this
        // server started, reasoning that `crate::provider` resolves a native
        // `claude.exe` in production so the kill lands where it says it does.
        // That is true about `claude.exe` and says nothing about what `claude`
        // started: its Bash tool's dev servers, the stdio MCP servers its
        // configuration names, its subagents. Those are this server's
        // grandchildren, a kill of this handle has never reached them, and on
        // 2026-09-01 six of them — two `bun dev` and four `vite` — were found
        // three days old with dead parents, holding four loopback ports.
        //
        // The same reasoning retired the rest of that comment. A job object is
        // now taken out at spawn (`crate::process::bound_to_this_server`), which
        // is what covers the exits this function is never called on; this is
        // what keeps a conversation's cost from being paid until then.
        crate::process::terminate_tree_and_wait_async(&mut self.child).await;
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
            "upstream expresses this by omitting the flag, and so does this — the \
             CLI's own default is to ask, and asking is what the mode means"
        );
        assert_eq!(permission_mode_for("something-later"), None);
    }

    /// The same table asked as a *push*, where the omission cannot stand: there
    /// is no request that means "pass no flag", so `approval-required` has to
    /// name a mode, and the mode whose behaviour is to ask is `default`.
    ///
    /// Pinned beside the launch table rather than instead of it, because the one
    /// thing that would quietly break ticket 11 is the two being made to agree —
    /// a launch that started passing `--permission-mode default` would override
    /// a developer's own configured default with the CLI's.
    #[test]
    fn a_pushed_mode_is_total_where_the_launch_table_is_lossy() {
        assert_eq!(pushed_permission_mode_for("auto-accept-edits"), "acceptEdits");
        assert_eq!(pushed_permission_mode_for("auto"), "auto");
        assert_eq!(pushed_permission_mode_for("full-access"), "bypassPermissions");
        assert_eq!(
            pushed_permission_mode_for("approval-required"),
            "default",
            "the one runtime mode with no launch flag still has to be sayable"
        );
        assert_eq!(
            pushed_permission_mode_for("something-later"),
            "default",
            "unreachable — the contract's closed set is checked first — and the \
             safe answer if it ever is reached, because default asks"
        );
        // The four the CLI named when it refused one, from
        // `fixtures/claude-cli/21-modes-refused.ndjson`: every mode this table
        // can produce is one the CLI will take.
        for mode in ["acceptEdits", "auto", "bypassPermissions", "default"] {
            assert!(
                "acceptEdits, auto, bypassPermissions, default, dontAsk, plan".contains(mode),
                "{mode} is not one the CLI accepts"
            );
        }
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
