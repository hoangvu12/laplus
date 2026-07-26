//! Running a turn: the agent's NDJSON on one side, the thread's events on the
//! other.
//!
//! This is the join between the two protocols the crate keeps apart. Neither
//! side is implemented here — [`crate::protocol`] parses and folds what the
//! agent says, [`crate::threads`] holds what the UI reads, and [`crate::agent`]
//! owns the process — so what is left is the translation and the lifetime, and
//! that is deliberately all this file is.
//!
//! ## One long-lived driver, not one per turn
//!
//! A session is a task that owns an agent and a channel of prompts. Dispatching
//! a turn puts a prompt in the channel and returns; it never waits for a process
//! to exist, which is what lets the socket acknowledge the developer's message
//! immediately. The task starts the agent on its first prompt and then stays,
//! because the agent stays: `--input-format stream-json` means the CLI reads
//! turns until its stdin closes, and re-spawning per turn would throw away the
//! conversation the developer is having.
//!
//! Everything the task does after that is one loop over two sources — a line
//! from the agent, or another prompt — and the loop ends when the agent's output
//! does or when the channel closes. Both endings reap the child.
//!
//! ## The translation
//!
//! | The agent says | The thread publishes |
//! |---|---|
//! | `system`/`init` | an activity naming the model, the permission mode and the tool count |
//! | a text delta | `thread.message-sent` with `streaming: true` — the client appends it |
//! | a buffered `assistant` message's text | `thread.message-sent` with `streaming: false` — the client replaces with it |
//! | a `tool_use` block | a `tool.updated` activity naming the tool and its input |
//! | a `tool_result` block | a `tool.completed` activity, paired to it by the agent's own id |
//! | a `thinking` block | a `task.progress` activity, which the UI renders as thinking |
//! | a `control_request` asking to use a tool | an `approval.requested` activity, which is the client's pending-approval panel |
//! | `result` | an activity carrying the duration and the cost, and the session is ready again |
//!
//! and two rows that go the other way: a `thread.approval.respond` command
//! becomes a `control_response` on the agent's stdin and an `approval.resolved`
//! activity that closes the panel, and a `thread.turn.interrupt` becomes a
//! `control_request` on that stdin, a `thread.turn-interrupt-requested` event
//! that settles the turn, and a `turn.interrupted` activity that records it.
//!
//! The second and third rows *are* accumulate-and-reconcile. Nothing decides
//! between them here: [`crate::protocol::Folded`] says which of the two a line
//! was, and the rule that makes the buffered message authoritative lives in the
//! reducer the golden files check.
//!
//! Nothing in that table makes the session `running`, and that is deliberate:
//! `init` is printed once for the whole conversation, so the transition belongs
//! to the prompt being sent rather than to anything the agent says.
//!
//! ## Tool use is read off the buffered messages, not off the stream
//!
//! The stream announces a tool call twice — once as a `content_block_start` with
//! the arguments still arriving, and again as the buffered `assistant` message
//! once the block has closed — and this file reads the second. That is the same
//! reconciliation rule the text follows, applied to the one thing it matters most
//! for: deltas are best-effort and may be shed, and a *shed tool call* would be a
//! step the developer never saw the agent take. The buffered message always
//! arrives, and it arrives with the input whole rather than as partial JSON this
//! file would have to reassemble.
//!
//! What it costs is announcing the call when the block closes rather than when it
//! opens, which is a moment before the tool runs rather than a moment after the
//! model decided to run it. What it buys is that the pair is never half-published.
//!
//! The order of the rows follows from the same choice, and needs no sorting: the
//! CLI closes one block before announcing the next, so folding lines in the order
//! they arrive puts the work log in the order the work happened. `worklog`'s
//! module documentation covers the rest — what the rows look like, and why they
//! are the kinds they are.
//!
//! ## A permission request is the one thing the agent waits for
//!
//! Everything else in the table above is the agent talking. A `control_request`
//! is the agent *asking*, and it has stopped until it is answered — so the loop
//! polls the decision channel unconditionally, where it deliberately does not
//! poll for a second prompt while one is in flight. A decision deferred behind a
//! queued turn would be the deadlock the ticket is about.
//!
//! Three things follow from the agent being stopped rather than merely busy:
//!
//! - **The request is remembered here, not in the fold.** The client answers by
//!   naming an id; what has to go back to the CLI is the whole request, because
//!   an approval carries the input the tool will run with. [`Driving`] is where
//!   the two are joined.
//! - **The agent is written to before the resolution is published.** The panel
//!   closing is what the developer sees; the write is what unsticks the
//!   conversation. Publishing first would risk a closed panel over a session
//!   still stopped.
//! - **Whatever is still outstanding when the driver ends is closed.** The
//!   client's panel is folded out of `approval.requested` minus
//!   `approval.resolved` and those activities are *stored*, so a request left
//!   open would be a composer the developer cannot type into — after a restart as
//!   well as before one. Ending the driver settles them as cancelled, which is
//!   what actually happened.
//!
//! An unanswered request costs a tool call and nothing else: closing the agent's
//! stdin closes the permission stream with it, the CLI abandons the request, and
//! the turn finishes. `fixtures/claude-cli/09-permission-unanswered.ndjson` is a
//! recording of exactly that, and [`crate::agent`] documents the mechanism.
//!
//! ## Stopping a turn is not stopping a session, and the difference is the point
//!
//! A turn is interrupted by a `control_request` on the agent's stdin — the same
//! envelope a permission arrives in, going the other way. It leaves the child
//! running, which is what makes the correction the developer types next a
//! *correction* rather than the first message of a conversation that has
//! forgotten what it was about.
//!
//! Four things follow, and the recordings in `fixtures/claude-cli/11`–`14` are
//! where each of them came from rather than from documentation:
//!
//! - **The CLI reports a stopped turn as a failed one.** Its `result` carries
//!   `"is_error": true` and the subtype `error_during_execution`. Nothing in the
//!   output distinguishes "the developer pressed stop" from "the turn went
//!   wrong" — so [`InFlight::stopped`] is the only thing that can, and it is why
//!   the flag exists rather than the turn being read off the wire.
//! - **The partial reply is kept, and the CLI hands it over whole.** After the
//!   acknowledgement comes a buffered `assistant` message carrying exactly what
//!   had streamed. So "output produced before the interrupt is retained" needs
//!   nothing special here: it is the ordinary reconcile, on a shorter message.
//! - **The turn settles twice, on purpose, and the first one is the click.**
//!   `thread.turn-interrupt-requested` goes up the moment the request has been
//!   written, and the client's reducer moves the latest turn to `interrupted` on
//!   it — so the turn stops being reported as running when the developer stops
//!   it rather than when the agent gets round to admitting it. The session
//!   follows when the agent's `result` arrives, as `interrupted` rather than
//!   `ready` or `error`. Both are the contract's own vocabulary; neither alone
//!   is enough, because the event does not describe the session and the session
//!   status arrives too late to be what the developer sees.
//! - **The session change at the end of a turn is conditional.** A developer who
//!   stops the agent can send the next turn while this one is still winding
//!   down, and by then the session describes *that* turn. Settling it here would
//!   report a turn that had just started as finished.
//!
//! An interrupt for a turn that is not running is a no-op rather than an error,
//! at every layer: the client sends one when it believes nothing is running, the
//! registry takes it with no session to route it to, and the driver drops it
//! with no turn to stop. See [`interrupt`], where the three races are named.
//!
//! ## Continuity is the agent's, and this is where the handle on it is kept
//!
//! Within one process there is nothing to do: the child is long-lived and the
//! conversation is its own, so a follow-up is a second line on the same stdin.
//! Across a restart there is exactly one thing to do, and it is a flag —
//! `--resume <session-id>` — because the context lives in the agent's own store
//! rather than in this server's transcript. Replaying the transcript into each
//! prompt would be the alternative, and it would be a second, worse copy of the
//! conversation that the agent had no reason to believe.
//!
//! So the `init` line's session id is remembered on the thread and written down
//! ([`crate::threads::Threads::remember_agent_session`]), and a session opened
//! for a thread that has one is opened with `--resume`. The id is the agent's own
//! account of itself rather than something this server minted, for the same
//! reason the model and the permission mode are.
//!
//! A resume the CLI will not honour is the one failure with no NDJSON to it at
//! all: the child writes its reason to stderr and exits. [`resume_refused`] is
//! how that becomes a sentence in the conversation, and the stored id is
//! deliberately *kept* — starting a fresh session under a thread whose history
//! the agent has forgotten would leave the developer talking to something that
//! cannot see the transcript in front of them.

use std::collections::HashMap;

use serde_json::json;

use crate::agent::{permission_mode_for, Agent, Launch};
use crate::clock::now_iso;
use crate::config::ClaudeSettings;
use crate::process::Search;
use crate::protocol::{ContentBlock, Folded, Permission, SessionState};
use crate::threads::{Activity, Answered, Change, Prompt, Session, Signal, Thread, Threads};
use crate::worklog::{Call, Decision, Returned};

/// Everything a session needs to start an agent, gathered while the thread is
/// known and carried into the task that will need it.
#[derive(Debug, Clone)]
pub struct Start {
    pub thread_id: String,
    /// The project's folder. The agent's working directory, which is what makes
    /// a relative path in the transcript mean what the developer thinks.
    pub workspace_root: String,
    pub model: Option<String>,
    pub runtime_mode: String,
    /// The `claude` session to continue, when the thread already has one. See
    /// this module's documentation: it is the whole of how a conversation
    /// survives a restart.
    pub resume: Option<String>,
    /// Read once, when the turn is dispatched. A settings change mid-session
    /// does not move a running agent, which is honest — the process was started
    /// with the old value and cannot be told otherwise.
    pub settings: ClaudeSettings,
}

/// Send one turn, starting a session for the thread if it has none.
///
/// Synchronous and non-blocking: it is called from the socket's read loop, which
/// must be free to take the next frame. The failure it can return is the prompt
/// channel being full or closed, which means a session that is not consuming —
/// and that is worth telling the client about rather than dropping.
pub fn send(threads: &Threads, start: &Start, prompt: Prompt) -> Result<(), String> {
    let driving = threads.clone();
    let starting = start.clone();
    let prompts = threads.attach(&start.thread_id, move |incoming, signals| {
        tokio::spawn(drive(driving, starting, incoming, signals))
    });

    prompts.try_send(prompt).map_err(|error| match error {
        tokio::sync::mpsc::error::TrySendError::Full(_) => {
            "The agent has not read the turns already sent to it, so this one was not queued."
                .to_string()
        }
        tokio::sync::mpsc::error::TrySendError::Closed(_) => {
            "The agent session has ended and could not be sent this turn.".to_string()
        }
    })
}

/// The session: start an agent, feed it turns and decisions, publish what it
/// says, reap it.
async fn drive(
    threads: Threads,
    start: Start,
    mut prompts: tokio::sync::mpsc::Receiver<Prompt>,
    mut signals: tokio::sync::mpsc::Receiver<Signal>,
) {
    let mut agent = match open(&start).await {
        Ok(agent) => agent,
        Err(why) => {
            // The session never existed, so there is no turn to attribute the
            // failure to beyond the one that asked for it. Reported in the
            // conversation rather than only to a log, because the developer is
            // looking at the conversation.
            threads.apply(
                &start.thread_id,
                Change::Activity(Activity::failed("session.failed", &why)),
            );
            threads.apply(
                &start.thread_id,
                Change::Session(Session {
                    status: "error",
                    runtime_mode: start.runtime_mode.clone(),
                    active_turn_id: None,
                    last_error: Some(why),
                    updated_at: now_iso(),
                }),
            );
            threads.detach(&start.thread_id);
            return;
        }
    };

    let mut folding = SessionState::new();
    let mut driving = Driving {
        turn: None,
        outstanding: HashMap::new(),
        interrupts: 0,
    };
    // A turn that arrived while another was still running. Held rather than
    // sent: sending it would orphan the turn in flight — that turn would never
    // settle, and the finished one's duration and cost would be attributed to
    // the wrong turn.
    let mut waiting: Option<Prompt> = None;
    // False once the prompt channel has closed. The agent is then told there
    // will be no more turns and the loop keeps draining what it still owes.
    let mut accepting = true;
    // False once the signal channel has closed, which is the same moment — both
    // ends live on the thread's `Live`. Tracked separately anyway, because a
    // closed channel yields `None` forever and a `select!` arm that kept polling
    // one would spin.
    let mut listening = true;

    loop {
        // Whatever is waiting goes next, as soon as the turn before it is done.
        if accepting && driving.turn.is_none() {
            if let Some(prompt) = waiting.take() {
                if let Err(error) = agent.send(&prompt.text).await {
                    eprintln!("lightcode: cannot send a turn to the agent: {error}");
                    break;
                }
                driving.turn = Some(InFlight {
                    turn_id: prompt.turn_id.clone(),
                    assistant_message_id: None,
                    tools: HashMap::new(),
                    stopped: None,
                });
                // The turn is under way, and *this* is where the session enters
                // `running` — not the agent's `init` line, which a long-lived
                // child prints once for the whole conversation. Driving it off
                // `init` would leave every turn after the first in `starting`,
                // and a session that is not `running` settles the turn at the
                // first assistant message, which is the mid-turn settle the
                // client's reducer exists to avoid.
                running(&threads, &start, &prompt.turn_id);
                continue;
            }
        }

        // The channel is polled whether or not a turn is running, so that a
        // shutdown mid-turn still closes the agent's input promptly. What it is
        // not allowed to do is take a second prompt before the first has been
        // dealt with, which is what `PROMPT_QUEUE` is behind it for.
        //
        // A signal is polled under no such condition, and that asymmetry is the
        // point: both kinds are owed to the turn *in flight*. A decision the
        // agent has stopped for, deferred behind a queued turn, is the deadlock
        // ticket 13 was about; an interrupt deferred the same way is a stop
        // button that works once the thing it was meant to stop has finished.
        let next = tokio::select! {
            line = agent.next_line() => Next::Line(line),
            signal = signals.recv(), if listening => Next::Signal(signal),
            prompt = prompts.recv(), if accepting && waiting.is_none() => Next::Prompt(prompt),
        };

        match next {
            Next::Line(Some(line)) => {
                publish(&threads, &start, &mut folding, &mut driving, &line);
            }
            // The agent stopped producing: it exited, or its output was
            // abandoned. Either way there is nothing more to publish.
            Next::Line(None) => break,
            Next::Signal(Some(Signal::Answer(answered))) => {
                answer(&threads, &start, &mut agent, &mut driving, answered).await;
            }
            Next::Signal(Some(Signal::Interrupt { turn_id })) => {
                interrupt(&threads, &start, &mut agent, &mut driving, turn_id).await;
            }
            Next::Signal(None) => listening = false,
            Next::Prompt(Some(prompt)) => waiting = Some(prompt),
            Next::Prompt(None) => {
                accepting = false;
                agent.close_input();
            }
        }
    }

    // Whatever the agent was still waiting on is never going to be answered now,
    // and the client derives its pending-approval panel from these two kinds
    // alone — so a request left open here is a composer the developer cannot type
    // into, for the life of the conversation and across every restart after it.
    // Closing them is what makes "the session remains usable" true of the way a
    // session actually ends.
    for asked in driving.take_outstanding() {
        threads.apply(
            &start.thread_id,
            Change::Activity(crate::worklog::resolved(
                &asked,
                Decision::Cancel,
                driving.turn.as_ref().map(|turn| turn.turn_id.clone()),
            )),
        );
    }

    // Reaped first, and the complaint comes back from the reaping. A session that
    // was asked to continue an old conversation and never announced itself did not
    // continue it: the CLI writes its reason to stderr and exits without a line of
    // NDJSON, so the agent's own words are the only account of it — and they are
    // only final once the child has gone. See [`Agent::stop`].
    let complaint = agent.stop().await;
    let refused = start
        .resume
        .as_ref()
        .filter(|_| folding.session_id.is_none())
        .map(|session| resume_refused(session, complaint.as_deref()));
    if let Some(why) = &refused {
        threads.apply(
            &start.thread_id,
            Change::Activity(Activity::failed("session.resume-failed", why)),
        );
    }

    // A turn still in flight when the agent went is a turn that will never
    // finish, and saying "stopped" would let it sit in the UI as running
    // forever. Which of the two it is decides how the client settles the turn.
    let unfinished = driving.turn.is_some();
    threads.apply(
        &start.thread_id,
        Change::Session(Session {
            status: if unfinished || refused.is_some() {
                "error"
            } else {
                "stopped"
            },
            runtime_mode: start.runtime_mode.clone(),
            active_turn_id: None,
            // The refusal wins when there is one: "the agent stopped before the
            // turn finished" is true of it and says nothing about why.
            last_error: refused.or_else(|| {
                unfinished.then(|| "The agent stopped before the turn finished.".to_string())
            }),
            updated_at: now_iso(),
        }),
    );
    threads.detach(&start.thread_id);
}

/// What the developer is told when a conversation cannot be picked back up.
///
/// The transcript above it is untouched and still readable — it is this server's
/// own copy, not the agent's — so what the sentence has to do is say that the
/// conversation is readable and not continuable, and name the session so the
/// developer can go and look for it. The CLI's own words are quoted when it left
/// any, because "no conversation found with that session id" is a great deal more
/// useful than anything this server could infer.
fn resume_refused(session_id: &str, complaint: Option<&str>) -> String {
    let mut why = format!(
        "Claude Code would not resume session {session_id}, so this conversation can be read \
         but not continued."
    );
    if let Some(complaint) = complaint {
        why.push_str(&format!(" The agent said: {complaint}"));
    }
    why
}

/// Which of the three sources the loop heard from. A named value rather than
/// bodies inside `select!`, because writing to the agent needs the same mutable
/// borrow the line future is holding until the select is over.
enum Next {
    Line(Option<String>),
    Prompt(Option<Prompt>),
    Signal(Option<Signal>),
}

/// What the driver knows that the fold does not.
///
/// Both fields are things only this side of the pair ever saw: the fold knows a
/// permission was *asked*, and this knows it has not been answered and which turn
/// to attribute the answer to.
struct Driving {
    /// The turn the agent is currently working on.
    turn: Option<InFlight>,
    /// Permission requests published and not yet answered, by the id the client
    /// answers with.
    ///
    /// Per session rather than per turn, unlike the tool calls beside them, and
    /// for the opposite reason: a tool call that outlived its turn has nothing
    /// left to answer it, while a permission that outlives its turn is a *panel
    /// the developer is still looking at*. Settling one is a thing that has to
    /// happen, so it must not be dropped with the turn.
    outstanding: HashMap<String, Permission>,
    /// How many interrupts this session has sent, which is what the next one's
    /// id is minted from.
    ///
    /// Per session and monotonic, so an acknowledgement can be matched to the
    /// request it answers and a *late* one — for a turn that has already ended —
    /// is recognised as late rather than mistaken for the current one.
    interrupts: usize,
}

impl Driving {
    /// The id for the next interrupt, and the count that makes it unique.
    fn next_interrupt_id(&mut self) -> String {
        self.interrupts += 1;
        format!("interrupt-{}", self.interrupts)
    }

    /// Take every request still waiting, so the caller can close it.
    fn take_outstanding(&mut self) -> Vec<Permission> {
        let mut open: Vec<Permission> = std::mem::take(&mut self.outstanding)
            .into_values()
            .collect();
        // A map has no order and these become rows in a work log. Ordered by the
        // id the agent minted, which is at least the same every run.
        open.sort_by(|left, right| left.request_id.cmp(&right.request_id));
        open
    }
}

/// Tell the agent what the developer decided, and say so in the conversation.
///
/// The agent is written to *first*, because that is the half with something
/// waiting on it: a decision published but not delivered would close the panel
/// on a conversation that is still stopped. If the write fails the row still goes
/// up — as a failure — because the alternative is a panel the developer can never
/// clear.
async fn answer(
    threads: &Threads,
    start: &Start,
    agent: &mut Agent,
    driving: &mut Driving,
    answered: Answered,
) {
    // An id this session never asked about, or asked about and has already been
    // answered on. Said the one way the client recognises as "this request is
    // gone", so a panel left behind by a session that died without settling is
    // cleared by the first attempt to answer it rather than being permanent.
    let Some(asked) = driving.outstanding.remove(&answered.request_id) else {
        threads.apply(
            &start.thread_id,
            Change::Activity(crate::worklog::unanswerable(&answered.request_id)),
        );
        return;
    };

    let sent = agent
        .answer(&asked.request_id, &answered.decision.answer(&asked))
        .await;

    // "Cancel" is a denial that also carries `interrupt: true`, so the CLI stops
    // the turn on it — which makes it an interrupt this server did not send but
    // did cause, and the turn has to end the same way one it did send would.
    // Ticket 13 sent the decision correctly and left this half undone by name;
    // it is the same fact recorded in the same field, and it is recorded only if
    // the decision actually reached the agent.
    let mut cancelled = None;
    if sent.is_ok() && answered.decision == Decision::Cancel {
        if let Some(active) = driving.turn.as_mut() {
            if active.stop(&asked.request_id) {
                cancelled = Some(active.turn_id.clone());
            }
        }
    }

    let turn_id = driving.turn.as_ref().map(|turn| turn.turn_id.clone());

    // Reported as a *session* failure rather than as an unanswerable request:
    // the request was real and this server knew it, and what went wrong is that
    // the agent stopped reading. The resolution below closes the panel either
    // way, which is why this does not have to.
    if let Err(error) = sent {
        eprintln!("lightcode: cannot answer a permission request: {error}");
        threads.apply(
            &start.thread_id,
            Change::Activity(Activity::failed(
                "session.failed",
                &format!(
                    "The decision could not be sent to the agent, which is no longer reading: \
                     {error}"
                ),
            )),
        );
    }

    // Published either way, and after the write either way. This is what closes
    // the panel, and a decision that reached the agent and was never recorded
    // would leave the developer asked a second time about work already under way.
    threads.apply(
        &start.thread_id,
        Change::Activity(crate::worklog::resolved(
            &asked,
            answered.decision,
            turn_id,
        )),
    );

    // After the resolution, so the work log reads in the order it happened: the
    // developer answered the question, and answering it that way stopped the turn.
    if let Some(turn_id) = cancelled {
        stopped(threads, start, &turn_id, &asked.request_id);
    }
}

/// Stop the turn the agent is working on, and say so in the conversation.
///
/// Three things make this a no-op rather than an error, and each is a race the
/// developer cannot see and should not have to think about:
///
/// - **Nothing is in flight.** The turn ended while the click was travelling, or
///   there was never one. Either way the agent is not doing the thing they asked
///   it to stop doing, which is what they wanted.
/// - **The turn named is not the turn running.** The client names the turn it is
///   looking at (`buildThreadTurnInterruptInput`), and the same race makes that
///   the *previous* turn a moment later. Stopping the one after it would be
///   stopping work the developer never saw start.
/// - **This turn has already been stopped.** A second click, or a stop after a
///   permission was cancelled. The agent is already winding down and a second
///   request would produce a second row saying so.
///
/// What is *not* published here is a session change. The turn is not over until
/// the agent says it is — the CLI still owes whatever it had buffered and then a
/// `result` — and this server saying "stopped" before that would be describing
/// an ending it had only asked for. `Folded::Completed` is where the turn
/// settles, and [`InFlight::stopped`] is what tells it how.
async fn interrupt(
    threads: &Threads,
    start: &Start,
    agent: &mut Agent,
    driving: &mut Driving,
    wanted: Option<String>,
) {
    // Every reason to do nothing is settled before an id is minted, so a no-op
    // costs nothing and the ids in the work log count stops that happened.
    let Some(active) = driving.turn.as_ref() else {
        return;
    };
    if wanted.is_some_and(|turn_id| turn_id != active.turn_id) || active.stopped.is_some() {
        return;
    }
    let turn_id = active.turn_id.clone();
    let request_id = driving.next_interrupt_id();

    // The agent is written to before anything else happens, the same way a
    // decision is and for the same reason: the rows below are what the developer
    // sees and the write is what actually stops the work. Publishing first would
    // be a claim about an agent still going — and *recording* first would be
    // worse, because a turn marked stopped that nobody managed to stop would
    // report itself as stopped when it finished normally.
    //
    // The borrow is taken again afterwards rather than held across the write:
    // `agent` and `driving` are separate borrows and the second must not outlive
    // the await, which is the same reason [`Next`] exists.
    if let Err(error) = agent.interrupt(&request_id).await {
        eprintln!("lightcode: cannot interrupt the agent: {error}");
        threads.apply(
            &start.thread_id,
            Change::Activity(Activity::failed(
                "turn.interrupt-failed",
                &format!(
                    "The agent could not be asked to stop, because it is no longer reading: \
                     {error}"
                ),
            )),
        );
        return;
    }

    // The turn can only have gone away while the write was in flight if the
    // agent's output ended, and then the loop is about to end too.
    let Some(active) = driving.turn.as_mut() else {
        return;
    };
    active.stop(&request_id);
    stopped(threads, start, &turn_id, &request_id);
}

/// Say in the conversation that the developer stopped this turn.
///
/// Two changes rather than one, because they are read by different parts of the
/// client and neither can be derived from the other:
///
/// - **The event settles the turn.** `thread.turn-interrupt-requested` is the
///   contract's own, and the client's reducer moves the latest turn to
///   `interrupted` on it *immediately* — so the turn stops being reported as
///   running when the developer's click lands rather than when the agent gets
///   round to admitting it. Without it the composer would show work in progress
///   for as long as the agent took to wind down.
/// - **The row is the record.** The work log is what a developer reads later,
///   and "this turn stopped because I stopped it" is not derivable from the
///   partial reply above it.
///
/// Shared by the two things that stop a turn — the stop button, and cancelling a
/// permission — because they are the same event to a client either way.
fn stopped(threads: &Threads, start: &Start, turn_id: &str, request_id: &str) {
    threads.apply(
        &start.thread_id,
        Change::InterruptRequested {
            turn_id: turn_id.to_string(),
        },
    );
    threads.apply(
        &start.thread_id,
        Change::Activity(Activity::info(
            "turn.interrupted",
            "The developer stopped the turn.",
            json!({
                "requestId": request_id,
                "turnId": turn_id,
                // What the client's work log renders as the row's body. Without
                // it the row is a heading with nothing under it — see
                // `Activity::failed`, which repeats its summary for the same
                // reason.
                "detail": "The developer stopped the turn. Anything the agent had already \
                           said is kept.",
            }),
            Some(turn_id.to_string()),
        )),
    );
}

/// The session is working on this turn.
fn running(threads: &Threads, start: &Start, turn_id: &str) {
    threads.apply(
        &start.thread_id,
        Change::Session(Session {
            status: "running",
            runtime_mode: start.runtime_mode.clone(),
            active_turn_id: Some(turn_id.to_string()),
            last_error: None,
            updated_at: now_iso(),
        }),
    );
}

/// The turn the agent is currently working on.
struct InFlight {
    turn_id: String,
    /// Minted at the first piece of assistant text and cleared when that message
    /// completes, so a turn that produces several messages — commentary between
    /// tool calls — gives each its own id rather than appending them all into one.
    assistant_message_id: Option<String>,
    /// Tool calls announced and not yet answered, by the id the agent minted.
    ///
    /// The pairing has to be remembered because the two halves arrive in
    /// different messages and only the first one says what the tool was: a
    /// `tool_result` carries an id, a payload and nothing else. Per turn rather
    /// than per session, because a call is always answered within the turn that
    /// made it — and a turn that ended with one outstanding has nothing left to
    /// answer it.
    tools: HashMap<String, Call>,
    /// The id of the request that stopped this turn, once something has.
    ///
    /// Two things can be: an interrupt this server sent, or a permission the
    /// developer *cancelled*, which is a denial the CLI honours by stopping the
    /// turn. They are one field because they are one fact — the turn is ending
    /// because the developer said so — and the turn's ending has to read the
    /// same way whichever of them it was.
    ///
    /// Two things read it. An acknowledgement matches against it, so an answer
    /// to some other request is not taken for this one. And `Folded::Completed`
    /// reads it as a fact about *how* the turn ended: the CLI reports an aborted
    /// turn as an error, and a turn the developer stopped on purpose is not one
    /// to show them a failure for.
    stopped: Option<String>,
}

impl InFlight {
    /// Record that something has stopped this turn, and say whether it was the
    /// first thing to.
    ///
    /// `false` is a turn already stopping — a second click, or a stop after a
    /// permission was cancelled — and the caller answers it by saying nothing,
    /// because the work log already says the turn is being stopped.
    fn stop(&mut self, request_id: &str) -> bool {
        if self.stopped.is_some() {
            return false;
        }
        self.stopped = Some(request_id.to_string());
        true
    }

    /// Is this the answer to the stop this turn is waiting on?
    fn awaiting(&self, request_id: &str) -> bool {
        self.stopped.as_deref() == Some(request_id)
    }

    /// The agent will not stop, so this turn is going to end the way it was
    /// going to end.
    ///
    /// The flag has to come off rather than merely being ignored: a normal
    /// ending reported as one the developer asked for would be a work log
    /// claiming they did something they did not.
    fn carries_on(&mut self) {
        self.stopped = None;
    }

    fn was_stopped(&self) -> bool {
        self.stopped.is_some()
    }
}

/// How a turn ended.
///
/// Three outcomes where the wire has two: a `result` is an error or it is not,
/// and a turn the developer stopped arrives as an error
/// (`fixtures/claude-cli/11-interrupted-turn.ndjson`). The third is this
/// server's own knowledge that it asked, and naming it once is what stops the
/// summary, the tone, the payload and the session status each working it out
/// again — three `match`es on the same pair of booleans, which is three chances
/// to disagree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ending {
    Completed,
    Failed,
    Stopped,
}

impl Ending {
    fn of(turn: Option<&InFlight>, result: Option<&crate::protocol::ResultSummary>) -> Ending {
        // Stopped wins over failed, because it is the more specific truth about
        // the same `result`.
        match (
            turn.is_some_and(InFlight::was_stopped),
            result.is_some_and(|result| result.is_error),
        ) {
            (true, _) => Ending::Stopped,
            (false, true) => Ending::Failed,
            (false, false) => Ending::Completed,
        }
    }

    /// What the session becomes. `interrupted` is one of the contract's own
    /// statuses and [`crate::threads`] settles the turn as interrupted with it.
    fn session_status(self) -> &'static str {
        match self {
            Ending::Completed => "ready",
            Ending::Failed => "error",
            Ending::Stopped => "interrupted",
        }
    }

    /// Only a failure is styled as one. A turn the developer stopped is not
    /// something that went wrong.
    fn tone(self) -> &'static str {
        match self {
            Ending::Failed => "error",
            _ => "info",
        }
    }

    fn failed(self) -> bool {
        self == Ending::Failed
    }

    fn stopped(self) -> bool {
        self == Ending::Stopped
    }

    /// How the sentence a developer reads begins.
    fn opening(self) -> &'static str {
        match self {
            Ending::Completed => "Turn completed",
            Ending::Failed => "Turn failed",
            Ending::Stopped => "Turn stopped by the developer",
        }
    }
}

/// Resolve the binary and start the agent, or say why not.
async fn open(start: &Start) -> Result<Agent, String> {
    // Resolved here rather than on the dispatch path: it is a walk of every
    // `PATH` directory, and the read loop is answering a developer who has just
    // pressed enter. Resolved per session rather than once at boot because the
    // setting can change and an install can move, and this is the moment the
    // answer actually matters.
    let (path, _) = crate::provider::resolve(&start.settings.binary_path, &Search::from_environment())
        .startable()?;

    Agent::start(&Launch {
        binary: path.clone(),
        cwd: start.workspace_root.clone(),
        model: start.model.clone(),
        permission_mode: permission_mode_for(&start.runtime_mode),
        resume: start.resume.clone(),
    })
    .await
    .map_err(|error| {
        format!(
            "The Claude Code binary {} could not be started in {}: {error}",
            path.display(),
            start.workspace_root
        )
    })
}

/// Fold one line and publish whatever it turned out to be.
fn publish(
    threads: &Threads,
    start: &Start,
    folding: &mut SessionState,
    driving: &mut Driving,
    line: &str,
) {
    let turn = &mut driving.turn;
    match folding.fold_line(line) {
        Folded::Nothing => {}

        // The agent has stopped and is waiting for the developer. The row is what
        // raises the question — the client's pending-approval panel is folded out
        // of these — and the copy beside it is what makes the answer possible,
        // because the answer names an id and the *request* is what has to be sent
        // back with it.
        //
        // The session stays `running` throughout, deliberately. It is true — the
        // turn has not ended — and it is also the only thing that can be said:
        // `OrchestrationSessionStatus` has no `waiting`, and a status outside that
        // union fails the client's decode of the whole session. What tells the
        // developer they are being waited on is the panel.
        Folded::PermissionRequested { index } => {
            let asked = folding.permissions[index].clone();
            threads.apply(
                &start.thread_id,
                Change::Activity(crate::worklog::requested(
                    &asked,
                    turn.as_ref().map(|turn| turn.turn_id.clone()),
                )),
            );
            driving.outstanding.insert(asked.request_id.clone(), asked);
        }

        // The agent announcing itself. Once per process rather than per turn, so
        // this only ever appends the activity; the session's `running` comes
        // from the prompt being sent, in `drive`.
        Folded::Initialized => {
            // Before the activity, because this is the load-bearing half: the
            // activity is what a developer reads and this is what the next run
            // resumes into. A resumed session announces an id too — the CLI is
            // free to hand back a new one — so the thread always holds the most
            // recent, which is the one `--resume` will be given next.
            if let Some(session_id) = &folding.session_id {
                threads.remember_agent_session(&start.thread_id, session_id);
            }
            threads.apply(
                &start.thread_id,
                Change::Activity(Activity::info(
                    "session.init",
                    &session_summary(folding),
                    json!({
                        "sessionId": folding.session_id,
                        "model": folding.model,
                        "permissionMode": folding.permission_mode,
                        "cwd": folding.cwd,
                        "toolCount": folding.tool_count,
                    }),
                    turn.as_ref().map(|turn| turn.turn_id.clone()),
                )),
            );
        }

        Folded::Streamed(text) => {
            let Some(active) = turn.as_mut() else { return };
            let message_id = active
                .assistant_message_id
                .get_or_insert_with(crate::threads::fresh_message_id)
                .clone();
            threads.apply(
                &start.thread_id,
                Change::AssistantDelta {
                    message_id,
                    turn_id: active.turn_id.clone(),
                    text,
                },
            );
        }

        Folded::Turn { index } => {
            let Some(active) = turn.as_mut() else { return };
            let completed = &folding.transcript[index];

            // Only the assistant's text is the conversation, and only when there
            // is some — or when deltas have already put some on screen under an id
            // that has not been closed. A message that is nothing but a tool call
            // or a pause to reason would otherwise arrive as an empty chat bubble:
            // the CLI emits one buffered message per *content block*, so a turn
            // that uses a tool produces several of them and most carry no text at
            // all. The second half of the condition is what stops the skip going
            // too far the other way — a reply that streamed and then buffered
            // nothing still owes the client the non-streaming send that settles it,
            // or the message would stay `streaming` for the life of the thread.
            //
            // The developer's own turn is not echoed back: that needs
            // `--replay-user-messages`, which this server does not pass. What does
            // arrive with the role `user` is a tool result, and that is work
            // rather than something said — it is published below, as work.
            let owes_a_message =
                !completed.text.is_empty() || active.assistant_message_id.is_some();
            if completed.role == "assistant" && owes_a_message {
                let message_id = active
                    .assistant_message_id
                    .take()
                    .unwrap_or_else(crate::threads::fresh_message_id);
                threads.apply(
                    &start.thread_id,
                    Change::AssistantMessage {
                        message_id,
                        turn_id: active.turn_id.clone(),
                        text: completed.text.clone(),
                    },
                );
            }

            // Everything the message carried besides its text, in the order it
            // carried it — which is the order the developer saw it happen, because
            // the CLI closes a block before announcing the next.
            for block in &completed.content {
                let change = match block {
                    ContentBlock::Thinking { thinking } => {
                        crate::worklog::thinking(thinking, Some(active.turn_id.clone()))
                    }
                    ContentBlock::ToolUse { id, name, input } => {
                        let call = Call {
                            id: id.clone(),
                            name: name.clone(),
                            input: input.clone(),
                        };
                        let invoked = call.invoked(Some(active.turn_id.clone()));
                        // Held until the result names this id: the result carries
                        // the id and nothing else, so what the tool *was* and what
                        // it was *given* only exists on this side of the pair.
                        active.tools.insert(id.clone(), call);
                        Some(invoked)
                    }
                    ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                    } => {
                        let call = active
                            .tools
                            .remove(tool_use_id)
                            .unwrap_or_else(|| Call::untracked(tool_use_id));
                        Some(call.returned(
                            Returned {
                                content,
                                failed: *is_error,
                            },
                            Some(active.turn_id.clone()),
                        ))
                    }
                    // The text is already in the transcript, and a block this
                    // build cannot read is drift with nothing to show for it.
                    ContentBlock::Text { .. } | ContentBlock::Unknown => None,
                };

                if let Some(activity) = change {
                    threads.apply(&start.thread_id, Change::Activity(activity));
                }
            }
        }

        // The agent answering the one thing this server asks it. Matched against
        // the request that is outstanding, because an acknowledgement for some
        // other id — a late one, for a turn that has already ended — says nothing
        // about the turn running now.
        Folded::Acknowledged(acknowledged) => {
            let Some(active) = turn.as_mut() else { return };
            if !active.awaiting(&acknowledged.request_id) {
                return;
            }
            let Some(why) = acknowledged.refusal() else { return };

            active.carries_on();
            threads.apply(
                &start.thread_id,
                Change::Activity(Activity::failed(
                    "turn.interrupt-failed",
                    &format!("The agent would not stop the turn: {why}"),
                )),
            );
        }

        Folded::Completed => {
            let finished = turn.take();
            let active = finished.as_ref().map(|turn| turn.turn_id.clone());
            let summary = folding.last_result.as_ref();
            let ending = Ending::of(finished.as_ref(), summary);

            let completed = Activity {
                tone: ending.tone(),
                ..Activity::info(
                    "turn.completed",
                    &turn_summary(folding, ending),
                    json!({
                        "durationMs": summary.and_then(|result| result.duration_ms),
                        "totalCostUsd": summary.and_then(|result| result.total_cost_usd),
                        "numTurns": summary.and_then(|result| result.num_turns),
                        "stopReason": summary.and_then(|result| result.stop_reason.clone()),
                        "isError": ending.failed(),
                        "interrupted": ending.stopped(),
                        // The drift accounting for this session, next to the turn
                        // it accumulated over — so a CLI that moved shows up where
                        // a developer is already looking.
                        "unknownEvents": folding.unknown_events,
                        "parseErrors": folding.parse_errors,
                    }),
                    active.clone(),
                )
            };
            threads.apply(&start.thread_id, Change::Activity(completed));

            // **Only when the session is still describing this turn.** A
            // developer who stopped the agent can send the next turn while this
            // one is still winding down — that is the whole point of stopping it
            // — and the dispatch has already moved the session on to that turn.
            // Publishing here would settle a turn that has just started, and the
            // client would show it finished until the agent got round to it.
            if threads.active_turn(&start.thread_id) != active {
                return;
            }

            // Leaving `running` is what ends the turn for the client, so this
            // is the event that settles it — and the reason a turn's reported
            // duration covers the whole turn rather than stopping at the last
            // thing the assistant said. `interrupted` is one of the contract's
            // own session statuses and settles the turn as interrupted with it,
            // which is what keeps the partial reply on screen marked as what it
            // is rather than as an answer.
            threads.apply(
                &start.thread_id,
                Change::Session(Session {
                    status: ending.session_status(),
                    runtime_mode: start.runtime_mode.clone(),
                    active_turn_id: None,
                    last_error: ending.failed().then(|| turn_summary(folding, ending)),
                    updated_at: now_iso(),
                }),
            );
        }
    }
}

/// What the developer is told about the session that just started.
///
/// The ticket asks for the model and the permission mode to be shown, and this
/// is where they are: the agent's own account of both, rather than what it was
/// asked for. The two can differ — an alias resolves, a permission mode is
/// overridden by the user's own settings file — and the one worth showing is
/// the one in force.
fn session_summary(state: &SessionState) -> String {
    format!(
        "Claude Code session started · model {} · permission mode {} · {} tools",
        state.model.as_deref().unwrap_or("unknown"),
        state.permission_mode.as_deref().unwrap_or("unknown"),
        state.tool_count,
    )
}

/// What the developer is told about the turn that just ended.
///
/// Duration and cost in one sentence, because the contract has nowhere
/// structured to put either: `OrchestrationLatestTurn` carries timestamps and no
/// money, and upstream's own `totalCostUsd` never leaves its internal event bus.
/// An activity is the contract's mechanism for exactly this, and the UI's work
/// log renders any kind it does not specifically suppress — so the sentence is
/// what a developer actually sees, and the payload beside it is what a later
/// ticket can render properly.
fn turn_summary(state: &SessionState, ending: Ending) -> String {
    let Some(result) = &state.last_result else {
        return format!("{}.", ending.opening());
    };

    let mut summary = ending.opening().to_string();
    if let Some(duration) = result.duration_ms {
        summary.push_str(&format!(" in {}", human_duration(duration)));
    }
    if let Some(cost) = result.total_cost_usd {
        // Four decimal places: a short turn costs a fraction of a cent, and two
        // would round every one of them to zero.
        summary.push_str(&format!(" · ${cost:.4}"));
    }
    if let Some(reason) = &result.stop_reason {
        summary.push_str(&format!(" · {reason}"));
    }
    summary
}

/// A duration a person reads rather than a number of milliseconds.
fn human_duration(milliseconds: u64) -> String {
    match milliseconds {
        under_a_second if under_a_second < 1_000 => format!("{under_a_second}ms"),
        under_a_minute if under_a_minute < 60_000 => {
            format!("{:.1}s", under_a_minute as f64 / 1_000.0)
        }
        longer => format!("{}m {}s", longer / 60_000, (longer % 60_000) / 1_000),
    }
}

/// The thread and project a turn needs, gathered into what the driver takes.
///
/// A free function rather than a method on either, because it is the one place
/// three things meet: the thread says which model and how much latitude, the
/// project says where, and the settings say which binary.
pub fn starting(thread: &Thread, workspace_root: &str, settings: &ClaudeSettings) -> Start {
    Start {
        thread_id: thread.id.clone(),
        workspace_root: workspace_root.to_string(),
        model: thread.model(),
        runtime_mode: thread.runtime_mode.clone(),
        // Read here rather than inside the driver, so what is resumed is the
        // session the thread held when the turn was dispatched. A session opened
        // for a thread that has none starts a fresh conversation and reports its
        // own id back a moment later.
        resume: thread.agent_session_id.clone(),
        settings: settings.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(
        is_error: bool,
        duration_ms: Option<u64>,
        cost: Option<f64>,
        stop_reason: Option<&str>,
    ) -> SessionState {
        let mut state = SessionState::new();
        state.last_result = Some(crate::protocol::ResultSummary {
            is_error,
            stop_reason: stop_reason.map(str::to_string),
            num_turns: Some(1),
            duration_ms,
            total_cost_usd: cost,
        });
        state
    }

    /// The ticket asks a completed turn to report its duration and its cost, and
    /// this sentence is where a developer reads both. The captured values from
    /// `fixtures/claude-cli/02-streamed-turn.ndjson`, so the rendering is pinned
    /// against a real turn rather than a round number.
    #[test]
    fn a_completed_turn_reports_its_duration_and_its_cost() {
        let summary = turn_summary(
            &result(
                false,
                Some(2008),
                Some(0.079_471_999_999_999_99),
                Some("end_turn"),
            ),
            Ending::Completed,
        );

        assert_eq!(summary, "Turn completed in 2.0s · $0.0795 · end_turn");
    }

    /// A turn that failed says so first, because that is what the developer
    /// needs from the sentence before anything else in it.
    #[test]
    fn a_failed_turn_says_so_before_it_says_anything_else() {
        let summary = turn_summary(
            &result(true, Some(400), Some(0.0), Some("error")),
            Ending::Failed,
        );
        assert!(summary.starts_with("Turn failed"), "{summary}");
    }

    /// A turn the developer stopped says *that*, over the failure the CLI
    /// reports it as. The values are `11-interrupted-turn.ndjson`'s own: the
    /// recording's `result` is `"is_error": true` with no cost and no stop
    /// reason, so a server reading the wire alone would tell the developer their
    /// own decision had gone wrong — which is the reading `Ending::of` exists to
    /// prevent, and the line below is the whole of it.
    #[test]
    fn a_stopped_turn_says_it_was_stopped_rather_than_that_it_failed() {
        let interrupted = InFlight {
            turn_id: "turn-1".to_string(),
            assistant_message_id: None,
            tools: HashMap::new(),
            stopped: Some("interrupt-1".to_string()),
        };
        let aborted = result(true, Some(13_660), Some(0.0), None);
        let ending = Ending::of(Some(&interrupted), aborted.last_result.as_ref());
        assert_eq!(ending, Ending::Stopped);
        assert_eq!(ending.session_status(), "interrupted");
        assert_eq!(ending.tone(), "info");
        assert!(!ending.failed(), "a turn the developer stopped is not an error");

        assert_eq!(
            turn_summary(&aborted, ending),
            "Turn stopped by the developer in 13.7s · $0.0000"
        );

        // And with nothing reported at all, which is the shape a session that
        // died before its `result` would leave behind.
        assert_eq!(
            turn_summary(&SessionState::new(), Ending::Stopped),
            "Turn stopped by the developer."
        );
    }

    /// The same turn *without* the flag reads as the failure the CLI called it.
    /// The pair is the point: one field decides which, and this is the other half
    /// of it.
    #[test]
    fn the_same_aborted_result_reads_as_a_failure_when_nobody_stopped_it() {
        let running = InFlight {
            turn_id: "turn-1".to_string(),
            assistant_message_id: None,
            tools: HashMap::new(),
            stopped: None,
        };
        let aborted = result(true, Some(13_660), Some(0.0), None);
        let ending = Ending::of(Some(&running), aborted.last_result.as_ref());

        assert_eq!(ending, Ending::Failed);
        assert_eq!(ending.session_status(), "error");
        assert_eq!(ending.tone(), "error");
    }

    /// A turn stops once. A second stop — a second click, or a stop after a
    /// permission was cancelled — is not a second row in the work log, and the
    /// id kept is the one the agent will answer.
    #[test]
    fn a_turn_records_the_first_thing_that_stopped_it_and_not_the_second() {
        let mut turn = InFlight {
            turn_id: "turn-1".to_string(),
            assistant_message_id: None,
            tools: HashMap::new(),
            stopped: None,
        };

        assert!(turn.stop("interrupt-1"));
        assert!(!turn.stop("interrupt-2"), "a turn cannot be stopped twice");
        assert!(turn.awaiting("interrupt-1"));
        assert!(
            !turn.awaiting("interrupt-2"),
            "an answer to a request this turn never made must not be taken for its own"
        );

        // And an agent that refuses puts the turn back where it was, so its
        // ordinary ending is reported as an ordinary ending.
        turn.carries_on();
        assert!(!turn.was_stopped());
        assert!(turn.stop("interrupt-3"), "a refused stop can be retried");
    }

    /// A CLI that reported neither still produces a sentence. The fields are
    /// optional in the protocol, and a half-built string with a dangling
    /// separator would be worse than a short one.
    #[test]
    fn a_turn_with_nothing_to_report_still_says_it_finished() {
        assert_eq!(
            turn_summary(&result(false, None, None, None), Ending::Completed),
            "Turn completed"
        );
        assert_eq!(
            turn_summary(&SessionState::new(), Ending::Completed),
            "Turn completed."
        );
    }

    /// Three orders of magnitude, because a turn can be any of them and
    /// "124000ms" is not something a person reads.
    #[test]
    fn a_duration_is_rendered_at_the_scale_it_happened_on() {
        assert_eq!(human_duration(0), "0ms");
        assert_eq!(human_duration(999), "999ms");
        assert_eq!(human_duration(1_000), "1.0s");
        assert_eq!(human_duration(2_008), "2.0s");
        assert_eq!(human_duration(59_999), "60.0s");
        assert_eq!(human_duration(124_000), "2m 4s");
    }

    /// The model and the permission mode the ticket asks to be shown are the
    /// agent's own account of them, which is why this reads them off the folded
    /// state rather than off what the agent was asked for.
    #[test]
    fn the_session_summary_names_the_model_and_the_permission_mode() {
        let mut state = SessionState::new();
        state.fold_line(
            r#"{"type":"system","subtype":"init","session_id":"s","model":"claude-opus-5[1m]","cwd":"/tmp","permissionMode":"bypassPermissions","tools":["Read","Write"]}"#,
        );

        let summary = session_summary(&state);
        assert!(summary.contains("claude-opus-5[1m]"), "{summary}");
        assert!(summary.contains("bypassPermissions"), "{summary}");
        assert!(summary.contains("2 tools"), "{summary}");

        // And a session that never announced itself says so rather than
        // rendering an empty gap where the model should be.
        let unknown = session_summary(&SessionState::new());
        assert!(unknown.contains("unknown"), "{unknown}");
    }
}
