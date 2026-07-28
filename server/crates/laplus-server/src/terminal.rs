//! Terminals: a real shell in the project's folder, driven over the socket.
//!
//! The whole of this module rests on one decision, and it is the decision the
//! spec already made: a terminal is a **pseudo-terminal**, not a pair of pipes.
//! A program only emits colour, redraws a full screen, or asks how wide the
//! window is when it believes it is talking to a tty. Pipes would not be a
//! smaller version of this feature; they would be a different one that looks
//! broken, and no amount of server-side work afterwards could put back what was
//! never emitted.
//!
//! ## The server is a pipe, and that is the design
//!
//! Nothing here interprets terminal output. Bytes come off the pty, are decoded
//! as UTF-8, and go to the client exactly as they arrived. The emulator is
//! `xterm.js`, in the UI, where it already is. That is what makes "interactive
//! programs render correctly" and "colour renders correctly" true by
//! construction rather than by feature work: there is no code here that could
//! get them wrong.
//!
//! The one place this server *reads* the stream is [`visible`], and it does so
//! only for the copy it keeps — see below.
//!
//! ## The client is the other half of the terminal, and it must answer
//!
//! ConPTY opens by sending `ESC [ 6 n` — "where is the cursor?" — and the shell
//! **blocks until something answers**. In production `xterm.js` answers, which
//! is why this works at all; a test that drives a terminal has to answer too, or
//! it will watch a shell that never prints a prompt. This was measured, not
//! assumed: a spike against `cmd.exe` hung indefinitely at exactly that byte and
//! ran normally as soon as a `ESC [ 1 ; 1 R` was written back.
//!
//! That fact drives the next one.
//!
//! ## Scrollback is not the stream
//!
//! The server keeps a copy of what the terminal has shown, because a client
//! that reconnects — or falls far enough behind that a snapshot is cheaper than
//! catching up — is sent the copy rather than the missing bytes. So the copy is
//! *replayed into an emulator*, and anything in it that asks the emulator a
//! question would be asked again, with the answer going to a shell that is not
//! waiting for one. [`visible`] therefore strips exactly the sequences that are
//! queries — cursor position, device attributes, the OSC colour queries — and
//! keeps everything else byte for byte, including all of the colour and cursor
//! motion that makes the replay look like what was there. Upstream strips the
//! same set (`sanitizeTerminalHistoryChunk`), for the same reason.
//!
//! ## An unanswered question is part of the terminal's state
//!
//! Those two facts collide. The very first thing a shell says is a question,
//! and it says it the instant it starts — which is during `terminal.open`,
//! before anything has attached. Stripping it from scrollback and dropping it
//! from a stream nobody was reading yet would leave a terminal that is running,
//! looks fine, and will never print a prompt as long as it lives.
//!
//! So a query the shell is still waiting on is *remembered*, and re-sent to
//! whoever attaches. It is cleared the moment anything is written to the pty,
//! because that write is the answer. Scrollback stays clean — it is still the
//! wrong place for a question — and the question still reaches something that
//! can answer it, whichever of `terminal.open` and `terminal.attach` the client
//! happened to send first. It generalises past the opening handshake for free:
//! a full-screen program that asks the window for its size while the pane is
//! detached is asking the same way.
//!
//! ## What one terminal costs
//!
//! Three OS threads, because all three pty operations are blocking and none of
//! them can be waited on together:
//!
//! - a **reader**, parked on the pty until bytes or EOF;
//! - a **writer**, parked on a queue until the developer types;
//! - a **reaper**, parked on the child until it exits, which then closes the pty
//!   so the reader can see EOF, joins the other two, and announces the exit.
//!
//! The reaper is the one with the ordering obligation. Closing the pty *before*
//! joining the reader is what makes the last line of output arrive before the
//! exit that ended it; on Windows the reader would otherwise never see EOF at
//! all, because ConPTY holds the output pipe open until the console object is
//! closed. That too was measured rather than assumed.
//!
//! ## A terminal ends when it is closed, not when its shell exits
//!
//! Those are two different facts and the contract has two events for them.
//! A shell that exits leaves the terminal on the list, saying `exited`, with
//! everything it printed still readable — a pane that vanished the moment a
//! command ran `exit` would take the output with it, and the output is usually
//! the reason the developer was looking. `terminal.close` is the developer
//! saying they are done: the shell is killed, waited for, and the terminal
//! comes off the list.
//!
//! Which is also what makes closing the reaping point. [`Terminals::close`]
//! takes the terminal out of the registry *first* and only then kills what was
//! in it, so nothing can arrive for a terminal that is being reaped — there is
//! no id left to send it to. Killing is the cheap half; the half that matters
//! is waiting, because a `terminal.close` that returned before the pty was shut
//! and its threads joined would be a promise the server had not kept.
//!
//! `deleteHistory` on that call is therefore not carried at all. What this
//! server keeps of a terminal is *in* that terminal, so closing one deletes its
//! history whichever way the flag was set; upstream keeps a second copy under
//! its logs directory and has to be told whether to remove that too. Reading
//! the flag and ignoring it would read, later, as a decision somebody made.
//!
//! ## Detaching is not a call, and reattaching is not a restart
//!
//! There is no `terminal.detach` on this wire — navigating away from a pane
//! cancels its `terminal.attach` subscription and touches nothing else. So
//! there is no code here for it, which is exactly why
//! `socket_terminal_lifecycle.rs` pins it: it is behaviour nothing in this
//! module asserts and that a plausible change — reaping a terminal when its
//! last subscriber leaves — would silently take away.
//!
//! ## What this module does not do
//!
//! - **Scrollback is in memory only.** Navigating away and back is a
//!   re-attachment to a terminal that never went anywhere, so it keeps
//!   everything; closing the app is not, and does not. Upstream writes each
//!   terminal's history to a file under its logs directory and reads it back
//!   when a terminal of that id is opened again. Nothing in ticket 18's
//!   acceptance asks for it — and the ticket *does* ask that closing the app
//!   reap every terminal, so what a restored scrollback would describe is a
//!   shell this server has already killed.
//! - **`hasRunningSubprocess` is always `false`, and the label is always the
//!   terminal's own.** Upstream polls the process tree with `powershell`/`pgrep`
//!   to notice that a shell is running `vim` and to title the tab after it.
//!   That is a poll per terminal per interval for a caption. A process running
//!   in a detached terminal is not lost by it being `false` — it keeps running,
//!   and everything it printed is there on reattachment, which is what ticket
//!   18 asks for and what
//!   `a_process_that_outlives_the_pane_keeps_running_and_its_output_is_kept`
//!   asserts. What is missing is the *busy dot* on the tab, and inventing an
//!   answer for it would be worse than the honest one.
//! - **`subscribeTerminalEvents` is not implemented.** It is a real method in
//!   the contract, but nothing in the reused UI calls it: the terminal pane
//!   reads its output through `terminal.attach` and its list through
//!   `subscribeTerminalMetadata`. Implementing it would be a surface with no
//!   caller.
//! - **No signal is ever reported.** `exitSignal` is always null. Windows has
//!   no signals and `portable_pty::ExitStatus` does not distinguish one, so
//!   there is nothing true to put there.

use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use portable_pty::{Child, ChildKiller, CommandBuilder, MasterPty, PtySize};
use serde_json::{json, Value};
use tokio::sync::broadcast;

use crate::clock::now_iso;
use crate::subscriptions::{EventSource, BACKLOG};

/// Start a shell, or hand back the one already running under this id.
pub const OPEN: &str = "terminal.open";

/// The subscription that *is* one terminal: its scrollback, then its output.
pub const ATTACH: &str = "terminal.attach";

/// Keystrokes, and everything else the emulator sends back — including the
/// answers to the queries the shell asked.
pub const WRITE: &str = "terminal.write";

/// The pane changed size, so the pty must.
pub const RESIZE: &str = "terminal.resize";

/// Forget what the terminal has shown. The shell in it is untouched.
pub const CLEAR: &str = "terminal.clear";

/// Put a new shell in this terminal, replacing whatever was in it.
pub const RESTART: &str = "terminal.restart";

/// End a terminal — or every terminal on a thread — and reap what was running
/// in it.
pub const CLOSE: &str = "terminal.close";

/// The subscription that *is* the terminal list. Captured whole in
/// `fixtures/socket-wire/04-streaming-subscription.ndjson`, and part of the
/// UI's boot sequence.
pub const SUBSCRIBE_METADATA: &str = "subscribeTerminalMetadata";

/// The most scrollback a terminal keeps.
///
/// The client's own number (`DEFAULT_MAX_TERMINAL_BUFFER_BYTES` in
/// `terminalSession.ts`), and deliberately the same one: keeping more would be
/// sending a client bytes it is about to throw away, and keeping less would
/// make a reattach show less than the tab it replaced.
pub const MAX_HISTORY_BYTES: usize = 512 * 1024;

/// The size a terminal opens at when the client does not say. Upstream's
/// `DEFAULT_OPEN_COLS`/`DEFAULT_OPEN_ROWS`; the pane resizes to its real size a
/// frame later either way.
const DEFAULT_COLS: u16 = 120;
const DEFAULT_ROWS: u16 = 30;

/// The contract's bounds on a terminal size (`terminal.ts`). A client that
/// sends more is out of contract; clamping rather than refusing keeps a
/// mis-measured pane from making the terminal unusable.
const MAX_COLS: u16 = 1000;
const MAX_ROWS: u16 = 500;

/// How many un-written keystroke batches may be queued for a shell that is not
/// reading its input.
///
/// A person types one key at a time and a shell reads continuously, so this is
/// only the window between a write arriving and the writer thread waking. It is
/// bounded rather than unbounded because the alternative to a bound here is a
/// shell that has stopped reading absorbing pasted input forever — and a full
/// queue is a *fact about the terminal* that the developer is better told than
/// left to guess at.
const INPUT_QUEUE: usize = 64;

/// The most input one `terminal.write` may carry.
///
/// The contract's own `isMaxLength(65_536)`. Enforced rather than trusted
/// because it is the only thing bounding what [`INPUT_QUEUE`] holds: sixty-four
/// slots of unbounded data is not a bound.
const MAX_WRITE_BYTES: usize = 65_536;

/// The most unanswered questions a terminal will remember. A handful of escape
/// sequences; the number is generous rather than tuned, because what it guards
/// against is a program in a loop rather than ordinary use.
const MAX_QUESTION_BYTES: usize = 4 * 1024;

/// How much the reader asks the pty for at a time. Whatever is available comes
/// back, so this is the ceiling on one output event rather than a target.
const READ_BUFFER: usize = 8 * 1024;

/// What a terminal is doing, as `TerminalSessionStatus` in the contract.
///
/// The contract's fourth state, `starting`, is never published: a terminal is
/// started by the call that answers with its snapshot, so by the time a client
/// can see one it is either running or it failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    Running,
    Exited,
    Error,
}

impl Status {
    fn as_str(self) -> &'static str {
        match self {
            Status::Running => "running",
            Status::Exited => "exited",
            Status::Error => "error",
        }
    }
}

/// Every terminal this server is running, and who is watching the list.
///
/// Cheap to clone and every clone is the same registry, like
/// [`crate::config_store::ConfigStore`] and [`crate::orchestration::Shell`]: an
/// attachment outlives the call that opened it, and the terminal outlives the
/// attachment.
#[derive(Debug, Clone)]
pub struct Terminals {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    sessions: Mutex<HashMap<Key, Arc<Session>>>,
    /// The list, as `subscribeTerminalMetadata` reads it. Separate from each
    /// terminal's own feed because they answer different questions: this one is
    /// "which terminals are there", and that one is "what did this terminal
    /// say" — and a client watching the list must not be woken by every byte.
    metadata: broadcast::Sender<Value>,
}

/// A terminal's name on this wire. Ids are the client's — the server never
/// allocates one — and are unique only within a thread, which is why the thread
/// is half of the name.
type Key = (String, String);

fn key(thread_id: &str, terminal_id: &str) -> Key {
    (thread_id.to_string(), terminal_id.to_string())
}

/// One terminal: what it is, what it has said, and what is running behind it.
///
/// Only the two ids are here rather than in [`State`], and that is the
/// distinction the type is drawing: they are the terminal's *name* and nothing
/// can change them. Where it is rooted can — `terminal.restart` carries a
/// working directory of its own — so that lives with everything else that
/// changes.
#[derive(Debug)]
struct Session {
    thread_id: String,
    terminal_id: String,
    events: broadcast::Sender<Value>,
    /// The registry's feed, held here rather than reached back for. The reaper
    /// runs on a thread of its own long after the call that opened this
    /// terminal has returned, and the list has to hear that a shell exited from
    /// exactly there.
    metadata: broadcast::Sender<Value>,
    state: Mutex<State>,
    /// The thread that waits for the shell to exit.
    ///
    /// Kept out of [`State`] deliberately: joining it has to be possible
    /// *without* holding the lock that it takes on its way out, and a handle
    /// stored inside the thing it locks makes that mistake the easy one.
    reaper: Mutex<Option<JoinHandle<()>>>,
}

/// Everything about a terminal that changes.
struct State {
    /// Where the shell was started. Changes when a restart names somewhere
    /// else, which is the reason it is not on [`Session`].
    cwd: String,
    worktree_path: Option<String>,
    status: Status,
    pid: Option<u32>,
    exit_code: Option<i64>,
    /// What the terminal has shown, with the emulator's questions taken out.
    /// See the module documentation.
    history: String,
    /// A control sequence split across two reads, waiting for its other half.
    pending: String,
    /// Questions the shell has asked and nothing has answered. Sent to whoever
    /// attaches; cleared by the write that answers them. See the module
    /// documentation — without this a terminal opened before it was attached to
    /// never prints a prompt.
    questions: String,
    cols: u16,
    rows: u16,
    /// Numbers every event this terminal publishes, and stamped into the
    /// snapshot so that a snapshot and the events after it can be told apart
    /// from the events already inside it. See [`Terminals::attach`].
    sequence: u64,
    updated_at: String,
    /// The live pty, or `None` once the shell has gone.
    ///
    /// One `Option` over three handles rather than three over one each, because
    /// they are acquired together and released together and there is no state
    /// in which some of them are the truth: "is there a shell in this terminal"
    /// has to be one question with one answer, or two callers will ask it two
    /// ways and disagree.
    pty: Option<Pty>,
}

/// The three handles on a running shell.
///
/// Dropping this closes the console, which is what lets the reader see EOF — on
/// Windows it is the *only* thing that does — and closes the input queue, which
/// is what lets the writer thread come back. So releasing the shell is one
/// assignment rather than a sequence somebody could get half right.
struct Pty {
    master: Box<dyn MasterPty + Send>,
    input: std::sync::mpsc::SyncSender<Vec<u8>>,
    killer: Box<dyn ChildKiller + Send + Sync>,
}

impl std::fmt::Debug for State {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("State")
            .field("status", &self.status)
            .field("pid", &self.pid)
            .field("history", &self.history.len())
            .field("size", &(self.cols, self.rows))
            .field("sequence", &self.sequence)
            .finish_non_exhaustive()
    }
}

impl Terminals {
    pub fn new() -> Terminals {
        Terminals {
            inner: Arc::new(Inner {
                sessions: Mutex::new(HashMap::new()),
                metadata: broadcast::channel(BACKLOG).0,
            }),
        }
    }

    /// Terminals with a shell still running behind them.
    ///
    /// The gauge that makes "nothing is orphaned" observable from outside,
    /// beside the ones for connections, subscriptions, watches and agents.
    pub fn live(&self) -> usize {
        self.sessions()
            .values()
            .filter(|session| session.state.lock().unwrap().status == Status::Running)
            .count()
    }

    fn sessions(&self) -> std::sync::MutexGuard<'_, HashMap<Key, Arc<Session>>> {
        self.inner
            .sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Start a shell, or answer with the one already under this id.
    ///
    /// Blocking — it stats a directory and starts a process — so it is run as a
    /// [`crate::rpc::Deferred`] rather than on the connection's read loop.
    pub fn open(&self, call: Open) -> Result<Value, Value> {
        let session = self.open_session(&call)?;
        let state = session.state.lock().unwrap();
        Ok(session.snapshot(&state))
    }

    /// The terminal under this id, started if it is not there.
    fn open_session(&self, call: &Open) -> Result<Arc<Session>, Value> {
        // Checked before the registry is locked: it touches the disk, and
        // nothing about the answer depends on what else is opening.
        check_cwd(&call.cwd)?;
        Ok(self.open_locked(self.sessions(), call))
    }

    /// [`Terminals::open_session`] with the registry already locked and the
    /// working directory already checked.
    ///
    /// Split out for one caller: [`Terminals::restart`] has to decide whether
    /// the terminal exists and act on the answer without letting go in between,
    /// and a helper that took the lock itself would make that impossible to
    /// write rather than merely easy to get wrong.
    fn open_locked(
        &self,
        mut sessions: std::sync::MutexGuard<'_, HashMap<Key, Arc<Session>>>,
        call: &Open,
    ) -> Arc<Session> {
        if let Some(existing) = sessions.get(&call.key()) {
            let existing = Arc::clone(existing);
            // A terminal already under this id is *this* terminal. The UI opens
            // and attaches from two places at once, so a second open is the
            // ordinary case rather than a mistake, and starting a second shell
            // for it would replace the developer's session with a blank one.
            let mut state = existing.state.lock().unwrap();
            if state.status == Status::Running {
                // Against the size it already has, so an open that said nothing
                // about the size leaves it alone. Upstream reads the same field
                // the same way (`input.cols ?? session.cols`), and the
                // alternative is worse than it sounds: the pane that opened this
                // terminal is not always the one re-opening it, so defaulting
                // here would shrink a terminal to 120x30 on somebody else's
                // second open.
                let (cols, rows) = call.size(state.cols, state.rows);
                // A resize that will not take is not a reason to refuse the
                // developer the terminal they already have.
                let _ = existing.apply_size(&mut state, cols, rows);
            }
            drop(state);
            return existing;
        }

        let (cols, rows) = call.size(DEFAULT_COLS, DEFAULT_ROWS);
        let session = Arc::new(Session::new(call, cols, rows, self.inner.metadata.clone()));
        match start_shell(call, cols, rows, &shell_candidates(&call.env)) {
            // Nothing to announce: this terminal did not exist a moment ago, so
            // there is nobody attached to tell. The call's own answer is the
            // announcement.
            Ok(shell) => session.adopt(shell, |_| {}),
            Err(why) => session.failed_to_start(&why),
        }

        sessions.insert(call.key(), Arc::clone(&session));
        // Announced with the registry lock released, because describing a
        // terminal takes its own lock and a subscriber woken by this will
        // immediately want the registry.
        drop(sessions);
        session.announce(&session.state.lock().unwrap());
        session
    }

    /// Open a `terminal.attach` subscription: this terminal's scrollback, then
    /// everything it says afterwards.
    ///
    /// **The snapshot and the events after it must not overlap**, and that is
    /// what the supersession rule is for. The scrollback in a snapshot already
    /// contains every byte published up to the sequence stamped on it, so an
    /// event at or below that number would append text the client has just been
    /// handed — visible as duplicated output, not as a stale value some later
    /// event corrects. The window is real rather than theoretical: the pump
    /// re-describes the world whenever a subscriber falls behind, which on a
    /// busy terminal is often.
    pub fn attach(&self, call: Attach) -> Result<EventSource, Value> {
        // Bound before the match, not read inside it. A guard in a match
        // scrutinee lives until the whole match ends, and the arm below locks
        // the same registry — which on a lock that is not reentrant is not a
        // slow path, it is a stop.
        let existing = self.sessions().get(&call.key()).cloned();
        let session = match existing {
            Some(session) => {
                // The client asked, by name, for a shell to be put back in a
                // terminal whose own has gone. Only ever *because it asked* —
                // an attach is otherwise a read, and a read that silently
                // replaced the terminal it was opened on would lose whatever
                // the developer was looking at.
                if let Some(open) = call.shell_to_put_back(&session) {
                    // Checked here rather than left to the spawn, because this
                    // attach *is* going to use the directory it named — which
                    // is what makes it different from an attach to a running
                    // terminal, where a `cwd` that is not one is beside the
                    // point.
                    check_cwd(&open.cwd)?;

                    let sessions = self.sessions();
                    // **Both halves of the decision are re-taken under the lock
                    // that would do the replacing**, because both can have
                    // stopped being true since it was made. A terminal closed
                    // since the lookup above is no longer this registry's to
                    // start a shell in, and one started there would be a shell
                    // nothing could reach or reap; a terminal that is running
                    // again is one another attach has already revived, and
                    // replacing it would kill the shell that attach just
                    // started.
                    let registered = sessions
                        .get(&call.key())
                        .is_some_and(|current| Arc::ptr_eq(current, &session));
                    let idle = session.state.lock().unwrap().status != Status::Running;
                    match registered && idle {
                        true => self.replace_shell(sessions, &session, open),
                        false => drop(sessions),
                    }
                }
                session
            }
            // The UI attaches and opens from two different places, so an attach
            // may genuinely arrive first. It carries everything an open needs
            // for exactly that reason — and without it there is nothing to
            // attach to and nothing to guess from.
            None => match &call.opening {
                Some(Ok(open)) => self.open_session(open)?,
                // It offered to open one and the directory it named is not a
                // directory. Said as that, rather than as "no such terminal",
                // which is true but is not the thing to go and fix.
                Some(Err(why)) => return Err(why.clone()),
                None => return Err(lookup_error(&call.thread_id, &call.terminal_id)),
            },
        };

        let updates = session.events.subscribe();
        let watermark = Arc::new(AtomicU64::new(0));
        let described = Arc::clone(&watermark);
        let describing = Arc::clone(&session);

        Ok(EventSource::new(
            move || {
                let state = describing.state.lock().unwrap();
                described.store(state.sequence, Ordering::Relaxed);
                let mut opening = vec![json!({
                    "type": "snapshot",
                    "snapshot": describing.snapshot(&state),
                })];

                // After the snapshot, not inside it. Scrollback is replayed on
                // every reattach and a question does not belong in something
                // replayed; this is sent once, to the client that is now the
                // only thing able to answer it.
                if !state.questions.is_empty() {
                    opening.push(describing.event(
                        state.sequence,
                        json!({"type": "output", "data": state.questions}),
                    ));
                }
                opening
            },
            updates,
        )
        .superseding(move |event| {
            event["sequence"]
                .as_u64()
                .is_some_and(|sequence| sequence <= watermark.load(Ordering::Relaxed))
        }))
    }

    /// Open a `subscribeTerminalMetadata` subscription: the terminal list, then
    /// every change to it.
    ///
    /// No supersession here, unlike [`Terminals::attach`]. The client folds
    /// these by replacing the entry with a matching id, so seeing one twice
    /// lands on the same list.
    pub fn subscribe_metadata(&self) -> EventSource {
        let updates = self.inner.metadata.subscribe();
        let terminals = self.clone();
        EventSource::new(
            move || {
                let mut listed: Vec<Value> = terminals
                    .sessions()
                    .values()
                    .map(|session| session.summary(&session.state.lock().unwrap()))
                    .collect();
                // Ordered so that two snapshots of an unchanged registry are
                // the same document. A `HashMap` would otherwise reorder the
                // list for no reason a reader could see.
                listed.sort_by(|left, right| {
                    (&left["threadId"].as_str(), &left["terminalId"].as_str())
                        .cmp(&(&right["threadId"].as_str(), &right["terminalId"].as_str()))
                });
                vec![json!({"type": "snapshot", "terminals": listed})]
            },
            updates,
        )
    }

    /// Send what the developer typed — and what their emulator answered — to
    /// the shell.
    pub fn write(&self, call: &WriteInput) -> Result<Value, Value> {
        let session = self.running(&call.thread_id, &call.terminal_id)?;
        let mut state = session.state.lock().unwrap();
        let pid = state.pid.unwrap_or_default();

        match session.type_in(&mut state, &call.data) {
            Ok(()) => Ok(Value::Null),
            Err(why) => Err(write_error(
                &call.thread_id,
                &call.terminal_id,
                pid,
                &why,
            )),
        }
    }

    /// Resize the pty, which is what makes output rewrap and what a full-screen
    /// program redraws itself against.
    pub fn resize(&self, call: &Resize) -> Result<Value, Value> {
        let session = self.running(&call.thread_id, &call.terminal_id)?;
        let mut state = session.state.lock().unwrap();
        let pid = state.pid.unwrap_or_default();

        match session.apply_size(&mut state, call.cols, call.rows) {
            Ok(()) => Ok(Value::Null),
            Err(why) => Err(resize_error(
                &call.thread_id,
                &call.terminal_id,
                pid,
                call.cols,
                call.rows,
                &why,
            )),
        }
    }

    /// Forget what this terminal has shown, without touching what is running
    /// in it.
    ///
    /// Deliberately not restricted to a *running* terminal. Clearing is what a
    /// developer does to a pane full of something they are done reading, and a
    /// shell that has exited is the case where there is most of it.
    pub fn clear(&self, call: &Clear) -> Result<Value, Value> {
        let session = self
            .sessions()
            .get(&key(&call.thread_id, &call.terminal_id))
            .cloned()
            .ok_or_else(|| lookup_error(&call.thread_id, &call.terminal_id))?;

        let mut state = session.state.lock().unwrap();
        state.history.clear();
        state.pending.clear();
        // `questions` is deliberately *not* cleared. It is not something the
        // terminal showed — it is a reply the shell is still blocked on, and
        // forgetting it here would leave a cleared terminal that never prints
        // another prompt. See "An unanswered question is part of the terminal's
        // state" in the module documentation, and ADR-0005.
        session.publish(&mut state, |sequence| {
            session.event(sequence, json!({"type": "cleared"}))
        });
        Ok(Value::Null)
    }

    /// Put a new shell in this terminal, replacing whatever was in it.
    ///
    /// Blocking — it kills a process, waits for it, and starts another — so it
    /// is run as a [`crate::rpc::Deferred`].
    pub fn restart(&self, call: &Restart) -> Result<Value, Value> {
        check_cwd(&call.opening.cwd)?;

        // One lock across both the question and the answer. Letting go in
        // between would make a restart that found nothing race an open that put
        // something there, and the restart would hand back the other caller's
        // shell without having restarted anything.
        let sessions = self.sessions();
        let session = match sessions.get(&call.opening.key()).cloned() {
            Some(session) => {
                self.replace_shell(sessions, &session, &call.opening);
                session
            }
            // Restarting a terminal that is not there is opening one. The
            // developer asked for a running shell under this id and there is no
            // shell under it to keep, which is the same request an open makes.
            None => self.open_locked(sessions, &call.opening),
        };

        let state = session.state.lock().unwrap();
        Ok(session.snapshot(&state))
    }

    /// End a terminal, or every terminal on a thread, and reap what was
    /// running in it.
    ///
    /// Blocking for the same reason [`Terminals::shutdown`] is: killing a shell
    /// is not the part that matters, and waiting for the reaper is.
    ///
    /// Closing something that is not there is **not** an error. A pane is
    /// closed by a client that has already removed the tab, and telling it that
    /// the terminal it just stopped showing does not exist would be answering a
    /// question it did not ask.
    pub fn close(&self, call: &Close) -> Result<Value, Value> {
        // Taken out of the registry first, and everything else happens after
        // the lock is released. Nothing can reach *this* session once it cannot
        // be found — a write, a resize or an attach naming it now misses it —
        // so it can be reaped at whatever pace that takes without holding up
        // every other terminal. What a later call under the same id gets is a
        // *new* terminal, which is the same answer it would get after the close
        // had finished.
        let mut sessions = self.sessions();
        let doomed: Vec<Arc<Session>> = match &call.terminal_id {
            Some(terminal_id) => sessions
                .remove(&key(&call.thread_id, terminal_id))
                .into_iter()
                .collect(),
            None => {
                let named: Vec<Key> = sessions
                    .keys()
                    .filter(|(thread_id, _)| thread_id == &call.thread_id)
                    .cloned()
                    .collect();
                named
                    .into_iter()
                    .filter_map(|one| sessions.remove(&one))
                    .collect()
            }
        };
        drop(sessions);

        for session in doomed {
            session.terminate();
            let mut state = session.state.lock().unwrap();
            session.publish(&mut state, |sequence| {
                session.event(sequence, json!({"type": "closed"}))
            });
            // …and off the list, which is the only way a client watching the
            // list rather than the terminal learns the tab has gone.
            session.withdraw();
        }
        Ok(Value::Null)
    }

    /// Kill what is in a terminal and start something else in it.
    ///
    /// Takes the registry guard **by value and holds it throughout**, and that
    /// is the one rule this module has about the registry lock: it is what
    /// makes a terminal's identity stable while somebody is changing what is in
    /// it. [`Terminals::open`] holds it across a spawn for the same reason, and
    /// [`Terminals::close`] can let go early only because it has taken the
    /// terminal *out* first, so there is no identity left to keep stable.
    ///
    /// The cost is real and is the price of that rule: while one terminal is
    /// being restarted, no other terminal can be opened, restarted or closed.
    /// It is bounded by a kill, two thread joins and a spawn.
    fn replace_shell(
        &self,
        sessions: std::sync::MutexGuard<'_, HashMap<Key, Arc<Session>>>,
        session: &Arc<Session>,
        call: &Open,
    ) {
        session.terminate();

        let (cols, rows) = {
            let mut state = session.state.lock().unwrap();
            // Against the size the terminal already has, so a restart that said
            // nothing about the size does not silently shrink the pane to the
            // default. Upstream reads it the same way.
            let (cols, rows) = call.size(state.cols, state.rows);
            state.cwd = call.cwd.clone();
            state.worktree_path = call.worktree_path.clone();
            state.cols = cols;
            state.rows = rows;
            state.exit_code = None;
            // The scrollback belonged to the shell that is gone, and so did
            // anything it was still waiting to be told. A question outlives an
            // attach; it does not outlive the process that asked it.
            state.history.clear();
            state.pending.clear();
            state.questions.clear();
            (cols, rows)
        };

        match start_shell(call, cols, rows, &shell_candidates(&call.env)) {
            Ok(shell) => session.adopt(shell, |state| {
                // The snapshot is taken and stamped under the same lock that
                // numbers the event, so the two carry the same sequence — which
                // is what [`Terminals::attach`] compares against when it drops
                // what a description already covered.
                let described = session.snapshot(state);
                session.publish(state, |sequence| {
                    let mut described = described;
                    described["sequence"] = json!(sequence);
                    session.event(sequence, json!({"type": "restarted", "snapshot": described}))
                });
            }),
            Err(why) => session.failed_to_start(&why),
        }

        // Released before the list is told, like the open path and for the same
        // reason: a subscriber woken by this immediately wants the registry.
        drop(sessions);
        session.announce(&session.state.lock().unwrap());
    }

    /// The terminal under this id, or the reason it cannot be spoken to.
    ///
    /// Two refusals rather than one, because they are two different facts with
    /// two different fixes: there is no such terminal, or there is one and its
    /// shell has gone.
    fn running(&self, thread_id: &str, terminal_id: &str) -> Result<Arc<Session>, Value> {
        let session = self
            .sessions()
            .get(&key(thread_id, terminal_id))
            .cloned()
            .ok_or_else(|| lookup_error(thread_id, terminal_id))?;

        let running = session.state.lock().unwrap().status == Status::Running;
        match running {
            true => Ok(session),
            false => Err(not_running_error(thread_id, terminal_id)),
        }
    }

    /// End every terminal and wait for all of it to actually be done.
    ///
    /// What shutdown calls, and for the same reason it waits for the agents: a
    /// shell that outlives the server that started it holds the project's files
    /// open and keeps a console the developer can no longer see. Killing is not
    /// enough on its own — the reaper is what closes the pty and joins the two
    /// threads reading and writing it, so this waits for the reaper rather than
    /// for the process.
    pub async fn shutdown(&self) {
        let sessions: Vec<Arc<Session>> = self.sessions().drain().map(|(_, one)| one).collect();
        if sessions.is_empty() {
            return;
        }

        // Joining a thread is blocking, and the reaper it joins is waiting on a
        // process — so this is the one place in the shutdown path that must not
        // run on a runtime worker.
        let _ = tokio::task::spawn_blocking(move || {
            for session in sessions {
                session.terminate();
            }
        })
        .await;
    }
}

impl Default for Terminals {
    fn default() -> Terminals {
        Terminals::new()
    }
}

impl Session {
    fn new(call: &Open, cols: u16, rows: u16, metadata: broadcast::Sender<Value>) -> Session {
        Session {
            thread_id: call.thread_id.clone(),
            terminal_id: call.terminal_id.clone(),
            events: broadcast::channel(BACKLOG).0,
            metadata,
            state: Mutex::new(State {
                cwd: call.cwd.clone(),
                worktree_path: call.worktree_path.clone(),
                status: Status::Running,
                pid: None,
                exit_code: None,
                history: String::new(),
                pending: String::new(),
                questions: String::new(),
                cols,
                rows,
                sequence: 0,
                updated_at: now_iso(),
                pty: None,
            }),
            reaper: Mutex::new(None),
        }
    }

    /// Take ownership of a started shell and put the threads that drive it to
    /// work.
    ///
    /// `announcing` runs with the state lock held, the new shell's handles
    /// already in it, and **the reader thread not yet started**. That window is
    /// the only place an event can be published that is guaranteed to precede
    /// every byte the new shell says, and `terminal.restart` needs exactly
    /// that: its `restarted` event carries a snapshot the client *replaces* its
    /// buffer with, so a byte that arrived first would be thrown away.
    fn adopt(self: &Arc<Session>, shell: Shell, announcing: impl FnOnce(&mut State)) {
        {
            let mut state = self.state.lock().unwrap();
            state.status = Status::Running;
            state.pid = shell.pid;
            state.pty = Some(Pty {
                master: shell.master,
                input: shell.input,
                killer: shell.killer,
            });
            state.updated_at = now_iso();
            announcing(&mut state);
        }

        let reading = Arc::clone(self);
        let reader = std::thread::spawn(move || read_output(&reading, shell.output));
        let reaping = Arc::clone(self);
        let reaper = std::thread::spawn(move || {
            reap(&reaping, shell.child, [reader, shell.writer]);
        });
        *self.reaper.lock().unwrap() = Some(reaper);
    }

    /// Say that no shell could be started here, and leave the terminal as the
    /// one thing that can say why.
    ///
    /// **Not a failed call**, on either of the two paths that reach it. The
    /// contract's `TerminalError` union has no class for "no shell could be
    /// started", so a refusal would tell the developer that the call broke
    /// rather than what went wrong.
    ///
    /// The message goes into the scrollback as well as into an event, because
    /// on the opening path the event has nowhere to go — nothing can be
    /// attached to a terminal that did not exist a moment ago, so the only
    /// reader it will ever have is whoever attaches next. The pane is also
    /// where the developer is looking.
    fn failed_to_start(&self, why: &str) {
        let mut state = self.state.lock().unwrap();
        state.status = Status::Error;
        state.pid = None;
        state.history = format!("[terminal] {why}\r\n");
        self.publish(&mut state, |sequence| {
            self.event(sequence, json!({"type": "error", "message": why}))
        });
    }

    /// Publish one event, numbered, with the state lock held.
    ///
    /// The lock is the point rather than an implementation detail. A snapshot
    /// reports the sequence its scrollback goes up to, and
    /// [`Terminals::attach`] drops any event at or below it — so an event
    /// numbered outside the lock that appended its text inside it could be both
    /// in a snapshot and above its watermark, and the client would show the
    /// same bytes twice.
    fn publish(&self, state: &mut State, event: impl FnOnce(u64) -> Value) {
        state.sequence += 1;
        state.updated_at = now_iso();
        // `send` on a broadcast channel never blocks — it drops the oldest
        // value when the buffer is full, and a subscriber that lags is resent a
        // snapshot instead. So this cannot stall the reader under the lock.
        let _ = self.events.send(event(state.sequence));
    }

    /// The identity every event on this stream carries.
    fn event(&self, sequence: u64, mut body: Value) -> Value {
        body["threadId"] = json!(self.thread_id);
        body["terminalId"] = json!(self.terminal_id);
        body["sequence"] = json!(sequence);
        body
    }

    /// `TerminalSessionSnapshot`. Every key the contract declares is present: a
    /// missing one fails the client's decode, and the pane then shows nothing
    /// rather than something slightly wrong.
    fn snapshot(&self, state: &State) -> Value {
        json!({
            "threadId": self.thread_id,
            "terminalId": self.terminal_id,
            "cwd": state.cwd,
            "worktreePath": state.worktree_path,
            "status": state.status.as_str(),
            "pid": state.pid,
            "history": state.history,
            "exitCode": state.exit_code,
            // Always null, and the contract allows it. Windows has no signals
            // and `portable_pty::ExitStatus` does not distinguish one, so a
            // field for it here would be a place to store something this build
            // can never learn.
            "exitSignal": Value::Null,
            "label": label(&self.terminal_id),
            "updatedAt": state.updated_at,
            "sequence": state.sequence,
        })
    }

    /// Put this terminal on the list, or replace what the list already says
    /// about it. The client folds these by replacing the entry with a matching
    /// id, so one shape covers both.
    fn announce(&self, state: &State) {
        let _ = self
            .metadata
            .send(json!({"type": "upsert", "terminal": self.summary(state)}));
    }

    /// Take this terminal off the list. The counterpart to
    /// [`Session::announce`], and the only one of the two that is final.
    fn withdraw(&self) {
        let _ = self.metadata.send(json!({
            "type": "remove",
            "threadId": self.thread_id,
            "terminalId": self.terminal_id,
        }));
    }

    /// `TerminalSummary` — the snapshot without what is in the terminal, which
    /// is what a list of them can afford to carry.
    fn summary(&self, state: &State) -> Value {
        json!({
            "threadId": self.thread_id,
            "terminalId": self.terminal_id,
            "cwd": state.cwd,
            "worktreePath": state.worktree_path,
            "status": state.status.as_str(),
            "pid": state.pid,
            "exitCode": state.exit_code,
            // Always null, and the contract allows it. Windows has no signals
            // and `portable_pty::ExitStatus` does not distinguish one, so a
            // field for it here would be a place to store something this build
            // can never learn.
            "exitSignal": Value::Null,
            "hasRunningSubprocess": false,
            "label": label(&self.terminal_id),
            "updatedAt": state.updated_at,
        })
    }

    /// Send keystrokes — and the answers to the shell's questions, which arrive
    /// by the same route because that is how an emulator answers.
    fn type_in(&self, state: &mut State, data: &str) -> Result<(), String> {
        // The contract caps one call's worth of input, and refusing what is over
        // it is the only honest answer: truncating would silently drop
        // keystrokes, which is worse than saying so.
        if data.len() > MAX_WRITE_BYTES {
            return Err(format!(
                "an input of {} bytes is more than the {MAX_WRITE_BYTES} a single write may carry",
                data.len()
            ));
        }

        // Cleared before the send rather than after, so that a full queue does
        // not leave a question re-asked forever.
        state.questions.clear();
        let Some(pty) = state.pty.as_ref() else {
            return Err("the terminal has no shell running in it".to_string());
        };

        pty.input
            .try_send(data.as_bytes().to_vec())
            .map_err(|error| format!("the terminal is not reading its input: {error}"))
    }

    /// Tell the pty how big it is now, if it is not that already.
    fn apply_size(&self, state: &mut State, cols: u16, rows: u16) -> Result<(), String> {
        if state.cols == cols && state.rows == rows {
            return Ok(());
        }
        let Some(pty) = state.pty.as_ref() else {
            return Err("the terminal has no shell running in it".to_string());
        };

        pty.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| error.to_string())?;
        state.cols = cols;
        state.rows = rows;
        state.updated_at = now_iso();
        Ok(())
    }

    /// Kill the shell and wait until nothing of this terminal is still running.
    ///
    /// Blocking, and it must be: the whole value of it is that it has finished
    /// when it returns.
    fn terminate(&self) {
        if let Some(pty) = self.state.lock().unwrap().pty.as_mut() {
            // Already gone is the ordinary case rather than a failure — the
            // reaper may have got there first.
            let _ = pty.killer.kill();
        }
        let reaper = self.reaper.lock().unwrap().take();
        if let Some(reaper) = reaper {
            let _ = reaper.join();
        }
    }
}

/// A started shell, before anything is driving it.
struct Shell {
    child: Box<dyn Child + Send + Sync>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    master: Box<dyn MasterPty + Send>,
    output: Box<dyn Read + Send>,
    input: std::sync::mpsc::SyncSender<Vec<u8>>,
    writer: JoinHandle<()>,
    pid: Option<u32>,
}

/// Start a shell in `cwd`, trying `candidates` in order.
///
/// Returns the last failure's message when none of them starts, naming every
/// one that was tried — "no shell was found" is only actionable next to *which*.
/// The list is an argument rather than a read of the environment so that the
/// case this cannot otherwise reach — a machine with no shell at all — is one a
/// test can ask for.
///
/// The size is an argument rather than read off `call` because the call does not
/// always carry one: what an open or a restart that said nothing about the size
/// gets is the terminal's own, and resolving that is the caller's business.
fn start_shell(
    call: &Open,
    cols: u16,
    rows: u16,
    candidates: &[Candidate],
) -> Result<Shell, String> {
    let mut attempted = Vec::new();
    let mut last = String::from("no shell was configured for this platform");

    for candidate in candidates {
        attempted.push(candidate.program.clone());
        match spawn(call, cols, rows, candidate) {
            Ok(shell) => return Ok(shell),
            Err(why) => last = why,
        }
    }

    Err(format!(
        "No shell could be started for this terminal. Tried {}. The last attempt failed: {last}",
        attempted.join(", ")
    ))
}

fn spawn(call: &Open, cols: u16, rows: u16, candidate: &Candidate) -> Result<Shell, String> {
    let pair = portable_pty::native_pty_system()
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|error| error.to_string())?;

    let mut command = CommandBuilder::new(&candidate.program);
    for argument in &candidate.arguments {
        command.arg(argument);
    }
    command.cwd(&call.cwd);
    // A shell that does not know what kind of terminal it is talking to will
    // not use colour. The emulator on the other end is an xterm, so say so —
    // and let the client override it, because it is its own terminal.
    if !call.env.contains_key("TERM") {
        command.env("TERM", "xterm-256color");
    }
    for (name, value) in &call.env {
        command.env(name, value);
    }

    let child = pair
        .slave
        .spawn_command(command)
        .map_err(|error| format!("{}: {error}", candidate.program))?;
    // Released immediately: while this end is open the pty has a writer that is
    // not the shell, and nothing downstream would ever see the console close.
    drop(pair.slave);

    let output = pair
        .master
        .try_clone_reader()
        .map_err(|error| error.to_string())?;
    let mut writer = pair
        .master
        .take_writer()
        .map_err(|error| error.to_string())?;
    let killer = child.clone_killer();
    let pid = child.process_id();

    let (input, queued) = std::sync::mpsc::sync_channel::<Vec<u8>>(INPUT_QUEUE);
    let writer = std::thread::spawn(move || {
        while let Ok(bytes) = queued.recv() {
            // A shell that has stopped reading makes this block, which is the
            // correct thing for a *thread of its own* to do: the queue in front
            // of it is what keeps the failure bounded and visible.
            if writer.write_all(&bytes).is_err() || writer.flush().is_err() {
                return;
            }
        }
    });

    Ok(Shell {
        child,
        killer,
        master: pair.master,
        output,
        input,
        writer,
        pid,
    })
}

/// Read the pty until it closes, publishing what it said.
fn read_output(session: &Arc<Session>, mut output: Box<dyn Read + Send>) {
    let mut buffer = [0u8; READ_BUFFER];
    let mut undecoded = Vec::new();

    loop {
        let read = match output.read(&mut buffer) {
            Ok(0) => return,
            Ok(read) => read,
            // A pty that has gone reads as an error rather than as EOF on some
            // platforms; both mean the same thing here.
            Err(_) => return,
        };

        undecoded.extend_from_slice(&buffer[..read]);
        let data = take_text(&mut undecoded);
        if data.is_empty() {
            continue;
        }

        // Reborrowed out of the guard so the two buffers below can be borrowed
        // separately: they are distinct fields, but the compiler cannot see
        // that through a `Deref`.
        let mut guard = session.state.lock().unwrap();
        let state = &mut *guard;
        // The scrollback grows under the same lock that numbers the event, so
        // a snapshot can never carry text the event beside it also carries.
        let kept = visible(&mut state.pending, &data, &mut state.questions);
        state.history.push_str(&kept);
        keep_last(&mut state.history, MAX_HISTORY_BYTES);
        // Bounded for the same reason and by a much smaller number: a program
        // asking in a loop with nothing attached would otherwise accumulate
        // every question it ever put, and the most recent is the one still
        // worth answering.
        keep_last(&mut state.questions, MAX_QUESTION_BYTES);
        session.publish(state, |sequence| {
            session.event(sequence, json!({"type": "output", "data": data}))
        });
    }
}

/// Wait for the shell, close the pty behind it, and say that it went.
///
/// The order is the whole content of this function. Closing the pty is what
/// lets the reader finish; joining the reader is what puts the shell's last
/// line of output in front of the exit that ended it.
fn reap(session: &Arc<Session>, mut child: Box<dyn Child + Send + Sync>, threads: [JoinHandle<()>; 2]) {
    // A wait that itself failed still means the child is gone; what is not
    // known is how it went, and a null exit code is the contract's way of
    // saying exactly that.
    let code = child.wait().ok().map(|status| i64::from(status.exit_code()));

    // Dropping the pty closes the console, which is what closes the output
    // pipe. On Windows it is the only thing that does — a shell exiting leaves
    // ConPTY holding it open, so without this the reader below would be joined
    // forever — and it closes the input queue, which is what lets the writer
    // thread come back.
    session.state.lock().unwrap().pty = None;
    for thread in threads {
        let _ = thread.join();
    }

    let mut state = session.state.lock().unwrap();
    state.status = Status::Exited;
    state.pid = None;
    state.exit_code = code;
    session.publish(&mut state, |sequence| {
        session.event(
            sequence,
            json!({"type": "exited", "exitCode": code, "exitSignal": Value::Null}),
        )
    });
    // The list hears about it too. A client watching the terminal list rather
    // than the terminal — which is how a tab knows to stop looking busy — has
    // no other way to learn that the shell has gone.
    session.announce(&state);
}

// ---------------------------------------------------------------------------
// Which shell
// ---------------------------------------------------------------------------

/// One shell worth trying, and the arguments it needs to be pleasant.
struct Candidate {
    program: String,
    arguments: Vec<String>,
}

impl Candidate {
    fn plain(program: impl Into<String>) -> Candidate {
        let program = program.into();
        // Both PowerShells print a copyright banner into a pane the developer
        // asked for a prompt in. Upstream passes the same flag.
        let arguments = match basename(&program).to_ascii_lowercase().as_str() {
            "pwsh.exe" | "powershell.exe" | "pwsh" | "powershell" => vec!["-NoLogo".to_string()],
            _ => Vec::new(),
        };
        Candidate { program, arguments }
    }
}

/// The shells to try, in order, for a session with this environment.
///
/// **The session's own environment is consulted first, and that is a deliberate
/// extension of what upstream does.** Upstream reads `SHELL` and `ComSpec` off
/// the *server process*; laplus reads them off the session as well, because
/// those two variables are exactly the platform's conventional way of naming
/// the command interpreter and the client already sends an environment for the
/// shell it is opening. No capability is gained by it — a client that can open
/// a terminal can already run any program by typing its name — and it is what
/// lets a test drive a shell it chose rather than whichever one the machine
/// running the suite happens to prefer.
fn shell_candidates(env: &BTreeMap<String, String>) -> Vec<Candidate> {
    let named = |name: &str| {
        env.get(name)
            .cloned()
            .or_else(|| std::env::var(name).ok())
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    };

    let mut candidates: Vec<Candidate> = Vec::new();
    let mut add = |program: Option<String>| {
        if let Some(program) = program {
            if !candidates.iter().any(|seen| seen.program == program) {
                candidates.push(Candidate::plain(program));
            }
        }
    };

    if cfg!(windows) {
        let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".to_string());
        add(named("ComSpec"));
        add(Some("pwsh.exe".to_string()));
        add(Some(format!(
            "{system_root}\\System32\\WindowsPowerShell\\v1.0\\powershell.exe"
        )));
        add(Some("powershell.exe".to_string()));
        add(Some(format!("{system_root}\\System32\\cmd.exe")));
        add(Some("cmd.exe".to_string()));
    } else {
        add(named("SHELL"));
        for fallback in ["/bin/zsh", "/bin/bash", "/bin/sh", "sh"] {
            add(Some(fallback.to_string()));
        }
    }

    candidates
}

fn basename(program: &str) -> String {
    program
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(program)
        .to_string()
}

/// The tab's caption, as `getTerminalLabel` in the reused UI computes it. The
/// server sends one because the contract says it does; sending a *different*
/// one from the client's own fallback would make a tab rename itself the moment
/// the list arrived.
fn label(terminal_id: &str) -> String {
    let digits = terminal_id
        .strip_prefix("term-")
        .or_else(|| terminal_id.strip_prefix("terminal-"))
        .or_else(|| terminal_id.strip_prefix("Term-"))
        .or_else(|| terminal_id.strip_prefix("Terminal-"));

    match digits {
        Some(digits) if !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()) => {
            format!("Terminal {digits}")
        }
        _ => terminal_id.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Bytes to scrollback
// ---------------------------------------------------------------------------

/// Everything decodable in `buffer`, leaving behind only a character whose
/// remaining bytes have not arrived.
///
/// A pty hands over whatever is available, so a multi-byte character is
/// routinely split across two reads. Decoding each read on its own would turn
/// the halves into two replacement characters — permanently, since what is sent
/// to the client cannot be taken back. Bytes that are not the start of anything
/// *are* replaced, because no later read can make them valid.
fn take_text(buffer: &mut Vec<u8>) -> String {
    let mut text = String::new();
    loop {
        match std::str::from_utf8(buffer) {
            Ok(all) => {
                text.push_str(all);
                buffer.clear();
                return text;
            }
            Err(error) => {
                let valid = error.valid_up_to();
                text.push_str(std::str::from_utf8(&buffer[..valid]).unwrap_or_default());
                match error.error_len() {
                    None => {
                        buffer.drain(..valid);
                        return text;
                    }
                    Some(bad) => {
                        text.push(char::REPLACEMENT_CHARACTER);
                        buffer.drain(..valid + bad);
                    }
                }
            }
        }
    }
}

/// Drop the oldest of `text` once there is more than `most` bytes of it, at a
/// character boundary.
///
/// The two things a terminal accumulates without being asked — its scrollback
/// and the questions nobody has answered — are bounded the same way and by the
/// same rule: what is worth keeping is what happened last.
fn keep_last(text: &mut String, most: usize) {
    if text.len() <= most {
        return;
    }
    let mut start = text.len() - most;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    text.replace_range(..start, "");
}

/// The part of `data` worth keeping in scrollback: everything except the
/// sequences that ask the emulator a question.
///
/// Scrollback is replayed into a live emulator, so a query in it is asked
/// again — and answered, to a shell that is not waiting. Three families are
/// therefore dropped and nothing else is:
///
/// - `ESC [ … n` — device status, which is the `ESC [ 6 n` ConPTY opens with;
/// - `ESC [ … R` — the cursor-position report itself, if it is ever echoed;
/// - `ESC [ … c` — device attributes;
/// - `ESC ] 10;? …`, `11`, `12` — the OSC colour queries.
///
/// Everything else survives byte for byte, which is what makes a reattached
/// pane look like the one it replaced rather than like a plain-text transcript
/// of it.
///
/// `pending` carries a sequence that was split across two reads. It is a
/// prefix of a control sequence and never ordinary text, so it is not lost by
/// being held back — only delayed until the rest of it arrives.
///
/// What is dropped is appended to `asked` rather than discarded, because a
/// question the shell is waiting on has to reach whoever attaches next. See the
/// module documentation.
///
/// Not handled: the 8-bit `CSI` (U+009B) form. It cannot reach here as a
/// control byte — a lone `0x9b` is not UTF-8 and has already become a
/// replacement character — and nothing on either platform emits it encoded.
fn visible(pending: &mut String, data: &str, asked: &mut String) -> String {
    let input = format!("{pending}{data}");
    pending.clear();

    let bytes = input.as_bytes();
    let mut kept = String::with_capacity(input.len());
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index] != 0x1b {
            let start = index;
            while index < bytes.len() && bytes[index] != 0x1b {
                index += 1;
            }
            kept.push_str(&input[start..index]);
            continue;
        }

        let Some(&introducer) = bytes.get(index + 1) else {
            pending.push_str(&input[index..]);
            return kept;
        };

        match introducer {
            // CSI: parameters and intermediates, then a final byte.
            b'[' => {
                let Some(end) = bytes[index + 2..]
                    .iter()
                    .position(|byte| (0x40..=0x7e).contains(byte))
                    .map(|offset| index + 2 + offset)
                else {
                    pending.push_str(&input[index..]);
                    return kept;
                };
                match is_a_question(&input[index + 2..end], bytes[end]) {
                    true => asked.push_str(&input[index..=end]),
                    false => kept.push_str(&input[index..=end]),
                }
                index = end + 1;
            }
            // OSC, DCS, PM, APC: a string, then a terminator.
            b']' | b'P' | b'^' | b'_' => {
                let Some(end) = string_terminator(bytes, index + 2) else {
                    pending.push_str(&input[index..]);
                    return kept;
                };
                let content = strip_terminator(&input[index + 2..end]);
                match introducer == b']' && is_a_colour_question(content) {
                    true => asked.push_str(&input[index..end]),
                    false => kept.push_str(&input[index..end]),
                }
                index = end;
            }
            // Everything else: optional intermediates, then one final byte.
            _ => {
                let mut cursor = index + 1;
                while cursor < bytes.len() && (0x20..=0x2f).contains(&bytes[cursor]) {
                    cursor += 1;
                }
                if cursor >= bytes.len() {
                    pending.push_str(&input[index..]);
                    return kept;
                }
                let end = match (0x30..=0x7e).contains(&bytes[cursor]) {
                    true => cursor + 1,
                    // Not a sequence at all. Keep the escape and carry on from
                    // the byte after it rather than swallowing what follows.
                    false => index + 1,
                };
                kept.push_str(&input[index..end]);
                index = end;
            }
        }
    }

    kept
}

/// Does this CSI sequence ask the emulator something?
fn is_a_question(body: &str, final_byte: u8) -> bool {
    match final_byte {
        b'n' => true,
        b'R' => body
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b';' || byte == b'?'),
        b'c' => body
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b';' || byte == b'?' || byte == b'>'),
        _ => false,
    }
}

/// `OSC 10/11/12` asking for a colour rather than setting one.
fn is_a_colour_question(content: &str) -> bool {
    let Some((number, value)) = content.split_once(';') else {
        return false;
    };
    matches!(number, "10" | "11" | "12") && (value.starts_with('?') || value.starts_with("rgb:"))
}

/// Where the string in an OSC/DCS/PM/APC sequence ends, terminator included.
fn string_terminator(bytes: &[u8], from: usize) -> Option<usize> {
    let mut index = from;
    while index < bytes.len() {
        match bytes[index] {
            0x07 => return Some(index + 1),
            0x1b if bytes.get(index + 1) == Some(&b'\\') => return Some(index + 2),
            _ => index += 1,
        }
    }
    None
}

fn strip_terminator(value: &str) -> &str {
    value
        .strip_suffix("\u{1b}\\")
        .or_else(|| value.strip_suffix('\u{07}'))
        .unwrap_or(value)
}

// ---------------------------------------------------------------------------
// Calls
// ---------------------------------------------------------------------------

/// A validated `terminal.open`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Open {
    thread_id: String,
    terminal_id: String,
    cwd: String,
    worktree_path: Option<String>,
    /// The size the client asked for, or `None` because it did not ask.
    ///
    /// The distinction is load-bearing rather than tidy. Both `terminal.open`
    /// and `terminal.attach` make the size optional, and the pane sending one of
    /// those is not always the pane that opened the terminal — so a missing size
    /// resolved to the *default* rather than to the terminal's own would shrink
    /// somebody else's terminal to 120x30 every time a second client mounted a
    /// pane on it. See [`Open::size`].
    cols: Option<u16>,
    rows: Option<u16>,
    env: BTreeMap<String, String>,
}

impl Open {
    pub fn read(payload: &Value) -> Result<Open, Value> {
        let thread_id = identifier(payload, "threadId")?;
        let terminal_id = identifier(payload, "terminalId")?;
        let cwd = payload
            .get("cwd")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();

        // A blank working directory is not a missing field to be defaulted: it
        // means the process's own directory, which is the server's install and
        // not anybody's project.
        if cwd.is_empty() {
            return Err(cwd_error("TerminalCwdNotFoundError", &cwd));
        }

        Ok(Open {
            thread_id,
            terminal_id,
            cwd,
            worktree_path: optional_text(payload, "worktreePath"),
            cols: size(payload, "cols", MAX_COLS),
            rows: size(payload, "rows", MAX_ROWS),
            env: environment(payload),
        })
    }

    /// The size this call asks the terminal to be, given the size it is now.
    ///
    /// `current` is the terminal's own for a call about one that exists, and the
    /// contract's defaults for one that does not. Either way what a call that
    /// said nothing about the size means is "leave it as it is".
    fn size(&self, current_cols: u16, current_rows: u16) -> (u16, u16) {
        (
            self.cols.unwrap_or(current_cols),
            self.rows.unwrap_or(current_rows),
        )
    }

    fn key(&self) -> Key {
        key(&self.thread_id, &self.terminal_id)
    }
}

/// A validated `terminal.attach`.
///
/// Carries an [`Open`] when the client sent enough to start one, because the
/// contract's attach input is an open input with `cwd` made optional — the UI
/// attaches and opens from two different places and neither can be sure it went
/// first.
///
/// The fallback is a `Result` rather than an `Option`, and the difference is a
/// diagnostic. `None` means the client was not offering to open one; `Some(Err)`
/// means it was, and the directory it named is not one. Only the second is worth
/// a message about a directory — and only if the terminal turns out not to be
/// there, because an attach to a *running* terminal has no business failing over
/// a `cwd` it is not going to use.
#[derive(Debug, Clone, PartialEq)]
pub struct Attach {
    thread_id: String,
    terminal_id: String,
    opening: Option<Result<Open, Value>>,
    /// The client asking that a terminal whose shell has gone be given another
    /// one, rather than attached to as it stands. Off unless it is said, which
    /// is what keeps an attach a read by default.
    restart_if_not_running: bool,
}

impl Attach {
    pub fn read(payload: &Value) -> Result<Attach, Value> {
        let thread_id = identifier(payload, "threadId")?;
        let terminal_id = identifier(payload, "terminalId")?;
        let offered = payload
            .get("cwd")
            .and_then(Value::as_str)
            .is_some_and(|cwd| !cwd.trim().is_empty());

        Ok(Attach {
            thread_id,
            terminal_id,
            opening: offered.then(|| Open::read(payload)),
            restart_if_not_running: payload
                .get("restartIfNotRunning")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
    }

    /// What this attach should put back in the terminal it found, if it should
    /// put anything back at all.
    ///
    /// Three things have to be true together: the client asked for it, it sent
    /// enough to start a shell with, and there is no shell there to displace.
    /// The third is the one that makes this safe — `restartIfNotRunning` means
    /// what it says, so an attach that arrives while the developer's shell is
    /// working never touches it.
    fn shell_to_put_back(&self, session: &Session) -> Option<&Open> {
        if !self.restart_if_not_running {
            return None;
        }
        let Some(Ok(open)) = &self.opening else {
            return None;
        };
        match session.state.lock().unwrap().status {
            Status::Running => None,
            Status::Exited | Status::Error => Some(open),
        }
    }

    fn key(&self) -> Key {
        key(&self.thread_id, &self.terminal_id)
    }
}

/// A validated `terminal.clear`. The contract's input is a terminal's two ids
/// and nothing else — clearing says nothing about what should be in the
/// terminal afterwards, only about what should not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clear {
    thread_id: String,
    terminal_id: String,
}

impl Clear {
    pub fn read(payload: &Value) -> Result<Clear, Value> {
        Ok(Clear {
            thread_id: identifier(payload, "threadId")?,
            terminal_id: identifier(payload, "terminalId")?,
        })
    }
}

/// A validated `terminal.restart`.
///
/// The contract's input is an open's with the size *required* rather than
/// optional. A missing one is defaulted rather than refused all the same,
/// because the two calls start the same shell the same way and a restart that
/// was pickier about a number the pane corrects a frame later would only be
/// pickier, not safer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Restart {
    opening: Open,
}

impl Restart {
    pub fn read(payload: &Value) -> Result<Restart, Value> {
        Ok(Restart {
            opening: Open::read(payload)?,
        })
    }
}

/// A validated `terminal.close`.
///
/// The one call on this wire that can name a terminal or decline to. Without a
/// `terminalId` it means every terminal on the thread, which is what the client
/// sends when a whole conversation goes away rather than one pane.
///
/// The contract's `deleteHistory` is deliberately not carried; see the module
/// documentation for why there is nothing for it to select between.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Close {
    thread_id: String,
    terminal_id: Option<String>,
}

impl Close {
    pub fn read(payload: &Value) -> Result<Close, Value> {
        let thread_id = identifier(payload, "threadId")?;
        // A *blank* `terminalId` is refused rather than read as an absent one.
        // Absence means "every terminal on this thread", so a payload that
        // meant to name one and sent an empty string would reap the lot — the
        // one place on this wire where being lenient about a blank field
        // destroys something.
        let terminal_id = match payload.get("terminalId") {
            None | Some(Value::Null) => None,
            Some(_) => Some(identifier(payload, "terminalId")?),
        };

        Ok(Close {
            thread_id,
            terminal_id,
        })
    }
}

/// A validated `terminal.write`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteInput {
    thread_id: String,
    terminal_id: String,
    data: String,
}

impl WriteInput {
    pub fn read(payload: &Value) -> Result<WriteInput, Value> {
        let thread_id = identifier(payload, "threadId")?;
        let terminal_id = identifier(payload, "terminalId")?;
        // Not trimmed, and not refused when empty. What arrives here is
        // keystrokes — a space is a keystroke, and so is a lone carriage
        // return.
        let data = payload
            .get("data")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();

        Ok(WriteInput {
            thread_id,
            terminal_id,
            data,
        })
    }
}

/// A validated `terminal.resize`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resize {
    thread_id: String,
    terminal_id: String,
    cols: u16,
    rows: u16,
}

impl Resize {
    pub fn read(payload: &Value) -> Result<Resize, Value> {
        let thread_id = identifier(payload, "threadId")?;
        let terminal_id = identifier(payload, "terminalId")?;
        Ok(Resize {
            thread_id,
            terminal_id,
            // Required by the contract, unlike an open's — a resize that named
            // no size would be a call with nothing in it — so the defaults here
            // are what a malformed payload gets rather than a meaning.
            cols: size(payload, "cols", MAX_COLS).unwrap_or(DEFAULT_COLS),
            rows: size(payload, "rows", MAX_ROWS).unwrap_or(DEFAULT_ROWS),
        })
    }
}

/// A `threadId` or `terminalId` from a payload.
///
/// A blank one is refused as a *lookup* failure rather than as a malformed
/// request, and that is the honest reading: the contract types both as trimmed
/// and non-empty, so a blank one names no terminal — which is what
/// `TerminalSessionLookupError` says. It is also the only error in the method's
/// declared union that a client can decode without inventing fields.
fn identifier(payload: &Value, field: &str) -> Result<String, Value> {
    let value = payload
        .get(field)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();

    if value.is_empty() {
        return Err(lookup_error(
            payload
                .get("threadId")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            payload
                .get("terminalId")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        ));
    }
    Ok(value)
}

fn optional_text(payload: &Value, field: &str) -> Option<String> {
    payload
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// A `cols` or `rows`, clamped to what the contract allows, or `None` because
/// the client did not send one.
///
/// Clamped rather than refused: an out-of-range size is a pane that measured
/// itself wrongly, and answering it with a failure would leave the developer
/// with a terminal that will not resize at all. Absent is *not* out of range —
/// what that means is the caller's to decide, and the two callers decide
/// differently. See [`Open::size`].
fn size(payload: &Value, field: &str, most: u16) -> Option<u16> {
    payload
        .get(field)
        .and_then(Value::as_u64)
        .map(|value| value.clamp(1, u64::from(most)) as u16)
}

/// The environment a client asked for, keeping only names that are names.
///
/// The contract's own pattern (`^[A-Za-z_][A-Za-z0-9_]*$`). Silently dropping
/// what does not match rather than refusing the call: a variable this server
/// will not set is not a reason to refuse the developer a terminal.
fn environment(payload: &Value) -> BTreeMap<String, String> {
    let Some(given) = payload.get("env").and_then(Value::as_object) else {
        return BTreeMap::new();
    };

    given
        .iter()
        .filter(|(name, _)| is_an_env_name(name))
        .filter_map(|(name, value)| {
            value
                .as_str()
                .map(|value| (name.clone(), value.to_string()))
        })
        .collect()
}

fn is_an_env_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

/// Is this a directory a terminal can be opened in?
///
/// Three distinct answers, because the contract has three distinct errors for
/// them and they have three different fixes: it is not there, it is there and
/// is a file, or it could not be looked at.
fn check_cwd(cwd: &str) -> Result<(), Value> {
    match std::fs::metadata(cwd) {
        Ok(metadata) if metadata.is_dir() => Ok(()),
        Ok(_) => Err(cwd_error("TerminalCwdNotDirectoryError", cwd)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Err(cwd_error("TerminalCwdNotFoundError", cwd))
        }
        Err(error) => Err(json!({
            "_tag": "TerminalCwdStatError",
            "cwd": cwd,
            "cause": error.to_string(),
        })),
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------
//
// Each of these is one of the classes in the contract's `TerminalError` union,
// carrying its declared fields and nothing else. **No `message`**: every one of
// them defines `message` as a getter over those fields, so the reference server
// sends none and the client computes it — the same asymmetry
// `Outcome::expect_declared` in the test harness documents.

fn cwd_error(tag: &str, cwd: &str) -> Value {
    json!({"_tag": tag, "cwd": cwd})
}

fn lookup_error(thread_id: &str, terminal_id: &str) -> Value {
    json!({
        "_tag": "TerminalSessionLookupError",
        "threadId": thread_id,
        "terminalId": terminal_id,
    })
}

fn not_running_error(thread_id: &str, terminal_id: &str) -> Value {
    json!({
        "_tag": "TerminalNotRunningError",
        "threadId": thread_id,
        "terminalId": terminal_id,
    })
}

fn write_error(thread_id: &str, terminal_id: &str, pid: u32, cause: &str) -> Value {
    json!({
        "_tag": "TerminalWriteError",
        "threadId": thread_id,
        "terminalId": terminal_id,
        "terminalPid": pid,
        "cause": cause,
    })
}

fn resize_error(
    thread_id: &str,
    terminal_id: &str,
    pid: u32,
    cols: u16,
    rows: u16,
    cause: &str,
) -> Value {
    json!({
        "_tag": "TerminalResizeError",
        "threadId": thread_id,
        "terminalId": terminal_id,
        "terminalPid": pid,
        "cols": cols,
        "rows": rows,
        "cause": cause,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The escape ConPTY opens with, which a terminal must answer and
    /// scrollback must not repeat.
    const CURSOR_QUERY: &str = "\u{1b}[6n";

    fn sanitised(chunks: &[&str]) -> String {
        let mut pending = String::new();
        let mut asked = String::new();
        let mut kept = String::new();
        for chunk in chunks {
            kept.push_str(&visible(&mut pending, chunk, &mut asked));
        }
        kept
    }

    /// [`visible`], with the questions it took out.
    fn split(chunk: &str) -> (String, String) {
        let mut pending = String::new();
        let mut asked = String::new();
        let kept = visible(&mut pending, chunk, &mut asked);
        (kept, asked)
    }

    /// The rule the whole scrollback design rests on: what makes a pane look
    /// right survives, and only the questions are dropped. A stripped colour
    /// sequence would make every reattached terminal monochrome.
    #[test]
    fn scrollback_keeps_what_renders_and_drops_what_asks() {
        assert_eq!(
            sanitised(&["\u{1b}[31mred\u{1b}[0m plain"]),
            "\u{1b}[31mred\u{1b}[0m plain"
        );
        assert_eq!(sanitised(&["\u{1b}[2;5Hmoved"]), "\u{1b}[2;5Hmoved");
        assert_eq!(sanitised(&["\u{1b}[?25lhidden\u{1b}[?25h"]), "\u{1b}[?25lhidden\u{1b}[?25h");

        // The queries, each of which would be answered into a shell that is not
        // asking if it were replayed.
        assert_eq!(sanitised(&[&format!("before{CURSOR_QUERY}after")]), "beforeafter");
        assert_eq!(sanitised(&["a\u{1b}[6;12Rb"]), "ab");
        assert_eq!(sanitised(&["a\u{1b}[>0cb"]), "ab");
        assert_eq!(sanitised(&["a\u{1b}]11;?\u{07}b"]), "ab");

        // …but setting a colour is not asking for one.
        assert_eq!(
            sanitised(&["\u{1b}]11;#000000\u{07}"]),
            "\u{1b}]11;#000000\u{07}"
        );
        // …and a window title is kept, which is what upstream does too.
        assert_eq!(
            sanitised(&["\u{1b}]0;a title\u{07}rest"]),
            "\u{1b}]0;a title\u{07}rest"
        );
    }

    /// A pty hands over whatever is available, so a sequence routinely arrives
    /// in two pieces. Held back rather than emitted half-formed, because a half
    /// sequence in scrollback is a half sequence replayed into an emulator.
    #[test]
    fn a_sequence_split_across_two_reads_is_held_until_it_is_whole() {
        let mut pending = String::new();
        let mut asked = String::new();
        assert_eq!(visible(&mut pending, "text\u{1b}[3", &mut asked), "text");
        assert_eq!(pending, "\u{1b}[3");
        assert_eq!(visible(&mut pending, "1mred", &mut asked), "\u{1b}[31mred");
        assert!(pending.is_empty());

        // And the same for a query, which must still be recognised as one when
        // its two halves are put back together.
        assert_eq!(visible(&mut pending, "a\u{1b}[", &mut asked), "a");
        assert_eq!(visible(&mut pending, "6nb", &mut asked), "b");
        assert_eq!(asked, CURSOR_QUERY);

        // A lone escape at the very end of a read is a sequence that has not
        // started yet.
        assert_eq!(visible(&mut pending, "c\u{1b}", &mut asked), "c");
        assert_eq!(pending, "\u{1b}");
        assert_eq!(visible(&mut pending, "=d", &mut asked), "\u{1b}=d");
    }

    /// A question is not thrown away when it is taken out of scrollback — it is
    /// kept, because the shell is still waiting on it and the client that
    /// attaches next is the only thing that can answer. Without this the very
    /// first thing a shell says is lost and it never prints a prompt.
    #[test]
    fn a_question_taken_out_of_scrollback_is_kept_to_be_asked_again() {
        assert_eq!(
            split(&format!("before{CURSOR_QUERY}after")),
            ("beforeafter".to_string(), CURSOR_QUERY.to_string())
        );
        assert_eq!(
            split("\u{1b}]11;?\u{07}\u{1b}[>0c"),
            (String::new(), "\u{1b}]11;?\u{07}\u{1b}[>0c".to_string())
        );
        // Nothing was asked, so nothing is remembered.
        assert_eq!(split("\u{1b}[31mred").1, "");
    }

    /// An escape that introduces nothing must not swallow the text after it.
    #[test]
    fn a_malformed_escape_does_not_eat_the_line() {
        assert_eq!(sanitised(&["\u{1b}\u{7f}rest"]), "\u{1b}\u{7f}rest");
    }

    /// Both halves of a character split across two reads are one character, not
    /// two replacements — and what is genuinely not UTF-8 is replaced rather
    /// than held forever.
    #[test]
    fn a_character_split_across_two_reads_survives_the_split() {
        let mut buffer = Vec::new();
        buffer.extend_from_slice("héllo".as_bytes());
        let tail = buffer.split_off(2); // between the two bytes of `é`
        assert_eq!(take_text(&mut buffer), "h");
        buffer.extend_from_slice(&tail);
        assert_eq!(take_text(&mut buffer), "éllo");
        assert!(buffer.is_empty());

        let mut invalid = vec![b'a', 0xff, b'b'];
        assert_eq!(take_text(&mut invalid), "a\u{fffd}b");
        assert!(invalid.is_empty());
    }

    /// Both of the things a terminal accumulates are bounded, at a character
    /// boundary, keeping what happened last. Scrollback's bound is the client's
    /// own number, so a client is never sent bytes it will immediately throw
    /// away.
    #[test]
    fn what_a_terminal_accumulates_is_capped_at_a_character_boundary() {
        let mut history = "é".repeat(MAX_HISTORY_BYTES);
        keep_last(&mut history, MAX_HISTORY_BYTES);
        assert!(history.len() <= MAX_HISTORY_BYTES);
        assert!(history.starts_with('é'), "the cap split a character");

        let mut questions = CURSOR_QUERY.repeat(MAX_QUESTION_BYTES);
        keep_last(&mut questions, MAX_QUESTION_BYTES);
        assert!(questions.len() <= MAX_QUESTION_BYTES);

        let mut short = "still here".to_string();
        keep_last(&mut short, MAX_HISTORY_BYTES);
        assert_eq!(short, "still here");
    }

    /// The caption the client would compute for itself. A server that sent a
    /// different one would make every tab rename itself when the list arrived.
    #[test]
    fn a_terminals_caption_matches_the_clients_own() {
        assert_eq!(label("term-1"), "Terminal 1");
        assert_eq!(label("terminal-12"), "Terminal 12");
        assert_eq!(label("scratch"), "scratch");
        assert_eq!(label("term-"), "term-");
    }

    /// The three refusals a working directory can earn, each with its own fix.
    #[test]
    fn a_working_directory_is_refused_by_what_is_wrong_with_it() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        assert!(check_cwd(&directory.path().to_string_lossy()).is_ok());

        let missing = directory.path().join("not-here");
        assert_eq!(
            check_cwd(&missing.to_string_lossy()).expect_err("no such directory")["_tag"],
            "TerminalCwdNotFoundError"
        );

        let file = directory.path().join("a-file.txt");
        std::fs::write(&file, "").expect("writes the file");
        assert_eq!(
            check_cwd(&file.to_string_lossy()).expect_err("not a directory")["_tag"],
            "TerminalCwdNotDirectoryError"
        );
    }

    /// The payload the UI sends, read. A size out of the contract's bounds is
    /// clamped rather than refused, and a size that is not there is *absent*
    /// rather than defaulted — the difference is what stops a second pane's
    /// bare open from shrinking a terminal somebody else is using.
    #[test]
    fn an_open_reads_its_payload_and_clamps_what_is_out_of_range() {
        let call = Open::read(&json!({
            "threadId": " thread-1 ",
            "terminalId": "term-1",
            "cwd": "C:\\work",
            "worktreePath": Value::Null,
            "cols": 100_000,
            "rows": 0,
            "env": {"SHELL": "/bin/sh", "not a name": "ignored", "NUMBER": 7},
        }))
        .expect("a well-formed open");

        assert_eq!(call.thread_id, "thread-1");
        assert_eq!(call.size(80, 24), (MAX_COLS, 1));
        assert_eq!(call.worktree_path, None);
        assert_eq!(
            call.env,
            BTreeMap::from([("SHELL".to_string(), "/bin/sh".to_string())])
        );

        let silent = Open::read(&json!({
            "threadId": "thread-1",
            "terminalId": "term-1",
            "cwd": "C:\\work",
        }))
        .expect("a well-formed open");
        // A terminal that does not exist yet gets the contract's defaults…
        assert_eq!(
            silent.size(DEFAULT_COLS, DEFAULT_ROWS),
            (DEFAULT_COLS, DEFAULT_ROWS)
        );
        // …and one that does keeps the size it already had, which is the half
        // that would resize somebody else's terminal if it were defaulted.
        assert_eq!(silent.size(200, 60), (200, 60));
    }

    /// A call that names no terminal is refused with the error that says so,
    /// rather than with one the client cannot decode.
    #[test]
    fn a_call_that_names_no_terminal_is_refused_as_a_lookup() {
        for payload in [
            json!({"terminalId": "term-1", "cwd": "C:\\work"}),
            json!({"threadId": "  ", "terminalId": "term-1", "cwd": "C:\\work"}),
            json!({"threadId": "thread-1", "cwd": "C:\\work"}),
        ] {
            let refusal = Open::read(&payload).expect_err("no terminal is named");
            assert_eq!(refusal["_tag"], "TerminalSessionLookupError");
            assert!(refusal["threadId"].is_string());
            assert!(refusal["terminalId"].is_string());
        }

        let refusal = Open::read(&json!({"threadId": "thread-1", "terminalId": "term-1"}))
            .expect_err("no working directory");
        assert_eq!(refusal["_tag"], "TerminalCwdNotFoundError");
    }

    /// An attach carries an open when the client sent enough for one, because
    /// the UI attaches and opens from two places and neither goes first — and
    /// it carries the *reason* when what the client sent is not enough, so that
    /// a directory that is not one is reported as that rather than as "no such
    /// terminal".
    #[test]
    fn an_attach_keeps_whether_it_could_open_and_why_not() {
        let bare = Attach::read(&json!({"threadId": "thread-1", "terminalId": "term-1"}))
            .expect("an attach needs only the two ids");
        assert!(bare.opening.is_none(), "no cwd is not an offer to open one");

        let complete = Attach::read(&json!({
            "threadId": "thread-1",
            "terminalId": "term-1",
            "cwd": "C:\\work",
        }))
        .expect("an attach that could open");
        assert!(matches!(complete.opening, Some(Ok(_))));

        // The client offered, and named something that could never be a
        // working directory. Which is a different fix from a missing terminal.
        let blank_id = Attach::read(&json!({
            "threadId": "thread-1",
            "terminalId": "term-1",
            "cwd": "C:\\work",
            "cols": "not a number",
        }))
        .expect("still a well-formed attach");
        assert!(matches!(blank_id.opening, Some(Ok(_))), "a bad size is clamped, not fatal");
    }

    /// A close names one terminal or declines to, and declining means the whole
    /// thread. A *blank* name is neither, and is refused — it is the one field
    /// on this wire where reading an empty string leniently would reap
    /// terminals the client did not ask about.
    #[test]
    fn a_close_that_names_no_terminal_means_the_whole_thread() {
        let one = Close::read(&json!({"threadId": "thread-1", "terminalId": "term-1"}))
            .expect("a close that names a terminal");
        assert_eq!(one.terminal_id.as_deref(), Some("term-1"));

        for whole_thread in [
            json!({"threadId": "thread-1"}),
            json!({"threadId": "thread-1", "terminalId": Value::Null}),
            json!({"threadId": "thread-1", "deleteHistory": true}),
        ] {
            let call = Close::read(&whole_thread).expect("a close that names no terminal");
            assert_eq!(call.terminal_id, None);
        }

        let refusal = Close::read(&json!({"threadId": "thread-1", "terminalId": "  "}))
            .expect_err("a blank terminal id names nothing");
        assert_eq!(refusal["_tag"], "TerminalSessionLookupError");
    }

    /// An attach only puts a shell back when it was asked to, and only into a
    /// terminal that has none. The default matters most: an attach is a read,
    /// and the reused UI sends one every time a pane mounts.
    #[test]
    fn an_attach_only_restarts_when_it_was_asked_to_and_there_is_nothing_running() {
        let opening = json!({
            "threadId": "thread-1",
            "terminalId": "term-1",
            "cwd": "C:\\work",
        });
        assert!(
            !Attach::read(&opening)
                .expect("a well-formed attach")
                .restart_if_not_running,
            "an attach that did not ask would replace a terminal it was only reading"
        );

        let asking = {
            let mut asking = opening.clone();
            asking["restartIfNotRunning"] = json!(true);
            Attach::read(&asking).expect("a well-formed attach")
        };
        let session = Session::new(
            &Open::read(&opening).expect("a well-formed open"),
            DEFAULT_COLS,
            DEFAULT_ROWS,
            broadcast::channel(BACKLOG).0,
        );

        // A new session is `running` until something says otherwise, which is
        // the case an attach must leave alone.
        assert!(asking.shell_to_put_back(&session).is_none());
        session.state.lock().unwrap().status = Status::Exited;
        assert!(asking.shell_to_put_back(&session).is_some());

        // …and asking without sending enough to start one with is not asking.
        let bare = Attach::read(&json!({
            "threadId": "thread-1",
            "terminalId": "term-1",
            "restartIfNotRunning": true,
        }))
        .expect("a well-formed attach");
        assert!(bare.shell_to_put_back(&session).is_none());
    }

    /// Clearing and restarting a terminal that is not there are both refused by
    /// name, and closing one is not refused at all — a pane is closed by a
    /// client that has already stopped showing it.
    #[test]
    fn the_lifecycle_calls_disagree_about_a_terminal_that_is_not_there() {
        let terminals = Terminals::new();
        let named = json!({"threadId": "thread-1", "terminalId": "term-1"});

        let clear = Clear::read(&named).expect("a well-formed clear");
        assert_eq!(
            terminals.clear(&clear).expect_err("no such terminal")["_tag"],
            "TerminalSessionLookupError"
        );

        let close = Close::read(&named).expect("a well-formed close");
        assert_eq!(
            terminals.close(&close).expect("closing nothing is not a failure"),
            Value::Null
        );

        // A restart names a directory, so what it refuses first is that.
        let restart = Restart::read(&json!({
            "threadId": "thread-1",
            "terminalId": "term-1",
            "cwd": "  ",
        }))
        .expect_err("a restart with nowhere to run");
        assert_eq!(restart["_tag"], "TerminalCwdNotFoundError");
    }

    /// An empty registry describes itself as one, and the shape is the captured
    /// one — `fixtures/socket-wire/04-streaming-subscription.ndjson` recorded
    /// exactly this value.
    #[test]
    fn an_empty_registry_describes_itself_the_captured_way() {
        let terminals = Terminals::new();
        assert_eq!(
            terminals.subscribe_metadata().describe(),
            vec![json!({"type": "snapshot", "terminals": []})]
        );
        assert_eq!(terminals.live(), 0);
    }

    /// Nothing to attach to, and nothing sent to make one with.
    #[test]
    fn attaching_to_a_terminal_that_is_not_there_is_refused() {
        let terminals = Terminals::new();
        let call = Attach::read(&json!({"threadId": "thread-1", "terminalId": "term-1"}))
            .expect("a well-formed attach");
        assert_eq!(
            terminals.attach(call).expect_err("no such terminal")["_tag"],
            "TerminalSessionLookupError"
        );
    }

    /// Writing and resizing name the same two failures, and they are different
    /// failures: there is no such terminal, or there is one and nothing is
    /// running in it.
    #[test]
    fn writing_to_a_terminal_that_is_not_there_is_refused() {
        let terminals = Terminals::new();
        let write = WriteInput::read(&json!({
            "threadId": "thread-1",
            "terminalId": "term-1",
            "data": "ls\r",
        }))
        .expect("a well-formed write");
        assert_eq!(
            terminals.write(&write).expect_err("no such terminal")["_tag"],
            "TerminalSessionLookupError"
        );

        let resize = Resize::read(&json!({
            "threadId": "thread-1",
            "terminalId": "term-1",
            "cols": 100,
            "rows": 40,
        }))
        .expect("a well-formed resize");
        assert_eq!(
            terminals.resize(&resize).expect_err("no such terminal")["_tag"],
            "TerminalSessionLookupError"
        );
    }

    /// A machine with no shell on it. Unreachable through the registry — every
    /// platform's fallbacks are named and one of them always exists where this
    /// suite runs — so the list is supplied here, which is the reason
    /// [`start_shell`] takes one.
    ///
    /// What is asserted is the *diagnostic*: it names every shell that was
    /// tried, because "no shell was found" without a list is not something a
    /// developer can act on.
    #[test]
    fn a_machine_with_no_shell_says_which_ones_it_looked_for() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let call = Open::read(&json!({
            "threadId": "thread-1",
            "terminalId": "term-1",
            "cwd": directory.path().to_string_lossy(),
        }))
        .expect("a well-formed open");

        let attempt = start_shell(
            &call,
            DEFAULT_COLS,
            DEFAULT_ROWS,
            &[
                Candidate::plain("not-a-shell-on-any-machine"),
                Candidate::plain("nor-is-this-one"),
            ],
        );
        let Err(why) = attempt else {
            panic!("a shell started from a name that is not one");
        };

        assert!(why.contains("not-a-shell-on-any-machine"), "{why}");
        assert!(why.contains("nor-is-this-one"), "{why}");
    }

    /// The two PowerShells print a copyright banner into a pane the developer
    /// asked for a prompt in; every other shell is started as it is.
    #[test]
    fn the_shells_that_need_quietening_get_the_flag() {
        assert_eq!(
            Candidate::plain("C:\\Program Files\\PowerShell\\7\\pwsh.exe").arguments,
            vec!["-NoLogo".to_string()]
        );
        assert!(Candidate::plain("C:\\Windows\\System32\\cmd.exe")
            .arguments
            .is_empty());
        assert!(Candidate::plain("/bin/bash").arguments.is_empty());
    }

    /// The session's own environment names the shell before the machine's does.
    /// The two variables consulted are the platform's conventional ones, which
    /// is what makes this an extension of the convention rather than an
    /// invention.
    #[test]
    fn the_sessions_environment_chooses_the_shell_first() {
        let asked = match cfg!(windows) {
            true => ("ComSpec", "C:\\somewhere\\chosen.exe"),
            false => ("SHELL", "/somewhere/chosen"),
        };
        let candidates = shell_candidates(&BTreeMap::from([
            (asked.0.to_string(), asked.1.to_string()),
        ]));

        assert_eq!(candidates[0].program, asked.1);
        assert!(
            candidates.len() > 1,
            "the platform's own shells are still there to fall back to"
        );
    }
}
