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
//! ## The table is one function, and it does not apply itself
//!
//! [`decide`] is that table in code — one match over [`crate::protocol::Folded`]
//! — and it answers with a [`Decided`] rather than publishing as it goes.
//! [`spend`] is what publishes. The split is `docs/adr/0027` and it is
//! ADR-0025's, taken again one level down: a function that applies its own
//! results can be tested only by watching what it did to a live world, and this
//! one's world is a [`Threads`] with a running `claude` behind it. Nothing about
//! the wire changed — the same events, in the same order, under the same numbers
//! — and the tests at the bottom of this file are what the change was for.
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
//! The other act — ending the session — arrives on the same channel as
//! [`Signal::Stop`] and is answered by *leaving this loop*. Everything below the
//! loop is the ending a session is owed, and [`Agent::stop`] at the end of it
//! closes stdin, waits, and kills if waiting was not enough. That bound is why
//! it is a signal rather than only the prompt channel closing, which is the
//! gentler "no more turns" a shutdown and a deleted project use: the case the
//! stop command exists for is an agent that is not answering, and closing a pipe
//! at one of those is a hope. Two things follow from the developer having asked,
//! and both are marked in the ending: the turn it cut short is not reported as
//! the agent having died, and it gets no checkpoint — for
//! [`Ending::checkpoint_status`]'s reason, since no checkpoint status means "the
//! developer ended the session" and both the ones there are would relabel the
//! turn.
//!
//! ## A turn is also a point in time, and this is the only place that knows when
//!
//! Ticket 20 asks for a turn to be reviewable as a diff, and a diff needs a
//! *before*. Nothing in git records what the working tree looked like when the
//! developer pressed enter, so this file records it: a checkpoint is written
//! before the prompt reaches the agent and again when the turn ends, and
//! [`crate::checkpoints`] owns everything about how. Two properties come from
//! the placement rather than from that module:
//!
//! - **The baseline is awaited before the agent is written to.** A capture
//!   racing the agent's first edit would record a tree that already had it, and
//!   the turn would show a diff missing its own first change.
//! - **The ending is recorded before the next prompt is taken.** That is what
//!   chains the checkpoints: turn two's baseline is turn one's ending, so a
//!   conversation is a sequence of adjoining ranges with no gap for a hand edit
//!   to fall into unattributed.
//!
//! Both are `await`s on a blocking thread in a loop that is otherwise reading a
//! child's output, and both cost a `git add -A` on the project. That is the
//! price of the feature and it is paid twice per conversation plus once per
//! turn.
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
//!
//! ## The child is moved rather than replaced when the conversation changes
//!
//! `--permission-mode` and `--model` are launch flags, and one child serves a
//! whole conversation — so for as long as those were the only places the values
//! reached the agent, a developer who changed either mid-conversation was
//! answered by a process still running under the old one. The picker moved, the
//! change survived a restart, the next turn was *requested* under it, and nothing
//! the agent did was any different.
//!
//! [`retune`] closes that, and the shape is worth stating because three
//! properties fall out of it rather than being enforced anywhere:
//!
//! - **The moment is the turn's own dispatch**, not the moment the picker's
//!   command commits. What a turn wants rides on its [`Prompt`], so it is spent
//!   when that turn is handed to the agent and not before: a change made while a
//!   turn is running does not move the rules under it, and a turn queued behind
//!   another keeps its own. Ticket 02 and the spec's story 29.
//! - **The comparison is against [`Start`]**, which is this driver's account of
//!   what the child in front of it is running under rather than of what the
//!   thread last said. That is what makes "no push when nothing changed" true,
//!   and it is why a successful push has to move `Start` — every session event
//!   this file publishes reads it.
//! - **A refusal is a `control_response`**, arriving later like everything else
//!   the agent says, so putting `Start` back is a thing [`decide`] asks for and
//!   the loop spends.
//!
//! Ticket 11 of `.scratch/thread-lifecycle/`, and
//! `fixtures/claude-cli/20-modes-changed-mid-conversation.ndjson` is a real
//! `claude` being moved between two turns.

use std::collections::HashMap;

use serde_json::json;

use crate::agent::{permission_mode_for, Agent, Launch};
use crate::clock::{iso_from_epoch, now_iso};
use crate::config::ClaudeSettings;
use crate::process::Search;
use crate::protocol::{
    Compaction, ContentBlock, Drift, Folded, Permission, RateLimit, SessionState, TokenUsage,
};
use crate::settling::SessionStatus;
use crate::threads::{
    Activity, Answered, Change, Prompt, Retune, Session, Signal, Thread, Threads,
    UserInputAnswered,
};
use crate::worklog::{Call, Decision, Returned};

/// Everything a session needs to start an agent, gathered while the thread is
/// known and carried into the task that will need it.
#[derive(Debug, Clone)]
pub struct Start {
    pub thread_id: String,
    /// The project's folder. The agent's working directory, which is what makes
    /// a relative path in the transcript mean what the developer thinks.
    pub workspace_root: String,
    /// The model the child is running under — the launch flag at first, and then
    /// whatever a successful push has moved it to. See [`retune`].
    pub model: Option<String>,
    /// The runtime mode the child is running under, on the same terms as
    /// [`Start::model`].
    ///
    /// **Every session event the driver publishes reads this**, which is why a
    /// push that succeeds has to move it and a push that is refused has to leave
    /// it alone: the badge beside the session state is a claim about what the
    /// agent is doing, not about what was asked of it.
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
pub fn send(threads: &Threads, start: &Start, turn_id: String, text: String) -> Result<(), String> {
    let driving = threads.clone();
    let starting = start.clone();
    let prompts = threads.attach(&start.thread_id, move |incoming, signals, epoch| {
        tokio::spawn(drive(driving, starting, incoming, signals, epoch))
    });

    // The whole prompt is built here rather than by the caller, because what a
    // turn carries is more than what the developer typed: it also carries what
    // the conversation said the agent should be running under when this turn was
    // dispatched, and that is read off the same [`Start`] the session was opened
    // from. A session started a line above is already launched with those values
    // and the driver's guard makes it the no-op it should be; what this is for is
    // every turn after the first, where the launch flags were the only place they
    // had ever reached the child.
    let prompt = Prompt {
        turn_id,
        text,
        wanted: Retune {
            runtime_mode: start.runtime_mode.clone(),
            model: start.model.clone(),
        },
    };

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
///
/// `epoch` is which of the conversation's sessions this one is, and is given
/// back on the way out — a driver may outlive its own slot, so it gives up only
/// the slot it was in. See [`crate::threads::Threads::detach`].
async fn drive(
    threads: Threads,
    mut start: Start,
    mut prompts: tokio::sync::mpsc::Receiver<Prompt>,
    mut signals: tokio::sync::mpsc::Receiver<Signal>,
    epoch: u64,
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
                    status: SessionStatus::Error,
                    runtime_mode: start.runtime_mode.clone(),
                    active_turn_id: None,
                    last_error: Some(why),
                    updated_at: now_iso(),
                }),
            );
            threads.detach(&start.thread_id, epoch);
            return;
        }
    };

    let mut folding = SessionState::new();
    let mut driving = Driving {
        turn: None,
        outstanding: HashMap::new(),
        interrupts: 0,
        measurements: 0,
        retunes: 0,
        pushed: HashMap::new(),
        unmeasured: false,
        drift_reported: Drift::default(),
        finished: None,
        reported_usage: None,
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
    // True once the developer has ended the session. Read at the bottom of this
    // function, where the difference it makes is that a turn cut short by *this*
    // is not reported as the agent having died: they asked.
    let mut asked_to_stop = false;

    'session: loop {
        // Anything already owed to the turn that just ended is dealt with before
        // the next one is started.
        //
        // The `select!` below cannot do this, because when a signal and a prompt
        // are both ready it takes either — and *which* matters here in a way it
        // does not anywhere else. A developer whose turn finished a moment ago
        // may have a stop click still in flight; the client sends one with no
        // `turnId`, meaning "whatever is running". Taken now it is the no-op it
        // was meant to be. Taken one iteration after the queued prompt, it stops
        // a turn that had just started, which is the developer's click landing on
        // the wrong turn.
        //
        // The window is real rather than theoretical: this loop `await`s a
        // `git add -A` at the end of every turn ([`checkpoint`]), and a click
        // during it is exactly a click after the turn the developer was watching
        // settled.
        while let Ok(signal) = signals.try_recv() {
            match signal {
                Signal::Answer(answered) => {
                    answer(&threads, &start, &mut agent, &mut driving, answered).await
                }
                Signal::AnswerUserInput(answered) => {
                    answer_user_input(&threads, &start, &mut agent, &mut driving, answered).await
                }
                Signal::Interrupt { turn_id } => {
                    interrupt(&threads, &start, &mut agent, &mut driving, turn_id).await
                }
                // Taken here as well as in the `select!` below, and it belongs
                // here most: the drain exists so that what was owed to the turn
                // that just ended is dealt with before the next one starts, and
                // a stop the developer pressed a moment ago must not be spent on
                // a turn they never saw begin.
                Signal::Stop => {
                    asked_to_stop = true;
                    break 'session;
                }
            }
        }

        // Whatever is waiting goes next, as soon as the turn before it is done.
        if accepting && driving.turn.is_none() {
            if let Some(prompt) = waiting.take() {
                // Before anything this turn publishes and before the turn itself
                // reaches the agent, which is what makes the mode this turn is
                // *requested* under the mode it is *answered* under. It moves
                // `start`, so the three session events below — `starting` from
                // the thread, `running` from here, and whatever ends the turn —
                // all report the same thing.
                //
                // **Here rather than when the prompt was taken off the channel**,
                // and that is what keeps a mode off the turn already in flight:
                // a turn queued behind a running one waits in `waiting` with its
                // own mode still attached, and spends it when its own turn comes.
                // Ticket 02's rule, and the spec's story 29.
                retune(&threads, &mut start, &mut agent, &mut driving, &prompt.wanted).await;

                // The turn is under way, and *this* is where the session enters
                // `running` — not the agent's `init` line, which a long-lived
                // child prints once for the whole conversation. Driving it off
                // `init` would leave every turn after the first in `starting`,
                // and a session that is not `running` settles the turn at the
                // first assistant message, which is the mid-turn settle the
                // client's reducer exists to avoid.
                //
                // **Before the baseline, not after**, and the difference is
                // seconds on a real repository. The client draws its working
                // indicator from this status, and the baseline below is a
                // `git add -A` over the whole project with no stat cache — a
                // fifth of a second on a scratch repo and over two on this one
                // (measured with `tools/ui-driver/first-turn.mjs`). Publishing
                // after it meant the developer pressed send and watched a pane
                // with nothing in it for as long as their repository is large,
                // which reads as the message having gone nowhere.
                //
                // Honest as well as kinder: the turn *is* running from here. The
                // developer asked for it, this loop has accepted it and is doing
                // the work that has to happen before the agent can be given it.
                // Nothing the client folds needs the prompt to have been written
                // yet, and a send that then fails settles the session on the way
                // out like any other failure.
                running(&threads, &start, &prompt.turn_id);

                // Before the agent is given the turn, which is the whole of what
                // makes it a *baseline*: everything the agent does from the next
                // line onwards is what this turn's diff will be against. Awaited
                // rather than spawned for the same reason — a capture racing the
                // agent's first edit would record a tree that already had it.
                baseline(&threads, &start).await;
                if let Err(error) = agent.send(&prompt.text).await {
                    eprintln!("laplus: cannot send a turn to the agent: {error}");
                    break;
                }
                driving.turn = Some(InFlight {
                    turn_id: prompt.turn_id.clone(),
                    assistant_message_id: None,
                    tools: HashMap::new(),
                    stopped: None,
                });
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
                // Decided and then applied, rather than applied as it is decided.
                // What the split is for is [`Decided`]; what it costs here is one
                // more local.
                let mut decided = decide(&mut folding, &mut driving, &line);
                // Taken before the changes are spent and applied after them, so
                // the developer reads the refusal and *then* the badge corrects
                // itself, rather than the badge flipping back with nothing
                // beside it saying why.
                let reverts = decided.reverts.take();
                spend(&threads, &start, decided);
                if let Some(refused) = reverts {
                    refused.revert(&mut start);
                    // Republished because the mode this turn was announced under
                    // is now known to be wrong, and the session is where the
                    // client reads it. Only while a turn is running: between
                    // turns there is no session event to correct, and the next
                    // one will carry the reverted value anyway.
                    if let Some(active) = driving.turn.as_ref().map(|turn| turn.turn_id.clone()) {
                        running(&threads, &start, &active);
                    }
                }
                // Asked before the checkpoint below, which is a `git add -A`
                // over the whole project: the meter is what the developer is
                // looking at and the question costs one line on stdin, so it
                // does not queue behind seconds of git.
                if std::mem::take(&mut driving.unmeasured) {
                    measure(&mut agent, &mut driving).await;
                }
                // The turn is over, so what the agent left behind is what this
                // turn did. Recorded before the loop takes the next prompt,
                // which is what makes the next turn's baseline this checkpoint
                // rather than a tree somebody has since typed into.
                if let Some(finished) = driving.finished.take() {
                    checkpoint(&threads, &start, &finished).await;
                }
            }
            // The agent stopped producing: it exited, or its output was
            // abandoned. Either way there is nothing more to publish.
            Next::Line(None) => break,
            Next::Signal(Some(Signal::Answer(answered))) => {
                answer(&threads, &start, &mut agent, &mut driving, answered).await;
            }
            Next::Signal(Some(Signal::AnswerUserInput(answered))) => {
                answer_user_input(&threads, &start, &mut agent, &mut driving, answered).await;
            }
            Next::Signal(Some(Signal::Interrupt { turn_id })) => {
                interrupt(&threads, &start, &mut agent, &mut driving, turn_id).await;
            }
            // The session is over. Left by leaving the loop rather than by
            // closing the agent's input and draining, which is what the *channel
            // closing* means below: everything this function does after the loop
            // is the ending a session is owed, and `Agent::stop` at the end of it
            // closes stdin, waits, and kills if waiting was not enough. A drain
            // would be a gentler ending with no bound on it, and the case this
            // command exists for is an agent that is not answering.
            Next::Signal(Some(Signal::Stop)) => {
                asked_to_stop = true;
                break;
            }
            Next::Signal(None) => listening = false,
            Next::Prompt(Some(prompt)) => waiting = Some(prompt),
            Next::Prompt(None) => {
                accepting = false;
                agent.close_input();
            }
        }
    }

    // A reply that streamed and will never be buffered, because the agent went
    // away in the middle of it. Two things are owed to it and neither is
    // optional:
    //
    // - **The client is showing it as still arriving.** A message left
    //   `streaming` is a reply the UI renders as growing, for the life of the
    //   thread, and after every restart — the same defect the ordinary
    //   reconcile is careful to avoid when a turn buffers nothing.
    // - **A delta owes the database nothing**, by design ([`crate::threads`]'s
    //   `durable`: a row per token would put the disk in the streaming path), so
    //   until a message settles there is nothing written down. Without this, the
    //   partial reply survives in memory and is gone the next time the app opens.
    //
    // Sent with **no text**, which is deliberate: the empty case is the one
    // where the accumulation stands rather than being replaced, so what the
    // developer saw stream is what is kept — and no reconciliation is recorded,
    // because none happened. A message forged out of the deltas and compared
    // against them would report the assumption as checked on a turn where
    // nothing checked it.
    if let Some(active) = driving.turn.as_mut() {
        if let Some(message_id) = active.assistant_message_id.take() {
            threads.apply(
                &start.thread_id,
                Change::AssistantMessage {
                    message_id,
                    turn_id: active.turn_id.clone(),
                    text: String::new(),
                },
            );
        }
    }

    // Whatever the agent was still waiting on is never going to be answered now,
    // and the client derives its pending-approval panel from these two kinds
    // alone — so a request left open here is a composer the developer cannot type
    // into, for the life of the conversation and across every restart after it.
    // Closing them is what makes "the session remains usable" true of the way a
    // session actually ends.
    //
    // A question is closed with *its* kinds, not these: the two folds are
    // separate (`derivePendingUserInputs` beside `derivePendingApprovals`), so an
    // `approval.resolved` over a question closes nothing and leaves exactly the
    // stuck composer this loop exists to prevent. Said as unanswerable rather
    // than as answers, because no answers were given and a row claiming some were
    // would put words in the developer's mouth.
    for asked in driving.take_outstanding() {
        let activity = match crate::worklog::questions(&asked).is_some() {
            true => crate::worklog::unanswerable_user_input(&asked.request_id),
            false => crate::worklog::resolved(
                &asked,
                Decision::Cancel,
                driving.turn.as_ref().map(|turn| turn.turn_id.clone()),
            ),
        };
        threads.apply(&start.thread_id, Change::Activity(activity));
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
    // finish, and this is the only moment anybody can say so. The refusal wins
    // when there is one: "the agent stopped before the turn finished" is true of
    // a resume that was refused and says nothing about why.
    //
    // The drift goes here too, and this is the only place it can: a turn that
    // never ends emits no `turn.completed`, so a session that died having also
    // been talking in a dialect this build could not read would otherwise report
    // the death and nothing about the dialect — which is the more likely
    // explanation of the two.
    //
    // **Unless the developer ended the session**, which is the one way a turn is
    // cut short that nobody needs telling about: they asked for the process to go
    // and it went. Reporting it as a death would put an error on a conversation
    // whose only fault was being stopped, and would settle the turn as `error`
    // rather than as the `interrupted` a `stopped` session settles it to — which
    // is the same reading `interrupted` and `stopped` already share (`CONTEXT.md`,
    // *Settling*): from the turn's point of view it did not finish, and nothing
    // went wrong.
    //
    // A stop mid-turn therefore takes that turn's unreported drift with it, and
    // that is the accepted cost rather than an oversight: the tally is reported on
    // every turn that ends on its own, and inventing a row to carry it out of a
    // session the developer ended would put a diagnostic about this build in front
    // of somebody who had just pressed stop.
    let unread = driving.drift_to_report(&folding);
    let death = (driving.turn.is_some() && !asked_to_stop)
        .then(|| died_mid_turn(complaint.as_deref(), unread));

    // Said in the conversation and not only on the session, because the session
    // is a banner and the conversation is what the developer is reading. A
    // refusal has already put its own row up, so this does not repeat it.
    if let (Some(why), None) = (&death, &refused) {
        threads.apply(
            &start.thread_id,
            Change::Activity(Activity::failed("session.failed", why)),
        );
    }

    // The slot given up before the ending is published, and **whether the
    // conversation is still this session's to describe decides whether the ending
    // is published at all.** A driver can outlive its own session: a developer who
    // stops one and sends a turn straight afterwards — which is exactly what the
    // branch toolbar does — has a new session describing this conversation while
    // this one is still being reaped. Its ending would then settle a turn that had
    // just started and report a session that had just opened as gone.
    //
    // The rows above are said either way, because they are about *this* session's
    // turn rather than about the conversation's current state: a partial reply
    // left streaming and a permission request left open are defects whoever is
    // running now.
    let ours = threads.detach(&start.thread_id, epoch);

    let failure = refused.or(death);
    if ours {
        threads.apply(
            &start.thread_id,
            Change::Session(Session {
                // **`error` rather than `stopped` for a turn cut short**, and
                // ticket 15 settled it deliberately — see ADR-0004. `stopped` is
                // available and would settle the turn as `interrupted`; it is not
                // used, because nobody asked for this and because `lastError`
                // below — the only place the developer is told the agent went away
                // mid-turn — is the sentence a session that is not in `error` has
                // nowhere to put.
                status: match failure.is_some() {
                    true => SessionStatus::Error,
                    false => SessionStatus::Stopped,
                },
                runtime_mode: start.runtime_mode.clone(),
                active_turn_id: None,
                last_error: failure,
                updated_at: now_iso(),
            }),
        );
    }

    // A turn the agent died in the middle of still changed the working tree, and
    // the developer's first question about a session that fell over is what it
    // had already done. So this ending is checkpointed like any other — the only
    // difference is that nothing published a `turn.completed` for it, which is
    // why it cannot be caught by the loop above.
    //
    // As an `error`, because that is how the session above has just reported it:
    // ADR-0004 settles a turn cut short by a dead agent as an error rather than
    // an interruption, and a checkpoint saying anything else would move the turn
    // back — see [`Ending::checkpoint_status`].
    //
    // **A session the developer ended gets no row**, and that is the same rule
    // rather than an exception to it: there is no checkpoint status that means "the
    // developer ended the session", so every one this could send would relabel the
    // turn the moment the client folded it — `error` says the turn failed, and it
    // did not. What the turn had already done to the tree falls into the diff of
    // the turn that follows, which is what a model built on photographs does with
    // an unattributed change (ADR-0008).
    //
    // Nor does a session the conversation has already replaced, for the reason the
    // ending above is conditional: the client reads a checkpoint's status back
    // into the *latest* turn, so a row for a turn two sessions ago would relabel
    // the turn running now.
    if let Some(active) = driving
        .turn
        .take()
        .filter(|_| ours && !asked_to_stop)
    {
        checkpoint(
            &threads,
            &start,
            &Finished {
                turn_id: active.turn_id,
                status: "error",
            },
        )
        .await;
    }
}

/// Record what the working tree looks like before this conversation's next turn
/// is given to the agent.
///
/// Nothing to do in the ordinary case: the checkpoint taken at the end of the
/// previous turn *is* this turn's baseline, which is what makes a conversation's
/// checkpoints a chain rather than a set of pairs. The cases where there is
/// something to do are the first turn of a conversation, and the first turn
/// after the developer ran `vcs.init` on a project that had no repository — both
/// of which are "there is no tree recorded for turn zero yet".
async fn baseline(threads: &Threads, start: &Start) {
    let turn_count = threads.checkpoint_count(&start.thread_id);
    let reference = crate::checkpoints::reference(&start.thread_id, turn_count);
    let root = std::path::PathBuf::from(&start.workspace_root);

    let recorded = tokio::task::spawn_blocking(move || {
        if crate::checkpoints::present(&root, &reference) {
            return Ok(());
        }
        crate::checkpoints::capture(&root, &reference)
    })
    .await;

    // Logged and not said in the conversation, unlike the failure at the end of
    // a turn. A baseline that could not be written is only visible as the turn
    // that follows it having no diff, and *that* is the moment the developer is
    // told — saying it twice would put two rows in the work log for one problem.
    //
    // A `JoinError` is the blocking pool having gone, which is the runtime
    // shutting down. Nothing is owed to a conversation nobody is reading.
    if let Ok(Err(why)) = recorded {
        if !crate::git::is_not_a_repository(&why) {
            eprintln!(
                "laplus: cannot record the state of {} before a turn: {}",
                start.workspace_root,
                why.detail()
            );
        }
    }
}

/// Record what the working tree looks like now as this conversation's next
/// checkpoint, and publish the row that offers the turn for review.
///
/// **A project that is not a repository is not a failure.** There is nowhere to
/// keep a checkpoint and nothing the developer did wrong; `vcs.init` is the door
/// out, and until they walk through it a conversation simply has no turns to
/// diff. Every other refusal is said in the conversation, because a turn the
/// developer cannot review is a thing they will otherwise go looking for.
async fn checkpoint(threads: &Threads, start: &Start, finished: &Finished) {
    let previous = threads.checkpoint_count(&start.thread_id);
    let turn_count = previous + 1;
    let from = crate::checkpoints::reference(&start.thread_id, previous);
    let reference = crate::checkpoints::reference(&start.thread_id, turn_count);
    let root = std::path::PathBuf::from(&start.workspace_root);

    // On a blocking thread, like every other git in this server: it is a child
    // process over a repository whose size is the developer's, and a runtime
    // worker parked on one is a worker the socket is not using.
    let taken = tokio::task::spawn_blocking({
        let reference = reference.clone();
        move || {
            crate::checkpoints::capture(&root, &reference)?;
            // Best effort, and deliberately not fatal: the summary is the line
            // above the patch, and a turn whose tree was recorded is reviewable
            // whether or not this could be read.
            Ok(crate::checkpoints::changed(&root, &from, &reference).unwrap_or_default())
        }
    })
    .await;

    let files = match taken {
        Ok(Ok(files)) => files,
        Ok(Err(why)) => {
            if !crate::git::is_not_a_repository(&why) {
                threads.apply(
                    &start.thread_id,
                    Change::Activity(Activity::failed(
                        "checkpoint.failed",
                        &format!(
                            "The state of the project after this turn could not be recorded, so \
                             the turn has no diff to review: {}",
                            why.detail()
                        ),
                    )),
                );
            }
            return;
        }
        // The blocking pool is gone, which happens when the runtime is shutting
        // down. There is nothing to say to a conversation nobody is reading.
        Err(_) => return,
    };

    threads.apply(
        &start.thread_id,
        Change::Checkpointed(Box::new(crate::threads::Checkpoint {
            turn_id: finished.turn_id.clone(),
            turn_count,
            reference,
            status: finished.status,
            files,
            // Resolved by the fold, which is where the transcript is. See
            // [`crate::threads::Threads::fold`].
            assistant_message_id: None,
            completed_at: now_iso(),
        })),
    );
}

/// What the developer is told when the agent went away in the middle of a turn.
///
/// The CLI's own last words are quoted when it left any, for the same reason
/// [`resume_refused`] quotes them: a process that died because it ran out of
/// memory, or because the machine has no credentials any more, said so on
/// stderr, and nothing this server could infer would be as useful.
fn died_mid_turn(complaint: Option<&str>, unread: Drift) -> String {
    let mut why = "The agent stopped before the turn finished.".to_string();
    if let Some(said) = complaint {
        why.push_str(&format!(" The agent said: {said}"));
    }
    if let Some(drifted) = drift_clause(unread) {
        why.push_str(&format!(" This session had {drifted}."));
    }
    why
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
    /// How much of this session's drift the developer has already been shown.
    ///
    /// The counters are the session's and monotonic, so what a report has to
    /// carry is the *difference* — a running total repeated on every turn would
    /// say the format moved on a turn where nothing did, which is the noise that
    /// trains a reader to skip the turn where it had.
    ///
    /// Kept here rather than on the turn, and that is the difference between
    /// "what this turn failed to read" and "what has gone unreported": the CLI
    /// talks *between* turns as well as during them — a rate-limit notice, a
    /// compaction boundary — and drift there belongs to somebody. Anchoring it
    /// to the start of a turn would drop it.
    drift_reported: Drift,
    /// How many times this session has asked the agent how full its window is,
    /// so the next request gets an id nothing else has used.
    measurements: u32,
    /// How many mode and model pushes this session has sent, on the same terms.
    retunes: u32,
    /// Pushes written to the agent and not yet answered, by the id this server
    /// minted.
    ///
    /// The answer is the only thing that says a push landed, and a refusal has to
    /// put [`Start`] back — so what it was before travels here rather than being
    /// re-derived from a thread that has since moved on. See [`Pushed`].
    pushed: HashMap<String, Pushed>,
    /// A moment has arrived that only the agent can settle, and the loop has not
    /// asked it yet.
    ///
    /// The same one-line handoff from the fold to the loop that `finished` is,
    /// and for the same reason: asking is a *write* to the agent and so has to
    /// happen where the loop can `await` it, while noticing that the moment
    /// arrived happens where the lines are read, which is synchronous.
    unmeasured: bool,
    /// A turn that has just ended and whose working tree has not been recorded
    /// yet.
    ///
    /// A one-line handoff from the fold to the loop, and it exists because the
    /// two halves cannot swap places. Recording a checkpoint is a `git` and
    /// therefore has to happen where the loop can `await` it; knowing that a
    /// turn ended happens where the lines are read, which is synchronous.
    /// Written by exactly one arm of [`decide`] and taken by the loop on the
    /// next statement, so it is never carried across an iteration.
    ///
    /// **Left here rather than moved onto [`Decided`]**, along with `unmeasured`
    /// above. Both are already what that type is — a thing decided on one line and
    /// spent on the next — but both are read by the loop itself rather than by
    /// [`spend`], and neither is a change to a conversation. Moving them would
    /// have widened `Decided` from "what happened to the thread" to "what the
    /// driver does next" for no assertion this file could not already make: they
    /// are `&mut Driving` fields, and a test of `decide` reads them off the
    /// `Driving` it handed in.
    finished: Option<Finished>,
    /// The last context-window reading the client was told about.
    ///
    /// Kept so an unchanged reading is not published again. The CLI repeats its
    /// counts — the `result` at the end of a turn usually agrees with the last
    /// assistant message of that turn — and a row per repetition would be a row
    /// per message on the thread, each one saying what the one before it said.
    /// Per session rather than per turn, because the repetition crosses the
    /// boundary: a turn that asked the agent nothing new ends where the last one
    /// did.
    reported_usage: Option<TokenUsage>,
}

/// A turn that ended in a way a checkpoint can describe.
///
/// See [`Ending::checkpoint_status`] for why the second field is what decides
/// whether there is anything to hand over at all.
struct Finished {
    turn_id: String,
    status: &'static str,
}

impl Driving {
    /// The id for the next interrupt, and the count that makes it unique.
    fn next_interrupt_id(&mut self) -> String {
        self.interrupts += 1;
        format!("interrupt-{}", self.interrupts)
    }

    /// The id for the next context-window question.
    ///
    /// Distinct from an interrupt's for the developer reading a log rather than
    /// for the code: the answer is told apart by what it carries, not by the id
    /// it names — see [`crate::protocol::Acknowledgement::reading`].
    fn next_measurement_id(&mut self) -> String {
        self.measurements += 1;
        format!("context-{}", self.measurements)
    }

    /// The id for the next mode or model push.
    ///
    /// One counter for both, because they are one kind of request as far as the
    /// answer is concerned: the id is what says *which* push a refusal is about,
    /// and [`Pushed`] carries which kind it was.
    fn next_retune_id(&mut self) -> String {
        self.retunes += 1;
        format!("retune-{}", self.retunes)
    }

    /// What has gone unread since the last time anybody was told, and a note
    /// that they have now been told it.
    fn drift_to_report(&mut self, folding: &SessionState) -> Drift {
        let unreported = folding.drift().since(self.drift_reported);
        self.drift_reported = folding.drift();
        unreported
    }

    /// The context-window row the client has not been told yet, if the reading
    /// has moved, and a note that it has now been told.
    ///
    /// `None` when nothing has changed, which is most lines: the counts arrive
    /// on every assistant message and on the `result`, and the last two of those
    /// usually agree.
    fn usage_to_report(&mut self, folding: &SessionState) -> Option<TokenUsage> {
        let reading = folding.token_usage.clone()?;
        if self.reported_usage.as_ref() == Some(&reading) {
            return None;
        }
        self.reported_usage = Some(reading.clone());
        Some(reading)
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

/// How full the context window is, as the row the composer's meter reads.
///
/// The client never renders this row in the work log — `session-logic.ts` skips
/// the kind in `deriveWorkLogEntries` — it walks the activity list backwards for
/// the newest one and draws the meter from it
/// (`apps/web/src/lib/contextWindow.ts`, `deriveLatestContextWindowSnapshot`).
/// So the row is a carrier rather than something a developer reads, and its
/// summary exists only because the contract requires a non-empty one.
fn context_window_row(usage: &TokenUsage, turn_id: Option<String>) -> Activity {
    Activity::info(
        "context-window.updated",
        "Context window updated",
        json!({
            "usedTokens": usage.used_tokens,
            // The same number as `usedTokens` on this path. The contract carries
            // both because a provider that compacts mid-turn ends below where it
            // peaked, and the client shows the peak as the turn's cost.
            "lastUsedTokens": usage.used_tokens,
            "totalProcessedTokens": usage.total_processed_tokens,
            "maxTokens": usage.max_tokens,
            "inputTokens": usage.input_tokens,
            "outputTokens": usage.output_tokens,
            // On every row rather than only on the one the CLI's answer
            // produced, because the client reads the newest row and does not
            // merge it with the ones before it
            // (`deriveLatestContextWindowSnapshot`). A row without this is a row
            // that says auto-compact is off.
            "compactsAutomatically": usage.compacts_automatically,
        }),
        turn_id,
    )
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
        eprintln!("laplus: cannot answer a permission request: {error}");
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

/// Tell the agent what the developer answered, and say so in the conversation.
///
/// [`answer`]'s twin for `AskUserQuestion`, and the same shape for the same
/// reasons: the agent is written to first because it is the half that has stopped,
/// and the row goes up either way because the alternative is a question header the
/// developer can never clear.
///
/// Two things it does *not* have. There is no cancel — the composer offers no way
/// to refuse a question, so no answer here can end a turn. And there is no
/// decision to refuse: [`crate::worklog::Decision::parse`] guards the approval
/// path against a verb this server cannot read, while answers are the developer's
/// own text and are carried through unread.
async fn answer_user_input(
    threads: &Threads,
    start: &Start,
    agent: &mut Agent,
    driving: &mut Driving,
    answered: UserInputAnswered,
) {
    // An id this session never asked about, or has already been answered on.
    // Closed with the wording the client folds as "that question is gone", which
    // is what clears a header left behind by a session that died holding one.
    let Some(asked) = driving.outstanding.remove(&answered.request_id) else {
        threads.apply(
            &start.thread_id,
            Change::Activity(crate::worklog::unanswerable_user_input(
                &answered.request_id,
            )),
        );
        return;
    };

    let sent = agent
        .answer(
            &asked.request_id,
            &crate::worklog::answers_for(&asked, &answered.answers),
        )
        .await;

    if let Err(error) = sent {
        eprintln!("laplus: cannot answer a question: {error}");
        threads.apply(
            &start.thread_id,
            Change::Activity(Activity::failed(
                "session.failed",
                &format!(
                    "The answers could not be sent to the agent, which is no longer reading: \
                     {error}"
                ),
            )),
        );
    }

    threads.apply(
        &start.thread_id,
        Change::Activity(crate::worklog::user_input_resolved(
            &asked.request_id,
            &answered.answers,
            driving.turn.as_ref().map(|turn| turn.turn_id.clone()),
        )),
    );
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
        eprintln!("laplus: cannot interrupt the agent: {error}");
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

/// Ask the agent how full its context window is.
///
/// **Nothing is published, and a failure is not reported.** That is the whole
/// difference between this and every other write to the agent, and it is what
/// makes the request safe to add to a build whose CLI may not implement it: a
/// stop that did not land is a button that did nothing and has to be said, while
/// a question that did not land leaves the meter exactly as it was — filled from
/// the token counts, the way ticket 40 filled it. A row reading "the agent would
/// not say how full its context window is" would be the first thing a developer
/// saw on an older CLI, about a number already on screen.
///
/// The answer comes back on stdout whenever the agent gets to it, and
/// [`crate::protocol::SessionState::reduce`] folds it. Nothing here waits, and
/// no state records that a question is outstanding — a second question asked
/// before the first was answered is two readings rather than a problem, and the
/// later of them wins because it arrives later.
async fn measure(agent: &mut Agent, driving: &mut Driving) {
    let request_id = driving.next_measurement_id();
    if let Err(error) = agent.measure_context(&request_id).await {
        eprintln!("laplus: cannot ask the agent how full its context window is: {error}");
    }
}

/// Move the child onto the mode and model this turn asks for, before it is given
/// the turn.
///
/// The whole of ticket 11. `--permission-mode` and `--model` are read once, when
/// the process is opened, and one process serves a whole conversation — so
/// before this a developer who tightened `full-access` to `approval-required`
/// saw the picker move, saw it survive a restart, saw the next turn *requested*
/// under the new mode, and was answered by an agent still bypassing permissions.
///
/// Four rules, and each of them is an acceptance criterion:
///
/// - **Nothing is sent for a value that has not moved.** The comparison is
///   against [`Start`], which is what the child is running under rather than what
///   the thread last said — so a conversation whose mode never changes never
///   sends one of these, and upstream's guard-on-change is followed rather than
///   re-derived.
/// - **The capture moves only when the write did.** `start` is what every session
///   event reads, so setting it before the line was written would publish a claim
///   about a child nobody had managed to tell.
/// - **The refusal is left to the acknowledgement.** The CLI answers on stdout,
///   and a mode it will not take comes back as a `control_response` naming this
///   id — see [`decide`]'s `Acknowledged` arm, which puts `start` back and says
///   so in the conversation. What is reported *here* is the narrower failure of
///   the child no longer reading at all.
/// - **A model is pushed only when there is one to push.** A selection that names
///   none cannot be expressed as a request — there is nothing that means "go back
///   to your default" — so the child keeps what it has, and `start` keeps saying
///   so.
async fn retune(
    threads: &Threads,
    start: &mut Start,
    agent: &mut Agent,
    driving: &mut Driving,
    wanted: &Retune,
) {
    // Decided before anything is written, so "has it moved" is asked of the same
    // capture both questions are asked of — and so the loop below is one shape
    // rather than two nearly-identical ones.
    let mode = (wanted.runtime_mode != start.runtime_mode).then(|| Pushed::Mode {
        previous: start.runtime_mode.clone(),
        asked: wanted.runtime_mode.clone(),
    });
    let model = wanted
        .model
        .clone()
        .filter(|wanted| Some(wanted) != start.model.as_ref())
        .map(|asked| Pushed::Model {
            previous: start.model.clone(),
            asked,
        });

    for asked in [mode, model].into_iter().flatten() {
        let request_id = driving.next_retune_id();
        let written = match &asked {
            Pushed::Mode { asked, .. } => {
                let mode = crate::agent::pushed_permission_mode_for(asked);
                agent.set_permission_mode(&request_id, mode).await
            }
            Pushed::Model { asked, .. } => agent.set_model(&request_id, asked).await,
        };

        match written {
            Ok(()) => {
                asked.apply(start);
                driving.pushed.insert(request_id, asked);
            }
            Err(error) => unreachable_agent(threads, start, &asked, &error),
        }
    }
}

/// The agent stopped reading before it could be told what this turn wants.
///
/// Reported rather than swallowed, unlike [`measure`]'s failure and for the
/// reason a failed interrupt is: the developer changed something and was shown
/// the change, so a change that reached nothing has to be said. The turn goes on
/// regardless — refusing to send it would leave a conversation with a message in
/// it and nothing running, which is worse than a turn answered under the old
/// rules with a row saying so.
fn unreachable_agent(threads: &Threads, start: &Start, asked: &Pushed, error: &std::io::Error) {
    let what = asked.what();
    eprintln!("laplus: cannot tell the agent its {what}: {error}");
    threads.apply(
        &start.thread_id,
        Change::Activity(Activity::failed(
            "session.retune-failed",
            &format!(
                "The agent could not be told which {what} this turn wants, because it is no \
                 longer reading: {error}"
            ),
        )),
    );
}

/// One thing this server asked a running child to become, and what it was before.
///
/// Held from the moment the line is written until the CLI answers it, because
/// the answer is the only thing that says whether it landed — and a refusal has
/// to put [`Start`] back to the value the child is really running under.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Pushed {
    Mode { previous: String, asked: String },
    Model { previous: Option<String>, asked: String },
}

impl Pushed {
    /// Which of the two this is, in the words the developer's own picker uses.
    ///
    /// Read off the variant rather than passed alongside it, so there is one
    /// place that decides what a mode and a model are called.
    fn what(&self) -> &'static str {
        match self {
            Pushed::Mode { .. } => "runtime mode",
            Pushed::Model { .. } => "model",
        }
    }

    /// Take the driver's capture to what the child was just asked to become.
    ///
    /// Called only where the write succeeded, which is the whole of the rule:
    /// [`Start`] is what every session event reads, so moving it before the line
    /// was written would publish a claim about a child nobody had told.
    fn apply(&self, start: &mut Start) {
        match self {
            Pushed::Mode { asked, .. } => start.runtime_mode = asked.clone(),
            Pushed::Model { asked, .. } => start.model = Some(asked.clone()),
        }
    }

    /// What the developer is told, when the CLI would not take it.
    ///
    /// Names what was refused, in this server's own vocabulary rather than the
    /// CLI's: the developer picked `approval-required` in a menu and has never
    /// seen the word `default`.
    fn sentence(&self, why: &str) -> String {
        match self {
            Pushed::Mode { asked, previous } => format!(
                "The agent would not change to the '{asked}' runtime mode and is still running \
                 under '{previous}': {why}"
            ),
            Pushed::Model { asked, previous } => match previous {
                Some(previous) => format!(
                    "The agent would not change to the '{asked}' model and is still running \
                     '{previous}': {why}"
                ),
                None => format!(
                    "The agent would not change to the '{asked}' model and is still running the \
                     one it started with: {why}"
                ),
            },
        }
    }

    /// Put the driver's capture back to what the child is really running under.
    fn revert(self, start: &mut Start) {
        match self {
            Pushed::Mode { previous, .. } => start.runtime_mode = previous,
            Pushed::Model { previous, .. } => start.model = previous,
        }
    }
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
            status: SessionStatus::Running,
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

    /// What the session becomes.
    ///
    /// This is the **encoder** — the one step on this path that knows about the
    /// `claude` CLI, which is why it lives beside the driver rather than in
    /// [`crate::settling`]. Upstream keeps its equivalents per-provider for the
    /// same reason. Reading these back out is
    /// [`crate::settling::SessionStatus::settles_turn_as`]'s job, and the two
    /// are not inverses: `ready` settles a turn as `completed`.
    fn session_status(self) -> SessionStatus {
        match self {
            Ending::Completed => SessionStatus::Ready,
            Ending::Failed => SessionStatus::Error,
            Ending::Stopped => SessionStatus::Interrupted,
        }
    }

    /// The `OrchestrationCheckpointStatus` a checkpoint for this ending would
    /// carry — or `None`, meaning **do not record one at all**.
    ///
    /// The third case is the interesting one and it is a decision, not an
    /// omission. `status` is not a fact about the capture; it is how the turn
    /// went, and the client reads it back as exactly that: `threadReducer.ts`,
    /// `case "thread.turn-diff-completed"`, sets `latestTurn.state` to
    /// `checkpointStatusToTurnState(status)` on every checkpoint it folds. That
    /// function has three inputs and two outputs — `ready` and `missing` both
    /// mean `completed`, `error` means `error` — so **there is no status that
    /// means "the developer stopped this turn"**.
    ///
    /// A turn the developer stopped therefore gets no row, because every row it
    /// could get would relabel it as finished — undoing the settle that ticket
    /// 14 exists for, and leaving this server and the client disagreeing about
    /// the same turn. The tree is not captured for one either: its changes fall
    /// into the diff of the turn that follows, which is the honest thing for a
    /// model built on photographs of the working tree. See ADR-0008.
    ///
    /// Upstream sends `missing` here (`CheckpointReactor.ts`,
    /// `checkpointStatusFromRuntime`) and takes the relabelling; laplus does
    /// not, because ticket 14 made "a stopped turn reads as interrupted" a
    /// promise this build keeps.
    fn checkpoint_status(self) -> Option<&'static str> {
        match self {
            Ending::Completed => Some("ready"),
            Ending::Failed => Some("error"),
            Ending::Stopped => None,
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

/// What one line of the agent's NDJSON turned out to mean for the conversation.
///
/// [`decide`] answers with one of these and [`spend`] applies it, which is the
/// same seam `docs/adr/0025` cut one level down: the fold there reports the
/// reconciliation verdict rather than spending it, because a function that
/// applies its own results can only be tested by watching what it did to a live
/// world. Here the world is a [`Threads`] with a running `claude` behind it, so
/// there was nothing to watch it with at all.
///
/// **Three fields rather than a `Vec<Change>`**, because two of the twelve things
/// [`decide`] used to do are not changes:
///
/// - the agent's own session id is written down and published to nobody
///   ([`crate::threads::Threads::remember_agent_session`]), so no `Change`
///   describes it and none could;
/// - the event that ends a turn is published *only if the session is still
///   describing that turn*, which is a question about the world asked between two
///   applies. Returned as a precondition rather than answered here, so the lock it
///   costs is taken on the lines that end a turn rather than on every line.
///
/// See `docs/adr/0027`.
#[derive(Debug, Default)]
struct Decided {
    /// The changes to apply, in the order they were decided — which is the order
    /// the developer saw the work happen, because the CLI closes one content
    /// block before announcing the next.
    changes: Vec<Change>,
    /// The `claude` session the agent has just announced, if it announced one.
    agent_session: Option<String>,
    /// The turn ended, and how. `None` on every line that is not a `result`.
    settles: Option<Settles>,
    /// A push the CLI refused, and what the driver's capture of the session has
    /// to go back to.
    ///
    /// A fourth field for the same reason the two above are not changes: putting
    /// [`Start`] back is not something a conversation can be told, and the
    /// re-publish that follows it needs a value this function does not hold. So
    /// the refusal's *row* travels in `changes` and the correction travels here,
    /// and the loop spends both — see `docs/adr/0027`.
    reverts: Option<Pushed>,
}

/// The end of a turn, as the line that ended it reports it.
///
/// Two of the five fields of a [`Session`] and not the other three, and the split
/// is the seam rather than an arbitrary cut: **how the turn went** is what this
/// line decided, while **which session and when** are facts the driver has held
/// since it started the child. Leaving the second pair to [`spend`] is what takes
/// the last clock read out of [`decide`].
#[derive(Debug)]
struct Settles {
    /// The turn that ended — and the turn the session must *still* be describing
    /// for the ending to be published at all.
    ///
    /// `None` is a `result` that arrived with no turn in flight, and it is a
    /// value rather than a missing one: the session's `activeTurnId` is then also
    /// `None`, and the two matching is what makes this publishable.
    turn_id: Option<String>,
    status: SessionStatus,
    last_error: Option<String>,
}

/// Fold one line and say what it turned out to be.
///
/// No [`Threads`], no lock, no clock and no child process: a [`SessionState`], a
/// [`Driving`] and a line in, a [`Decided`] out. That is the whole of what this
/// commit bought and the whole of why the tests at the bottom of this file can
/// exist — see [`Decided`] and `docs/adr/0027`.
///
/// It is **not** pure, and the difference is worth being exact about rather than
/// claiming otherwise in a doc comment. It advances the reducer, it mutates the
/// turn in flight, and the five [`Activity`] constructors it calls read a clock
/// and mint identifiers exactly as ADR-0025 left them. What changed is that it
/// returns its results instead of applying them.
fn decide(folding: &mut SessionState, driving: &mut Driving, line: &str) -> Decided {
    let folded = folding.fold_line(line);
    let mut decided = Decided::default();

    // How full the context window is, reported here rather than from each arm
    // that might have moved it. Two kinds of line carry the counts — every
    // assistant message during a turn, and the `result` that ends one — and both
    // are already handled below for other reasons; asking once, where the answer
    // is the same question, keeps the arms about what they are about.
    //
    // Ahead of the match rather than inside it, which is what puts this in front
    // of `turn.completed` on the line that ends a turn: the row a developer
    // actually reads stays the last thing in it. The client never renders this
    // one — see [`context_window_row`].
    let moved = driving.usage_to_report(folding);
    if let Some(usage) = moved {
        let turn_id = driving.turn.as_ref().map(|turn| turn.turn_id.clone());
        decided
            .changes
            .push(Change::Activity(context_window_row(&usage, turn_id)));
    }

    // The answer to a mode or model push, taken before anything below can borrow
    // the turn — and taken whether or not one is running, which is the difference
    // between this and the interrupt acknowledgement it shares an envelope with.
    // A push is about the *session*, so its answer means the same thing between
    // turns as during one.
    //
    // Silence is success: the CLI answers `{"subtype": "success"}` and there is
    // nothing to do about a change that landed, because `retune` moved the
    // capture when it wrote the line. Only a refusal has work in it.
    if let Folded::Acknowledged(answer) = &folded {
        if let Some(asked) = driving.pushed.remove(&answer.request_id) {
            if let Some(why) = answer.refusal() {
                decided.changes.push(Change::Activity(Activity::failed(
                    "session.retune-refused",
                    &asked.sentence(&why),
                )));
                decided.reverts = Some(asked);
            }
            return decided;
        }
    }

    let turn = &mut driving.turn;
    match folded {
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
            let turn_id = turn.as_ref().map(|turn| turn.turn_id.clone());
            // Two rows for one wire shape, and which one is decided here rather
            // than by the client: an `AskUserQuestion` is a question the composer
            // can render as one, and everything else — including an
            // `AskUserQuestion` this build cannot read as a question — is a
            // permission the panel can render as one. See
            // [`crate::worklog::questions`] for why the unreadable case must fall
            // through to the approval row rather than to nothing.
            let activity = match crate::worklog::questions(&asked) {
                Some(questions) => {
                    crate::worklog::user_input_requested(&asked, questions, turn_id)
                }
                None => crate::worklog::requested(&asked, turn_id),
            };
            decided.changes.push(Change::Activity(activity));
            // Held either way: the request is outstanding because the *agent* is
            // waiting, which is a fact about the protocol rather than about which
            // row the developer was shown.
            driving.outstanding.insert(asked.request_id.clone(), asked);
        }

        // The agent announcing itself. Once per process rather than per turn.
        //
        // **Nothing is published.** The id is written down and that is all, which
        // is the one thing this event is load-bearing for: it is what the next
        // run is given as `--resume`. A resumed session announces an id too — the
        // CLI is free to hand back a new one — so the thread always holds the
        // most recent.
        //
        // There used to be a row here as well, reading "Claude Code session
        // started · model … · permission mode … · N tools". It is gone because
        // upstream has no equivalent and this is a fork measured against it: the
        // row was the first visibly non-upstream thing in every transcript, and
        // it opened each conversation by restating two facts the UI already
        // shows — the model in the composer's picker, the mode in its runtime
        // selector.
        //
        // What that gives up is worth naming, because it is not nothing: the
        // model and mode shown elsewhere are what the agent was *asked* for, and
        // the row carried what the CLI reported as actually in force after
        // applying the developer's own settings file over it. The two can
        // differ, and now nothing says so. Ticket 12 asked for that visibility;
        // this is a later decision against it, not an oversight.
        Folded::Initialized => {
            decided.agent_session = folding.session_id.clone();
            // The first of the two moments worth asking about, and the one
            // upstream does not have: it asks as a turn *completes*, which
            // leaves the opening turn of a session drawing a bare token count
            // with no window behind it, because `modelUsage` has not arrived
            // yet. Here the CLI has loaded its system prompt, its tools and its
            // skills and knows exactly how much room they took — and answers
            // while the turn it is announcing is still running.
            driving.unmeasured = true;
        }

        // A row and nothing else, which is the whole of the criterion: the
        // transcript is this server's own copy and no branch here touches it.
        //
        // Reported rather than merely tolerated, because it explains something a
        // developer would otherwise experience as the agent losing the thread —
        // a follow-up that refers to what is plainly on screen may be answered
        // by an agent that no longer has it.
        Folded::Compacted(compaction) => {
            decided.changes.push(Change::Activity(Activity::info(
                "session.compacted",
                &compaction_summary(&compaction),
                json!({
                    "trigger": compaction.trigger,
                    "preTokens": compaction.pre_tokens,
                    "postTokens": compaction.post_tokens,
                    "detail": "The agent summarised the conversation to make room. \
                               Everything above is still here — what changed is how much \
                               of it the agent can still see.",
                }),
                turn.as_ref().map(|turn| turn.turn_id.clone()),
            )));
        }

        // The account's standing with the API, when it is worth saying. Surfaced
        // rather than swallowed because it is the difference between a turn that
        // is slow and a turn that is not going to happen — and because a
        // developer who is refused with no explanation has nothing to act on.
        Folded::RateLimited(limit) => {
            let summary = rate_limit_summary(&limit);
            let mut told = Activity::info(
                "session.rate-limited",
                &summary,
                json!({
                    "status": limit.status,
                    "limit": limit.limit,
                    "resetsAt": limit.resets_at.map(reset_time),
                    "detail": summary,
                }),
                turn.as_ref().map(|turn| turn.turn_id.clone()),
            );
            // A refusal is a failure; being close to a limit is not yet one.
            if limit.rejected() {
                told.tone = "error";
            }
            decided.changes.push(Change::Activity(told));
        }

        Folded::Streamed(text) => {
            let Some(active) = turn.as_mut() else {
                return decided;
            };
            let message_id = active
                .assistant_message_id
                .get_or_insert_with(crate::threads::fresh_message_id)
                .clone();
            decided.changes.push(Change::AssistantDelta {
                message_id,
                turn_id: active.turn_id.clone(),
                text,
            });
        }

        Folded::Turn { index } => {
            let Some(active) = turn.as_mut() else {
                return decided;
            };
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
                decided.changes.push(Change::AssistantMessage {
                    message_id,
                    turn_id: active.turn_id.clone(),
                    text: completed.text.clone(),
                });
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
                    decided.changes.push(Change::Activity(activity));
                }
            }
        }

        // The agent answering the one thing this server asks it. Matched against
        // the request that is outstanding, because an acknowledgement for some
        // other id — a late one, for a turn that has already ended — says nothing
        // about the turn running now.
        // The agent said how full its window is. Nothing to do here: the reading
        // was folded where it landed, and the block at the top of this function
        // has already published the row — it reads `folding.token_usage` after
        // the fold, so an answer that moved the meter moves it on the same line
        // it arrived on.
        Folded::Measured => {}

        Folded::Acknowledged(acknowledged) => {
            let Some(active) = turn.as_mut() else {
                return decided;
            };
            if !active.awaiting(&acknowledged.request_id) {
                return decided;
            }
            let Some(why) = acknowledged.refusal() else {
                return decided;
            };

            active.carries_on();
            decided.changes.push(Change::Activity(Activity::failed(
                "turn.interrupt-failed",
                &format!("The agent would not stop the turn: {why}"),
            )));
        }

        Folded::Completed => {
            let finished = driving.turn.take();
            let active = finished.as_ref().map(|turn| turn.turn_id.clone());
            let summary = folding.last_result.as_ref();
            let ending = Ending::of(finished.as_ref(), summary);
            // Before the early return below, because the working tree has to be
            // recorded whether or not this turn is still the one the session is
            // describing. A developer who stopped the agent and typed again has
            // *two* turns to review, and the one that just ended is the one this
            // is about.
            driving.finished = active
                .clone()
                .zip(ending.checkpoint_status())
                .map(|(turn_id, status)| Finished { turn_id, status });
            // The second moment worth asking about, and upstream's own timing —
            // `completeTurn` in `ClaudeAdapter.ts`. What it settles that the
            // counts cannot: a turn that reached no API has a `usage` of zeroes
            // and leaves the meter reading whatever it read before, and a turn
            // that compacted mid-way ends carrying far less than its counts
            // added up to. Asked here rather than only at the start of a session
            // because the conversation is what changed, and the conversation is
            // what the reading is of.
            driving.unmeasured = true;
            // What has gone unread and unreported. The session's running totals
            // go in the payload beside it; the sentence gets what is new, so a
            // turn that drifted says so and the one after it does not repeat the
            // claim.
            let drift = driving.drift_to_report(folding);

            let completed = Activity {
                tone: ending.tone(),
                ..Activity::info(
                    "turn.completed",
                    &turn_summary(folding, ending, drift),
                    json!({
                        "durationMs": summary.and_then(|result| result.duration_ms),
                        "totalCostUsd": summary.and_then(|result| result.total_cost_usd),
                        "numTurns": summary.and_then(|result| result.num_turns),
                        "stopReason": summary.and_then(|result| result.stop_reason.clone()),
                        "isError": ending.failed(),
                        "interrupted": ending.stopped(),
                        // The drift accounting for this session, next to the turn
                        // it accumulated over — so a CLI that moved shows up where
                        // a developer is already looking. Session totals rather
                        // than this turn's, because the question a number answers
                        // is "how much of this build has the CLI outgrown"; the
                        // summary above carries the turn's own.
                        "unknownEvents": folding.unknown_events,
                        "parseErrors": folding.parse_errors,
                    }),
                    active.clone(),
                )
            };
            decided.changes.push(Change::Activity(completed));

            // Leaving `running` is what ends the turn for the client, so this
            // is the event that settles it — and the reason a turn's reported
            // duration covers the whole turn rather than stopping at the last
            // thing the assistant said. `interrupted` is one of the contract's
            // own session statuses and settles the turn as interrupted with it,
            // which is what keeps the partial reply on screen marked as what it
            // is rather than as an answer.
            //
            // Carried out of here rather than published, and the turn id beside
            // it is the whole reason: it may go up **only while the session is
            // still describing this turn**. A developer who stopped the agent can
            // send the next turn while this one is still winding down — that is
            // the whole point of stopping it — and the dispatch has already moved
            // the session on to that turn. Published unconditionally it would
            // settle a turn that had just started, and the client would show it
            // finished until the agent got round to it. [`spend`] asks.
            decided.settles = Some(Settles {
                turn_id: active,
                status: ending.session_status(),
                last_error: ending
                    .failed()
                    .then(|| turn_summary(folding, ending, drift)),
            });
        }
    }

    decided
}

/// Apply what [`decide`] decided.
///
/// The impure half of the pair, and it is deliberately this short: every line of
/// it is a call on [`Threads`], so there is nothing here to get wrong that a test
/// of `decide` would not already have caught.
///
/// The order is the order the arms decided in, and the two things that are not
/// changes go where they went before the split — the id after the row that may
/// have preceded it on the same line, the ending last of all.
fn spend(threads: &Threads, start: &Start, decided: Decided) {
    for change in decided.changes {
        threads.apply(&start.thread_id, change);
    }

    if let Some(session_id) = &decided.agent_session {
        threads.remember_agent_session(&start.thread_id, session_id);
    }

    let Some(settles) = decided.settles else { return };
    // The question [`Settles::turn_id`] exists to ask, and the lock it costs is
    // taken here — on the lines that end a turn — rather than on every line the
    // agent writes.
    if threads.active_turn(&start.thread_id) != settles.turn_id {
        return;
    }

    threads.apply(
        &start.thread_id,
        Change::Session(Session {
            status: settles.status,
            // The session's own, not the line's: a turn ends with the latitude
            // the child was started with, whatever the thread has been moved to
            // since. See `CONTEXT.md`, *Runtime mode*.
            runtime_mode: start.runtime_mode.clone(),
            active_turn_id: None,
            last_error: settles.last_error,
            updated_at: now_iso(),
        }),
    );
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
fn turn_summary(state: &SessionState, ending: Ending, drift: Drift) -> String {
    let mut summary = match &state.last_result {
        None => format!("{}.", ending.opening()),
        Some(result) => {
            let mut summary = ending.opening().to_string();
            if let Some(duration) = result.duration_ms {
                summary.push_str(&format!(" in {}", human_duration(duration)));
            }
            if let Some(cost) = result.total_cost_usd {
                // Four decimal places: a short turn costs a fraction of a cent,
                // and two would round every one of them to zero.
                summary.push_str(&format!(" · ${cost:.4}"));
            }
            if let Some(reason) = &result.stop_reason {
                summary.push_str(&format!(" · {reason}"));
            }
            // The agent's own account of what went wrong, on a turn that went
            // wrong. Without it the sentence is "Turn failed" and the developer
            // has nothing to decide with — and this sentence is also the
            // session's `lastError`, which is the banner they are looking at.
            //
            // Only on a failure: the CLI reports a turn the developer *stopped*
            // as a failed one, and quoting its diagnostics there would show them
            // an error for having pressed stop.
            if let (true, Some(said)) = (ending.failed(), result.error.as_deref()) {
                summary.push_str(&format!(" · {said}"));
            }
            summary
        }
    };

    // Last, because it is about this build rather than about the turn.
    if let Some(drifted) = drift_clause(drift) {
        summary.push_str(&format!(" · {drifted}"));
    }
    summary
}

/// What a turn failed to read, as a clause a developer can see.
///
/// The counters are the project's early-warning system for its most externally
/// volatile dependency, and a number nobody renders is not a warning. The
/// payload beside this row already carried both totals before ticket 15; what it
/// did not do was put either one anywhere the UI shows, so a `claude` release
/// that moved the format would still have been learned from a bug report.
///
/// `None` on a clean turn, which is almost all of them — a clause saying nothing
/// went unread on every turn would be noise that trained the developer to skip
/// the one that mattered.
fn drift_clause(drift: Drift) -> Option<String> {
    if drift.is_clean() {
        return None;
    }
    let mut said = Vec::new();
    if drift.unknown_events > 0 {
        said.push(format!(
            "{} unrecognised {}",
            drift.unknown_events,
            plural(drift.unknown_events, "event", "events")
        ));
    }
    if drift.parse_errors > 0 {
        said.push(format!(
            "{} unreadable {}",
            drift.parse_errors,
            plural(drift.parse_errors, "line", "lines")
        ));
    }
    Some(said.join(" and "))
}

fn plural(count: usize, one: &'static str, many: &'static str) -> &'static str {
    match count {
        1 => one,
        _ => many,
    }
}

/// What the developer is told when the agent compacted its own context.
///
/// The trigger is named because the two are different events to a reader: one is
/// the conversation having outgrown the window, the other is somebody having
/// asked. The token counts are shown when the CLI gave them, because "how much
/// was thrown away" is the only quantity here that predicts what the agent will
/// have forgotten.
fn compaction_summary(compaction: &Compaction) -> String {
    let mut summary = match compaction.trigger.as_deref() {
        Some("auto") => "Context compacted automatically".to_string(),
        Some("manual") => "Context compacted at the developer's request".to_string(),
        Some(other) => format!("Context compacted ({other})"),
        None => "Context compacted".to_string(),
    };
    if let (Some(before), Some(after)) = (compaction.pre_tokens, compaction.post_tokens) {
        summary.push_str(&format!(
            " · {} tokens → {}",
            thousands(before),
            thousands(after)
        ));
    }
    summary
}

/// What the developer is told about the account's standing.
///
/// The reset time is the whole reason this is worth a row: "you have run out"
/// with no answer to "until when" leaves the developer with nothing to plan
/// around.
fn rate_limit_summary(limit: &RateLimit) -> String {
    let mut summary = match limit.status.as_str() {
        "rejected" => "The agent's usage limit has been reached".to_string(),
        "allowed_warning" => "The agent is close to its usage limit".to_string(),
        // A standing this build has never seen. Named rather than described:
        // saying "close to its limit" about a word nobody read would be
        // asserting something the agent did not say.
        unknown => format!("The agent reported its usage standing as '{unknown}'"),
    };
    if let Some(kind) = &limit.limit {
        summary.push_str(&format!(" ({kind})"));
    }
    if let Some(resets_at) = limit.resets_at {
        summary.push_str(&format!(" · resets at {}", reset_time(resets_at)));
    }
    summary
}

/// The CLI's Unix-seconds reset stamp, as the timestamp the rest of this wire
/// speaks. A number of seconds since 1970 is not something a developer reads,
/// and every other instant the client is handed is an `IsoDateTime`.
fn reset_time(seconds: i64) -> String {
    iso_from_epoch(seconds.max(0) as u64, 0)
}

/// A token count with separators, because six digits without them is not a
/// number anybody reads at a glance.
fn thousands(count: u64) -> String {
    let digits = count.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (position, digit) in digits.chars().enumerate() {
        if position > 0 && (digits.len() - position).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
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
            error: None,
            token_usage: None,
        });
        state
    }

    /// A driver's capture of what its child is running under. Only the two
    /// fields a push can move are interesting here; the rest are what the tests
    /// that need a real session build.
    fn a_start() -> Start {
        Start {
            thread_id: "thread-1".to_string(),
            workspace_root: ".".to_string(),
            model: None,
            runtime_mode: "full-access".to_string(),
            resume: None,
            settings: ClaudeSettings {
                enabled: true,
                binary_path: "claude".to_string(),
                home_path: String::new(),
                launch_args: String::new(),
                custom_models: Vec::new(),
            },
        }
    }

    /// A turn in flight, however it is going.
    fn in_flight(stopped: Option<&str>) -> InFlight {
        InFlight {
            turn_id: "turn-1".to_string(),
            assistant_message_id: None,
            tools: HashMap::new(),
            stopped: stopped.map(str::to_string),
        }
    }

    // -- what one line turns out to mean --------------------------------------
    //
    // [`decide`] is 26% of this file's implementation and until ADR-0027 none of
    // it was assertable: it applied its own results, so reaching any of these
    // nine arms meant a live `Threads` and a real `claude` child. What follows is
    // the other half of the join `tests/protocol_golden.rs` checks the first half
    // of — that file pins `protocol` → [`Folded`] against captured NDJSON, and
    // these pin [`Folded`] → [`Change`].
    //
    // The lines are minimal rather than captured, and deliberately so: a whole
    // recording is what the golden files are for, and what is under test here is
    // which changes one line produces, in what order. The shapes are the ones
    // `crate::protocol`'s own tests use, which is where they were checked against
    // the real captures.

    /// A driver at the start of a session, with or without a turn under way.
    fn driver(turn: Option<InFlight>) -> Driving {
        Driving {
            turn,
            outstanding: HashMap::new(),
            interrupts: 0,
            measurements: 0,
            retunes: 0,
            pushed: HashMap::new(),
            unmeasured: false,
            drift_reported: Drift::default(),
            finished: None,
            reported_usage: None,
        }
    }

    /// The kinds of the activities a line produced, which is what a work log is
    /// a list of. Anything that is not an activity reads as the variant it is,
    /// because a test that silently skipped a `thread.message-sent` would be
    /// asserting about a shorter conversation than the one that happened.
    fn kinds(decided: &Decided) -> Vec<&str> {
        decided
            .changes
            .iter()
            .map(|change| match change {
                Change::Activity(activity) => activity.kind.as_str(),
                Change::AssistantDelta { .. } => "<delta>",
                Change::AssistantMessage { .. } => "<message>",
                Change::Session(_) => "<session>",
                _ => "<other>",
            })
            .collect()
    }

    /// The one activity a line produced, or a failure naming what it produced
    /// instead.
    fn only_activity(decided: &Decided) -> &Activity {
        match decided.changes.as_slice() {
            [Change::Activity(activity)] => activity,
            other => panic!("expected one activity, got {other:?}"),
        }
    }

    /// The id the agent announces is written down and **published to nobody**.
    /// It is not a `Change` and there is no event in the contract that describes
    /// one, which is the whole reason [`Decided`] has a field for it rather than
    /// being a `Vec<Change>` — see `docs/adr/0027`.
    #[test]
    fn the_agents_announcement_is_written_down_and_told_to_nobody() {
        let mut folding = SessionState::new();
        let mut driving = driver(Some(in_flight(None)));

        let decided = decide(
            &mut folding,
            &mut driving,
            r#"{"type":"system","subtype":"init","session_id":"s-1","model":"m","cwd":"/tmp","permissionMode":"default"}"#,
        );

        assert_eq!(decided.agent_session.as_deref(), Some("s-1"));
        assert!(decided.changes.is_empty(), "{:?}", kinds(&decided));
        assert!(decided.settles.is_none());
        // And the loop is told to go and ask how full the window is, which is the
        // one thing this arm does besides remembering the id.
        assert!(driving.unmeasured);
    }

    /// The meter's row is asked for once per reading rather than once per line.
    /// The CLI repeats its counts — the `result` at the end of a turn usually
    /// agrees with the last assistant message in it — and a row per repetition
    /// would be a row per message on the thread, each saying what the one before
    /// it said.
    #[test]
    fn the_context_window_row_goes_up_once_per_reading() {
        let mut folding = SessionState::new();
        let mut driving = driver(Some(in_flight(None)));
        let answer = r#"{"type":"control_response","response":{"subtype":"success","request_id":"context-1","response":{"totalTokens":26937,"maxTokens":200000,"isAutoCompactEnabled":true,"percentage":13,"categories":[],"gridRows":[]}}}"#;

        let first = decide(&mut folding, &mut driving, answer);
        assert_eq!(kinds(&first), vec!["context-window.updated"]);
        let row = only_activity(&first);
        assert_eq!(row.payload["usedTokens"], 26_937);
        assert_eq!(row.payload["maxTokens"], 200_000);
        assert_eq!(row.turn_id.as_deref(), Some("turn-1"));

        let again = decide(&mut folding, &mut driving, answer);
        assert!(again.changes.is_empty(), "{:?}", kinds(&again));
    }

    /// A permission request is a row *and* a copy held here, and the copy is the
    /// half that makes an answer possible: the client answers by naming an id,
    /// and what has to go back to the CLI is the whole request.
    #[test]
    fn a_permission_request_is_a_row_and_a_copy_kept_to_answer_with() {
        let mut folding = SessionState::new();
        let mut driving = driver(Some(in_flight(None)));

        let decided = decide(
            &mut folding,
            &mut driving,
            r#"{"type":"control_request","request_id":"req-1","request":{"subtype":"can_use_tool","tool_name":"Write","input":{"file_path":"note.txt"},"tool_use_id":"toolu_1"}}"#,
        );

        assert_eq!(kinds(&decided), vec!["approval.requested"]);
        assert_eq!(only_activity(&decided).turn_id.as_deref(), Some("turn-1"));
        assert!(
            driving.outstanding.contains_key("req-1"),
            "the request the answer names has to survive the line that raised it"
        );
    }

    /// The same wire shape, told apart by what this build can *render*. An
    /// `AskUserQuestion` the composer can draw as a question becomes one; see
    /// [`crate::worklog::questions`] for why anything else falls through to the
    /// approval row rather than to nothing.
    #[test]
    fn a_question_the_composer_can_draw_is_a_question_and_not_an_approval() {
        let mut folding = SessionState::new();
        let mut driving = driver(Some(in_flight(None)));

        let readable = decide(
            &mut folding,
            &mut driving,
            r#"{"type":"control_request","request_id":"req-1","request":{"subtype":"can_use_tool","tool_name":"AskUserQuestion","input":{"questions":[{"question":"Which?","header":"Pick","options":[{"label":"This","description":"the first"}]}]}}}"#,
        );
        assert_eq!(kinds(&readable), vec!["user-input.requested"]);

        // A question with no options is one the client would silently discard,
        // so it goes up as the approval it arrived as — the developer is left
        // with something to answer either way.
        let unreadable = decide(
            &mut folding,
            &mut driving,
            r#"{"type":"control_request","request_id":"req-2","request":{"subtype":"can_use_tool","tool_name":"AskUserQuestion","input":{"questions":[{"question":"Which?","options":[]}]}}}"#,
        );
        assert_eq!(kinds(&unreadable), vec!["approval.requested"]);
        assert!(driving.outstanding.contains_key("req-2"));
    }

    /// Compaction is a row and nothing else, which is the whole of the criterion:
    /// the transcript is this server's own copy and a compaction is a fact about
    /// what the *agent* can still see.
    #[test]
    fn a_compaction_is_a_row_and_touches_nothing_else() {
        let mut folding = SessionState::new();
        let mut driving = driver(Some(in_flight(None)));

        let decided = decide(
            &mut folding,
            &mut driving,
            r#"{"type":"system","subtype":"compact_boundary","session_id":"s","compact_metadata":{"trigger":"auto","pre_tokens":154000,"post_tokens":21000}}"#,
        );

        assert_eq!(kinds(&decided), vec!["session.compacted"]);
        let row = only_activity(&decided);
        assert_eq!(row.payload["trigger"], "auto");
        assert_eq!(row.payload["preTokens"], 154_000);
        assert_eq!(row.tone, "info", "a compaction is not a failure");
        assert!(decided.settles.is_none());
    }

    /// A refusal is a failure; being close to a limit is not yet one. The tone is
    /// the difference between a turn that is slow and a turn that is not going to
    /// happen, and it is the only thing the developer reads it by.
    #[test]
    fn a_refused_request_is_an_error_and_a_warning_is_not() {
        let mut folding = SessionState::new();
        let mut driving = driver(Some(in_flight(None)));

        let warned = decide(
            &mut folding,
            &mut driving,
            r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed_warning","rateLimitType":"five_hour","resetsAt":1764547200},"session_id":"s"}"#,
        );
        assert_eq!(kinds(&warned), vec!["session.rate-limited"]);
        assert_eq!(only_activity(&warned).tone, "info");

        let refused = decide(
            &mut folding,
            &mut driving,
            r#"{"type":"rate_limit_event","rate_limit_info":{"status":"rejected","resetsAt":1764547200},"session_id":"s"}"#,
        );
        assert_eq!(only_activity(&refused).tone, "error");
    }

    /// Deltas open a message and keep appending to the *same* one, because the
    /// client folds a delta into a message it already holds. A second id would be
    /// a second bubble halfway through a sentence.
    #[test]
    fn deltas_open_one_message_and_go_on_filling_it() {
        let mut folding = SessionState::new();
        let mut driving = driver(Some(in_flight(None)));
        let delta = |text: &str| {
            format!(
                r#"{{"type":"stream_event","event":{{"type":"content_block_delta","index":0,"delta":{{"type":"text_delta","text":"{text}"}}}}}}"#
            )
        };

        let first = decide(&mut folding, &mut driving, &delta("hel"));
        let second = decide(&mut folding, &mut driving, &delta("lo"));

        let (Some(Change::AssistantDelta { message_id: opened, text: hel, .. }), Some(Change::AssistantDelta { message_id: still, text: lo, .. })) =
            (first.changes.first(), second.changes.first())
        else {
            panic!("{:?} then {:?}", kinds(&first), kinds(&second));
        };
        assert_eq!((hel.as_str(), lo.as_str()), ("hel", "lo"));
        assert_eq!(opened, still, "a reply is one message, not one per token");
    }

    /// The buffered message is authoritative and closes the id the deltas opened,
    /// which is what settles a reply the client is drawing as still arriving.
    #[test]
    fn a_buffered_message_closes_the_id_the_deltas_opened() {
        let mut folding = SessionState::new();
        let mut driving = driver(Some(in_flight(None)));

        let streamed = decide(
            &mut folding,
            &mut driving,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hello"}}}"#,
        );
        let Some(Change::AssistantDelta { message_id: opened, .. }) = streamed.changes.first()
        else {
            panic!("{:?}", kinds(&streamed));
        };
        let opened = opened.clone();

        let buffered = decide(
            &mut folding,
            &mut driving,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"hello"}]}}"#,
        );

        let [Change::AssistantMessage { message_id, text, .. }] = buffered.changes.as_slice()
        else {
            panic!("{:?}", kinds(&buffered));
        };
        assert_eq!(*message_id, opened, "the same message, now settled");
        assert_eq!(text, "hello");
        assert!(
            driving
                .turn
                .as_ref()
                .is_some_and(|turn| turn.assistant_message_id.is_none()),
            "the id has to be given up, or the next message appends to this one"
        );
    }

    /// A message that is nothing but a tool call is not an empty chat bubble. The
    /// CLI emits one buffered message per content block, so a turn that uses a
    /// tool produces several and most carry no text at all.
    #[test]
    fn a_message_that_is_only_a_tool_call_is_not_an_empty_bubble() {
        let mut folding = SessionState::new();
        let mut driving = driver(Some(in_flight(None)));

        let decided = decide(
            &mut folding,
            &mut driving,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_1","name":"Read","input":{"file_path":"a.rs"}}]}}"#,
        );

        assert_eq!(kinds(&decided), vec!["tool.updated"]);
    }

    /// Everything a message carried besides its text, in the order it carried it
    /// — which is the order the developer saw it happen. The pairing is held on
    /// this side because a `tool_result` carries an id and nothing else: what the
    /// tool *was* only exists here.
    #[test]
    fn a_calls_two_halves_are_two_rows_in_the_order_they_happened() {
        let mut folding = SessionState::new();
        let mut driving = driver(Some(in_flight(None)));

        let called = decide(
            &mut folding,
            &mut driving,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"thinking","thinking":"weighing it up"},{"type":"tool_use","id":"toolu_1","name":"Read","input":{"file_path":"a.rs"}}]}}"#,
        );
        assert_eq!(kinds(&called), vec!["task.progress", "tool.updated"]);

        let returned = decide(
            &mut folding,
            &mut driving,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"fn main() {}"}]}}"#,
        );
        assert_eq!(kinds(&returned), vec!["tool.completed"]);
        // The result names an id and nothing else, so the tool's own name in this
        // row can only have come from the call held on this side. `Call::untracked`
        // is what an unpaired result renders as, and it says `Tool`.
        assert_eq!(
            only_activity(&returned).payload["data"]["toolName"], "Read",
            "the pair is joined here or it is not joined at all"
        );
        assert!(
            driving
                .turn
                .as_ref()
                .is_some_and(|turn| turn.tools.is_empty()),
            "a call answered is a call no longer held"
        );
    }

    /// An agent that will not stop says so, and the turn goes back to ending the
    /// way it was going to end. The flag has to come off rather than merely being
    /// ignored: a normal ending reported as one the developer asked for would be a
    /// work log claiming they did something they did not.
    #[test]
    fn an_agent_that_refuses_to_stop_says_so_and_the_turn_carries_on() {
        let mut folding = SessionState::new();
        let mut driving = driver(Some(in_flight(Some("interrupt-1"))));

        let decided = decide(
            &mut folding,
            &mut driving,
            r#"{"type":"control_response","response":{"subtype":"error","request_id":"interrupt-1","error":"No active turn"}}"#,
        );

        assert_eq!(kinds(&decided), vec!["turn.interrupt-failed"]);
        let row = only_activity(&decided);
        assert!(row.summary.ends_with("No active turn"), "{}", row.summary);
        assert!(
            driving.turn.as_ref().is_some_and(|turn| !turn.was_stopped()),
            "a stop that was refused is not a stop"
        );
    }

    /// An acknowledgement for some other request says nothing about the turn
    /// running now — a late one, for a turn that has already ended, would
    /// otherwise put a failure on the turn after it.
    #[test]
    fn an_acknowledgement_for_another_request_is_not_this_turns() {
        let mut folding = SessionState::new();
        let mut driving = driver(Some(in_flight(Some("interrupt-2"))));

        let decided = decide(
            &mut folding,
            &mut driving,
            r#"{"type":"control_response","response":{"subtype":"error","request_id":"interrupt-1","error":"No active turn"}}"#,
        );

        assert!(decided.changes.is_empty(), "{:?}", kinds(&decided));
        assert!(
            driving.turn.as_ref().is_some_and(InFlight::was_stopped),
            "the stop this turn is actually waiting on still stands"
        );
    }

    // -- a push the agent would not take ---------------------------------------
    //
    // Ticket 11. The push itself needs a live child and lives in
    // `tests/socket_thread_modes.rs`; what is decidable here is what the *answer*
    // means, which is the half that has to put the driver's capture back.

    /// A mode the CLI refused. Two things are owed and neither is optional: a
    /// sentence naming what was refused, and the correction that stops the
    /// session claiming a mode it is not running under.
    #[test]
    fn a_refused_mode_push_is_reported_and_the_capture_goes_back() {
        let mut folding = SessionState::new();
        let mut driving = driver(Some(in_flight(None)));
        driving.pushed.insert(
            "retune-1".to_string(),
            Pushed::Mode {
                previous: "full-access".to_string(),
                asked: "approval-required".to_string(),
            },
        );

        let decided = decide(
            &mut folding,
            &mut driving,
            r#"{"type":"control_response","response":{"subtype":"error","request_id":"retune-1","error":"Cannot set permission mode: must be one of acceptEdits, auto, bypassPermissions, default, dontAsk, plan"}}"#,
        );

        assert_eq!(kinds(&decided), vec!["session.retune-refused"]);
        let row = only_activity(&decided);
        assert!(
            row.summary.contains("approval-required") && row.summary.contains("full-access"),
            "the sentence has to name what was refused and what is still running: {}",
            row.summary
        );

        // The correction is the driver's to spend, and it takes the capture back
        // to what the child is really running under.
        let mut start = a_start();
        start.runtime_mode = "approval-required".to_string();
        decided.reverts.clone().expect("a correction").revert(&mut start);
        assert_eq!(start.runtime_mode, "full-access");
    }

    /// A model the CLI refused, which is the reachable half of the two: a runtime
    /// mode is checked against the contract's closed set before it gets here, and
    /// a model slug is whatever the picker offered.
    #[test]
    fn a_refused_model_push_names_the_model_the_agent_kept() {
        let mut folding = SessionState::new();
        let mut driving = driver(None);
        driving.pushed.insert(
            "retune-1".to_string(),
            Pushed::Model {
                previous: Some("haiku".to_string()),
                asked: "not-a-model".to_string(),
            },
        );

        let decided = decide(
            &mut folding,
            &mut driving,
            r#"{"type":"control_response","response":{"subtype":"error","request_id":"retune-1","error":"Model \"not-a-model\" is not a recognized model id. Run /model to see available models."}}"#,
        );

        // Decided with **no turn in flight**, which is the difference between
        // this answer and an interrupt's: a push is about the session, so its
        // refusal means the same thing between turns as during one.
        let row = only_activity(&decided);
        assert!(
            row.summary.contains("not-a-model") && row.summary.contains("haiku"),
            "{}",
            row.summary
        );

        let mut start = a_start();
        start.model = Some("not-a-model".to_string());
        decided.reverts.clone().expect("a correction").revert(&mut start);
        assert_eq!(start.model.as_deref(), Some("haiku"));
    }

    /// A push that landed says nothing, and that is the point: the capture moved
    /// when the line was written, so a success has no work in it. A row per
    /// accepted mode change would be a row on every turn a developer picked a
    /// mode for.
    #[test]
    fn an_accepted_push_publishes_nothing_and_leaves_the_capture_alone() {
        let mut folding = SessionState::new();
        let mut driving = driver(Some(in_flight(None)));
        driving.pushed.insert(
            "retune-1".to_string(),
            Pushed::Mode {
                previous: "full-access".to_string(),
                asked: "approval-required".to_string(),
            },
        );

        let decided = decide(
            &mut folding,
            &mut driving,
            r#"{"type":"control_response","response":{"subtype":"success","request_id":"retune-1","response":{"mode":"default"}}}"#,
        );

        assert!(decided.changes.is_empty(), "{:?}", kinds(&decided));
        assert_eq!(decided.reverts, None);
        assert!(
            driving.pushed.is_empty(),
            "an answered push is no longer outstanding"
        );
    }

    /// An interrupt's acknowledgement travels the same envelope, so the two must
    /// not be confused: a stop the agent refused is about the *turn* and has to
    /// reach the arm that un-marks it, rather than being read as a push nobody
    /// sent.
    #[test]
    fn an_interrupts_acknowledgement_is_not_taken_for_a_push() {
        let mut folding = SessionState::new();
        let mut driving = driver(Some(in_flight(Some("interrupt-1"))));
        driving.pushed.insert(
            "retune-1".to_string(),
            Pushed::Mode {
                previous: "full-access".to_string(),
                asked: "approval-required".to_string(),
            },
        );

        let decided = decide(
            &mut folding,
            &mut driving,
            r#"{"type":"control_response","response":{"subtype":"error","request_id":"interrupt-1","error":"No active turn"}}"#,
        );

        assert_eq!(kinds(&decided), vec!["turn.interrupt-failed"]);
        assert_eq!(decided.reverts, None);
        assert_eq!(driving.pushed.len(), 1, "the push is still outstanding");
    }

    /// The row and the ending, and the ending names the turn it is about. That
    /// name is the whole of ADR-0027's second field: [`spend`] publishes the
    /// session change only while the conversation is still describing this turn,
    /// because a developer who stopped the agent can send the next turn while
    /// this one is still winding down.
    #[test]
    fn a_completed_turn_is_a_row_and_an_ending_that_names_its_turn() {
        let mut folding = SessionState::new();
        let mut driving = driver(Some(in_flight(None)));

        let decided = decide(
            &mut folding,
            &mut driving,
            r#"{"type":"result","subtype":"success","duration_ms":2008,"total_cost_usd":0.5}"#,
        );

        assert_eq!(kinds(&decided), vec!["turn.completed"]);
        assert_eq!(only_activity(&decided).turn_id.as_deref(), Some("turn-1"));

        let settles = decided.settles.expect("a result ends a turn");
        assert_eq!(settles.turn_id.as_deref(), Some("turn-1"));
        assert!(matches!(settles.status, SessionStatus::Ready));
        assert_eq!(settles.last_error, None);

        // And the loop is left the two things it has to `await`: the checkpoint
        // this turn's tree owes, and the question about the window.
        assert!(driving.unmeasured);
        assert_eq!(
            driving.finished.map(|finished| finished.status),
            Some("ready")
        );
        assert!(driving.turn.is_none(), "the turn is over");
    }

    /// A turn the developer stopped ends as `interrupted` rather than as the
    /// failure the CLI reports it as, and it is checkpointed as nothing at all —
    /// there is no checkpoint status that means interrupted, and one that said
    /// otherwise would relabel the turn. ADR-0004 and `CONTEXT.md`, *Settling*.
    #[test]
    fn a_stopped_turn_ends_as_interrupted_and_leaves_no_checkpoint() {
        let mut folding = SessionState::new();
        let mut driving = driver(Some(in_flight(Some("interrupt-1"))));

        let decided = decide(
            &mut folding,
            &mut driving,
            r#"{"type":"result","subtype":"error_during_execution","is_error":true,"duration_ms":13660}"#,
        );

        let settles = decided.settles.expect("a result ends a turn");
        assert!(matches!(settles.status, SessionStatus::Interrupted));
        assert_eq!(
            settles.last_error, None,
            "a turn the developer stopped did not fail"
        );
        assert!(
            driving.finished.is_none(),
            "there is no checkpoint status that means interrupted"
        );
    }

    /// A `result` that arrives with no turn in flight still ends the session's
    /// idea of one, and the `None` it names is a value rather than a missing
    /// answer: the session's `activeTurnId` is `None` too, and the two matching is
    /// what makes the ending publishable.
    #[test]
    fn a_result_with_no_turn_behind_it_names_the_turn_it_is_about_anyway() {
        let mut folding = SessionState::new();
        let mut driving = driver(None);

        let decided = decide(
            &mut folding,
            &mut driving,
            r#"{"type":"result","subtype":"success","duration_ms":12}"#,
        );

        assert_eq!(kinds(&decided), vec!["turn.completed"]);
        assert_eq!(only_activity(&decided).turn_id, None);
        let settles = decided.settles.expect("a result ends a turn");
        assert_eq!(settles.turn_id, None);
        assert!(driving.finished.is_none(), "no turn, no tree to record");
    }

    /// Every line that reaches a turn asks for one first, and a line that arrives
    /// with none says nothing rather than inventing a turn to attribute it to.
    /// Three arms return early for this and each of them has to carry the
    /// context-window row out with it — see below.
    #[test]
    fn a_line_that_needs_a_turn_and_has_none_publishes_nothing() {
        let mut folding = SessionState::new();
        let mut driving = driver(None);

        for line in [
            r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hello"}}}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"hello"}]}}"#,
            r#"{"type":"control_response","response":{"subtype":"error","request_id":"interrupt-1","error":"No active turn"}}"#,
        ] {
            let decided = decide(&mut folding, &mut driving, line);
            assert!(decided.changes.is_empty(), "{line}: {:?}", kinds(&decided));
            assert!(decided.settles.is_none(), "{line}");
        }
    }

    /// The meter's row is decided *ahead of* the match and must survive the arms
    /// that give up early. It was applied before the early `return` when this
    /// function published its own results; returning a value made losing it
    /// possible for the first time, so it is pinned here.
    #[test]
    fn the_early_returns_still_carry_the_context_window_row_out() {
        let mut folding = SessionState::new();
        let mut driving = driver(None);

        // A reading arrives, and the same line is one of the three that needs a
        // turn and has none.
        let decided = decide(
            &mut folding,
            &mut driving,
            r#"{"type":"control_response","response":{"subtype":"success","request_id":"context-1","response":{"totalTokens":26937,"maxTokens":200000,"isAutoCompactEnabled":true}}}"#,
        );
        assert_eq!(kinds(&decided), vec!["context-window.updated"]);

        // And on a line that does give up early: a delta with no turn behind it,
        // arriving after a reading that has moved.
        let mut driving = driver(None);
        let mut folding = SessionState::new();
        folding.token_usage = Some(TokenUsage {
            used_tokens: 10,
            total_processed_tokens: None,
            max_tokens: Some(200_000),
            input_tokens: None,
            output_tokens: None,
            compacts_automatically: None,
        });
        let decided = decide(
            &mut folding,
            &mut driving,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hello"}}}"#,
        );
        assert_eq!(
            kinds(&decided),
            vec!["context-window.updated"],
            "the row is decided before the match and the arm gave up after it"
        );

        // And on the fourth, which ticket 11 added: the answer to a mode push
        // returns before the match, so it is the one early return in front of
        // the rest rather than inside them. A reading that arrived on the same
        // line still has to come out.
        let mut driving = driver(None);
        driving.pushed.insert(
            "retune-1".to_string(),
            Pushed::Mode {
                previous: "full-access".to_string(),
                asked: "approval-required".to_string(),
            },
        );
        let mut folding = SessionState::new();
        folding.token_usage = Some(TokenUsage {
            used_tokens: 10,
            total_processed_tokens: None,
            max_tokens: Some(200_000),
            input_tokens: None,
            output_tokens: None,
            compacts_automatically: None,
        });
        let decided = decide(
            &mut folding,
            &mut driving,
            r#"{"type":"control_response","response":{"subtype":"error","request_id":"retune-1","error":"Cannot set permission mode"}}"#,
        );
        assert_eq!(
            kinds(&decided),
            vec!["context-window.updated", "session.retune-refused"],
            "the reading the meter is waiting for was dropped by the arm that \
             reports a refused push"
        );
    }

    /// The meter's row goes *in front of* the row that ends the turn, so the last
    /// thing in a turn is the one a developer actually reads. Both are decided on
    /// the same line — the `result` carries counts — and the order they are
    /// decided in is the order they are applied in.
    #[test]
    fn the_row_that_ends_a_turn_stays_the_last_thing_in_it() {
        let mut folding = SessionState::new();
        let mut driving = driver(Some(in_flight(None)));

        let decided = decide(
            &mut folding,
            &mut driving,
            r#"{"type":"result","subtype":"success","duration_ms":12,"usage":{"input_tokens":11,"output_tokens":22},"modelUsage":{"claude-opus-5":{"contextWindow":200000}}}"#,
        );

        assert_eq!(
            kinds(&decided),
            vec!["context-window.updated", "turn.completed"]
        );
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
            Drift::default(),
        );

        assert_eq!(summary, "Turn completed in 2.0s · $0.0795 · end_turn");
    }

    /// A turn that failed says so first, because that is what the developer
    /// needs from the sentence before anything else in it — and then says what
    /// the agent said went wrong, because "Turn failed" alone is not something
    /// anybody can act on.
    #[test]
    fn a_failed_turn_says_so_first_and_then_says_why() {
        let mut failed = result(true, Some(400), Some(0.0), Some("error"));
        failed.last_result.as_mut().expect("a result").error =
            Some("upstream connect error".to_string());

        let summary = turn_summary(&failed, Ending::Failed, Drift::default());
        assert!(summary.starts_with("Turn failed"), "{summary}");
        assert!(summary.ends_with("· upstream connect error"), "{summary}");

        // And the same failure with nothing said about it still reads as a
        // sentence rather than as one with a dangling separator.
        assert_eq!(
            turn_summary(
                &result(true, Some(400), Some(0.0), Some("error")),
                Ending::Failed,
                Drift::default()
            ),
            "Turn failed in 400ms · $0.0000 · error"
        );
    }

    /// A turn the developer stopped is reported as an error by the CLI and
    /// carries the CLI's diagnostics with it. Quoting those would show the
    /// developer a failure for having pressed stop, so the agent's complaint is
    /// read only on a turn that actually failed.
    #[test]
    fn a_stopped_turn_does_not_quote_the_clis_complaint_about_it() {
        let mut aborted = result(true, Some(13_660), Some(0.0), None);
        aborted.last_result.as_mut().expect("a result").error =
            Some("[ede_diagnostic] result_type=user".to_string());

        let summary = turn_summary(&aborted, Ending::Stopped, Drift::default());
        assert_eq!(summary, "Turn stopped by the developer in 13.7s · $0.0000");
    }

    /// What a turn could not read, in the sentence a developer actually sees.
    /// The counters existed before ticket 15 and were only in a payload nothing
    /// renders, which is a warning system nobody is warned by.
    #[test]
    fn a_turn_that_drifted_says_so_in_the_sentence() {
        let finished = result(false, Some(2008), None, None);

        assert_eq!(
            turn_summary(
                &finished,
                Ending::Completed,
                Drift {
                    unknown_events: 1,
                    parse_errors: 0
                }
            ),
            "Turn completed in 2.0s · 1 unrecognised event"
        );
        assert_eq!(
            turn_summary(
                &finished,
                Ending::Completed,
                Drift {
                    unknown_events: 3,
                    parse_errors: 2
                }
            ),
            "Turn completed in 2.0s · 3 unrecognised events and 2 unreadable lines"
        );
        // And a clean turn says nothing, which is what stops the clause becoming
        // noise a developer learns to skip.
        assert_eq!(
            turn_summary(&finished, Ending::Completed, Drift::default()),
            "Turn completed in 2.0s"
        );
    }

    /// Compaction is reported as what it is: the agent's memory rewritten, not
    /// the conversation changed.
    #[test]
    fn compaction_names_its_trigger_and_what_it_cost() {
        assert_eq!(
            compaction_summary(&Compaction {
                trigger: Some("auto".to_string()),
                pre_tokens: Some(154_000),
                post_tokens: Some(21_000),
            }),
            "Context compacted automatically · 154,000 tokens → 21,000"
        );
        assert_eq!(
            compaction_summary(&Compaction {
                trigger: Some("manual".to_string()),
                ..Compaction::default()
            }),
            "Context compacted at the developer's request"
        );
        // A trigger this build has never heard of is still worth a row, and is
        // named rather than dropped.
        assert!(compaction_summary(&Compaction {
            trigger: Some("microcompact".to_string()),
            ..Compaction::default()
        })
        .contains("microcompact"));
        assert_eq!(
            compaction_summary(&Compaction::default()),
            "Context compacted"
        );
    }

    /// The reset time is the whole reason a rate-limit row is worth having:
    /// "you have run out" with no answer to "until when" is not something a
    /// developer can plan around.
    #[test]
    fn a_rate_limit_says_which_limit_and_when_it_lifts() {
        assert_eq!(
            rate_limit_summary(&RateLimit {
                status: "rejected".to_string(),
                limit: Some("five_hour".to_string()),
                resets_at: Some(1_700_000_000),
            }),
            "The agent's usage limit has been reached (five_hour) · resets at \
             2023-11-14T22:13:20.000Z"
        );
        assert_eq!(
            rate_limit_summary(&RateLimit {
                status: "allowed_warning".to_string(),
                limit: None,
                resets_at: None,
            }),
            "The agent is close to its usage limit"
        );
        // A standing this build has never seen is named rather than guessed at.
        // Describing it as either of the two above would be putting words in the
        // agent's mouth about the one thing it is telling us.
        assert_eq!(
            rate_limit_summary(&RateLimit {
                status: "allowed_overage".to_string(),
                limit: None,
                resets_at: None,
            }),
            "The agent reported its usage standing as 'allowed_overage'"
        );
    }

    /// The agent's own last words are what a developer needs when the process
    /// went away, because nothing this server could infer would say why.
    ///
    /// The drift is there too, and this is the only sentence it can be in: a
    /// turn that never ends emits no `turn.completed`, and a session talking in
    /// a dialect this build could not read is the likelier explanation of the
    /// two for why it stopped talking at all.
    #[test]
    fn a_dead_agent_quotes_what_it_said_on_the_way_out() {
        assert_eq!(
            died_mid_turn(None, Drift::default()),
            "The agent stopped before the turn finished."
        );
        assert_eq!(
            died_mid_turn(
                Some("FATAL ERROR: JavaScript heap out of memory"),
                Drift::default()
            ),
            "The agent stopped before the turn finished. The agent said: FATAL ERROR: \
             JavaScript heap out of memory"
        );
        assert_eq!(
            died_mid_turn(
                None,
                Drift {
                    unknown_events: 4,
                    parse_errors: 0
                }
            ),
            "The agent stopped before the turn finished. This session had 4 unrecognised \
             events."
        );
    }

    /// A turn the developer stopped says *that*, over the failure the CLI
    /// reports it as. The values are `11-interrupted-turn.ndjson`'s own: the
    /// recording's `result` is `"is_error": true` with no cost and no stop
    /// reason, so a server reading the wire alone would tell the developer their
    /// own decision had gone wrong — which is the reading `Ending::of` exists to
    /// prevent, and the line below is the whole of it.
    #[test]
    fn a_stopped_turn_says_it_was_stopped_rather_than_that_it_failed() {
        let interrupted = in_flight(Some("interrupt-1"));
        let aborted = result(true, Some(13_660), Some(0.0), None);
        let ending = Ending::of(Some(&interrupted), aborted.last_result.as_ref());
        assert_eq!(ending, Ending::Stopped);
        assert_eq!(ending.session_status(), SessionStatus::Interrupted);
        assert_eq!(ending.tone(), "info");
        assert!(!ending.failed(), "a turn the developer stopped is not an error");

        assert_eq!(
            turn_summary(&aborted, ending, Drift::default()),
            "Turn stopped by the developer in 13.7s · $0.0000"
        );

        // And with nothing reported at all, which is the shape a session that
        // died before its `result` would leave behind.
        assert_eq!(
            turn_summary(&SessionState::new(), Ending::Stopped, Drift::default()),
            "Turn stopped by the developer."
        );
    }

    /// The same turn *without* the flag reads as the failure the CLI called it.
    /// The pair is the point: one field decides which, and this is the other half
    /// of it.
    #[test]
    fn the_same_aborted_result_reads_as_a_failure_when_nobody_stopped_it() {
        let running = in_flight(None);
        let aborted = result(true, Some(13_660), Some(0.0), None);
        let ending = Ending::of(Some(&running), aborted.last_result.as_ref());

        assert_eq!(ending, Ending::Failed);
        assert_eq!(ending.session_status(), SessionStatus::Error);
        assert_eq!(ending.tone(), "error");
    }

    /// A turn stops once. A second stop — a second click, or a stop after a
    /// permission was cancelled — is not a second row in the work log, and the
    /// id kept is the one the agent will answer.
    #[test]
    fn a_turn_records_the_first_thing_that_stopped_it_and_not_the_second() {
        let mut turn = in_flight(None);

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
            turn_summary(&result(false, None, None, None), Ending::Completed, Drift::default()),
            "Turn completed"
        );
        assert_eq!(
            turn_summary(&SessionState::new(), Ending::Completed, Drift::default()),
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

}
