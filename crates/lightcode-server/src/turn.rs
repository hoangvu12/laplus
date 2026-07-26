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
//! and one row that goes the other way: a `thread.approval.respond` command
//! becomes a `control_response` on the agent's stdin, and an `approval.resolved`
//! activity that closes the panel.
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
use crate::threads::{Activity, Answered, Change, Prompt, Session, Thread, Threads};
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
    let prompts = threads.attach(&start.thread_id, move |incoming, decisions| {
        tokio::spawn(drive(driving, starting, incoming, decisions))
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
    mut decisions: tokio::sync::mpsc::Receiver<Answered>,
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
    };
    // A turn that arrived while another was still running. Held rather than
    // sent: sending it would orphan the turn in flight — that turn would never
    // settle, and the finished one's duration and cost would be attributed to
    // the wrong turn.
    let mut waiting: Option<Prompt> = None;
    // False once the prompt channel has closed. The agent is then told there
    // will be no more turns and the loop keeps draining what it still owes.
    let mut accepting = true;
    // False once the decision channel has closed, which is the same moment —
    // both ends live on the thread's `Live`. Tracked separately anyway, because
    // a closed channel yields `None` forever and a `select!` arm that kept
    // polling one would spin.
    let mut answering = true;

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
        // A decision is polled under no such condition, and that asymmetry is the
        // point: the agent has *stopped* until one arrives, so anything that
        // deferred it would be the deadlock this ticket is about.
        let next = tokio::select! {
            line = agent.next_line() => Next::Line(line),
            decision = decisions.recv(), if answering => Next::Answer(decision),
            prompt = prompts.recv(), if accepting && waiting.is_none() => Next::Prompt(prompt),
        };

        match next {
            Next::Line(Some(line)) => {
                publish(&threads, &start, &mut folding, &mut driving, &line);
            }
            // The agent stopped producing: it exited, or its output was
            // abandoned. Either way there is nothing more to publish.
            Next::Line(None) => break,
            Next::Answer(Some(answered)) => {
                answer(&threads, &start, &mut agent, &mut driving, answered).await;
            }
            Next::Answer(None) => answering = false,
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
    Answer(Option<Answered>),
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
}

impl Driving {
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

        Folded::Completed => {
            let finished = turn.take();
            let active = finished.as_ref().map(|turn| turn.turn_id.clone());
            let summary = folding.last_result.as_ref();
            let failed = summary.is_some_and(|result| result.is_error);

            let mut completed = Activity::info(
                "turn.completed",
                &turn_summary(folding),
                json!({
                    "durationMs": summary.and_then(|result| result.duration_ms),
                    "totalCostUsd": summary.and_then(|result| result.total_cost_usd),
                    "numTurns": summary.and_then(|result| result.num_turns),
                    "stopReason": summary.and_then(|result| result.stop_reason.clone()),
                    "isError": failed,
                    // The drift accounting for this session, next to the turn it
                    // accumulated over — so a CLI that moved shows up where a
                    // developer is already looking.
                    "unknownEvents": folding.unknown_events,
                    "parseErrors": folding.parse_errors,
                }),
                active,
            );
            if failed {
                completed.tone = "error";
            }
            threads.apply(&start.thread_id, Change::Activity(completed));

            // Leaving `running` is what ends the turn for the client, so this
            // is the event that settles it — and the reason a turn's reported
            // duration covers the whole turn rather than stopping at the last
            // thing the assistant said.
            threads.apply(
                &start.thread_id,
                Change::Session(Session {
                    status: if failed { "error" } else { "ready" },
                    runtime_mode: start.runtime_mode.clone(),
                    active_turn_id: None,
                    last_error: failed.then(|| turn_summary(folding)),
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
fn turn_summary(state: &SessionState) -> String {
    let Some(result) = &state.last_result else {
        return "Turn completed.".to_string();
    };

    let mut summary = match result.is_error {
        true => "Turn failed".to_string(),
        false => "Turn completed".to_string(),
    };
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
        let summary = turn_summary(&result(
            false,
            Some(2008),
            Some(0.079_471_999_999_999_99),
            Some("end_turn"),
        ));

        assert_eq!(summary, "Turn completed in 2.0s · $0.0795 · end_turn");
    }

    /// A turn that failed says so first, because that is what the developer
    /// needs from the sentence before anything else in it.
    #[test]
    fn a_failed_turn_says_so_before_it_says_anything_else() {
        let summary = turn_summary(&result(true, Some(400), Some(0.0), Some("error")));
        assert!(summary.starts_with("Turn failed"), "{summary}");
    }

    /// A CLI that reported neither still produces a sentence. The fields are
    /// optional in the protocol, and a half-built string with a dangling
    /// separator would be worse than a short one.
    #[test]
    fn a_turn_with_nothing_to_report_still_says_it_finished() {
        assert_eq!(turn_summary(&result(false, None, None, None)), "Turn completed");
        assert_eq!(turn_summary(&SessionState::new()), "Turn completed.");
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
