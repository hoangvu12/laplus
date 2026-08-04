//! The `claude` driver: the CLI's NDJSON on one side, the thread's events on the
//! other.
//!
//! This is the join between the two protocols the crate keeps apart. Neither
//! side is implemented here — [`crate::protocol`] parses and folds what the
//! agent says, [`crate::threads`] holds what the UI reads, and [`crate::agent`]
//! owns the process — so what is left is the translation, and that is
//! deliberately all this file is. The *lifetime* around it is
//! [`crate::session`]'s, and is written once for every driver there will ever
//! be: this file is one implementation of [`Driver`] and currently the only one.
//!
//! ## The translation
//!
//! | The agent says | The thread publishes |
//! |---|---|
//! | `system`/`init` | nothing — the id is written down and the window is measured |
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
//! [`crate::session::spend`] is what publishes. The split is `docs/adr/0027` and
//! it is ADR-0025's, taken again one level down: a function that applies its own
//! results can be tested only by watching what it did to a live world, and this
//! one's world is a [`crate::threads::Threads`] with a running `claude` behind
//! it. Nothing about
//! the wire changed — the same events, in the same order, under the same numbers
//! — and the tests at the bottom of this file are what the change was for.
//!
//! It is also what makes this file a driver rather than a loop. [`Decided`] is
//! the shared vocabulary a session is answered in, so everything below reads the
//! `claude` CLI and nothing below knows what a checkpoint is.
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
//! ## What this CLI needs that the trait's verbs are the shape of
//!
//! Every write in [`Agent`] is one line of JSON on the child's stdin, and four of
//! them carry an id the CLI answers on stdout. That is why a request id travels
//! into [`Driver::interrupt`], [`Driver::measure`] and [`Driver::retune`] and
//! comes back out through [`decide`]: the answer is the only thing that says
//! whether a push landed, and matching it is this driver's problem rather than
//! the session's.
//!
//! - **An interrupt** is a `control_request` on stdin — the same envelope a
//!   permission arrives in, going the other way — and it leaves the child
//!   running. `fixtures/claude-cli/12-interrupt-then-continue.ndjson` is the
//!   recording that settles it.
//! - **The CLI reports a stopped turn as a failed one.** Its `result` carries
//!   `"is_error": true` and the subtype `error_during_execution`, and nothing in
//!   the output distinguishes "the developer pressed stop" from "the turn went
//!   wrong" — so [`InFlight::stopped`] is the only thing that can, and it is why
//!   the flag exists rather than the ending being read off the wire.
//! - **The partial reply is kept, and the CLI hands it over whole.** After the
//!   acknowledgement comes a buffered `assistant` message carrying exactly what
//!   had streamed, so "output produced before the interrupt is retained" needs
//!   nothing special here: it is the ordinary reconcile, on a shorter message.
//!   A second driver need not have that property, and Codex does not.
//! - **`--permission-mode` and `--model` are read once, at launch**, so a
//!   developer who changed either mid-conversation was answered by a process
//!   still running under the old one until [`Driver::retune`] pushed it. Ticket
//!   11 of `thread-lifecycle`, and
//!   `fixtures/claude-cli/20-modes-changed-mid-conversation.ndjson` is a real
//!   `claude` being moved between two turns.
//!
//! An unanswered permission costs a tool call and nothing else: closing the
//! agent's stdin closes the permission stream with it, the CLI abandons the
//! request, and the turn finishes.
//! `fixtures/claude-cli/09-permission-unanswered.ndjson` is a recording of
//! exactly that, and [`crate::agent`] documents the mechanism.
//!
//! ## Continuity is the agent's, and this is where the handle on it is kept
//!
//! Within one process there is nothing to do: the child is long-lived and the
//! conversation is its own, so a follow-up is a second line on the same stdin.
//! Across a restart the driver reads its provider resume cursor and turns the
//! session id inside it into one flag — `--resume <session-id>` — because the
//! context lives in the agent's own store rather than in this server's
//! transcript. Legacy rows whose only continuation is the raw session-id column
//! are the v0 form and are migrated when the resumed CLI announces itself.
//! Replaying the transcript into each prompt would be the alternative, and it
//! would be a second, worse copy of the conversation that the agent had no reason
//! to believe.
//!
//! So the `init` line's session id is written as the driver's versioned cursor
//! through the shared continuation boundary, and a session opened for a thread
//! that has one is opened with `--resume`. The id is the agent's own account of
//! itself rather than something this server minted, for the same reason the model
//! and the permission mode are.
//!
//! A resume the CLI will not honour is the one failure with no NDJSON to it at
//! all: the child writes its reason to stderr and exits. [`resume_refused`] is
//! how that becomes a sentence in the conversation — carried out of
//! [`Driver::stop`], because the CLI's own words are only final once the child
//! has gone — and the stored id is deliberately *kept*: starting a fresh session
//! under a thread whose history the agent has forgotten would leave the developer
//! talking to something that cannot see the transcript in front of them.

use serde_json::json;

use crate::agent::{permission_mode_for, Agent, Launch};
use crate::approval::ApprovalRequest;
use crate::clock::iso_from_epoch;
use crate::process::Search;
use crate::protocol::{Compaction, ContentBlock, Drift, Folded, RateLimit, SessionState, TokenUsage};
use crate::session::{
    Decided, Driver, Driving, Finished, InFlight, Opened, Pushed, Reaped, Reply, Settles, Start,
};
use crate::settling::SessionStatus;
use crate::threads::{Activity, Change};
use crate::worklog::{Call, Returned};

/// Named by the tests at the bottom of this file and nowhere else in it: they
/// build an [`InFlight`] and a [`Start`] by hand, where the driver reaches the
/// first through a field and reads the second's settings without naming their
/// type.
#[cfg(test)]
use {crate::config::ClaudeSettings, std::collections::HashMap};

/// A `claude` CLI behind a session: the child, what it has said so far, and the
/// conversation it was asked to continue.
///
/// The three are one type because the ending needs all three at once — see
/// [`Driver::stop`], where "the agent never announced itself and it was asked to
/// resume" is the whole of how a refused resume is recognised.
#[derive(Debug)]
pub(crate) struct Claude {
    agent: Agent,
    /// Everything the CLI has said, folded. The accumulated state behind
    /// [`Folded`]'s two index-carrying variants, and per-driver by construction:
    /// a second driver folds its own protocol into its own.
    folding: SessionState,
    /// The conversation this child was asked to continue, kept for the one
    /// question only the ending can ask of it.
    ///
    /// Captured when the child was opened rather than read off [`Start`] at the
    /// end, and the two are the same value: a retune moves the mode and the
    /// model on that capture and never the conversation, because there is no
    /// such request and a session that changed which conversation it was in the
    /// middle of would not be one.
    resume: Option<String>,
}

fn resume_session(start: &Start) -> Result<Option<String>, String> {
    let Some(cursor) = &start.resume_cursor else {
        return Ok(None);
    };
    if let Some(session_id) = cursor.value.as_str().filter(|id| !id.is_empty()) {
        return Ok(Some(session_id.to_string()));
    }
    if cursor.value.as_object().map(serde_json::Map::len) != Some(2) {
        return Err("The stored Claude continuation is incompatible with this build.".to_string());
    }
    let version = cursor.value.get("version").and_then(serde_json::Value::as_u64);
    let session_id = cursor.value.get("sessionId").and_then(serde_json::Value::as_str);
    match (version, session_id) {
        (Some(1), Some(session_id)) if !session_id.is_empty() => Ok(Some(session_id.to_string())),
        (Some(version), _) if version > 1 => Err(format!(
            "Claude continuation version {version} is newer than this build supports."
        )),
        _ => Err("The stored Claude continuation is incompatible with this build.".to_string()),
    }
}

fn resume_cursor(provider: &crate::provider::ProviderIdentity, session_id: &str) -> crate::provider::ResumeCursor {
    crate::provider::ResumeCursor {
        provider: provider.clone(),
        value: json!({"version": 1, "sessionId": session_id}),
    }
}

impl Driver for Claude {
    /// Resolve the binary and start the agent, or say why not.
    async fn open(start: &Start) -> Result<Opened<Claude>, String> {
        let resume = resume_session(start)?;
        // Resolved here rather than on the dispatch path: it is a walk of every
        // `PATH` directory, and the read loop is answering a developer who has
        // just pressed enter. Resolved per session rather than once at boot
        // because the setting can change and an install can move, and this is the
        // moment the answer actually matters.
        let settings = start.driver.claude()?;
        let (path, _) =
            crate::provider::resolve(&settings.binary_path, &Search::from_environment())
                .startable()?;

        let agent = Agent::start(&Launch {
            binary: path.clone(),
            cwd: start.workspace_root.clone(),
            model: start.model.clone(),
            permission_mode: permission_mode_for(&start.runtime_mode),
            resume: resume.clone(),
        })
        .await
        .map_err(|error| {
            format!(
                "The Claude Code binary {} could not be started in {}: {error}",
                path.display(),
                start.workspace_root
            )
        })?;

        Ok(Opened {
            driver: Claude {
                agent,
                folding: SessionState::new(),
                resume,
            },
            decided: Decided::default(),
        })
    }

    /// Take the next line and say what it turned out to mean.
    ///
    /// **Cancel-safe, and it has to be**: this is one arm of the session's
    /// `select!`, so it is dropped unfinished whenever a prompt or a signal wins
    /// the race. The only `await` is the channel receive — [`decide`] is
    /// synchronous — so a drop can never land between a line being taken off the
    /// channel and being folded. Every write this driver makes is one of the
    /// other verbs, which the loop awaits on their own.
    async fn next(&mut self, driving: &mut Driving) -> Option<Decided> {
        let line = self.agent.next_line().await?;
        Some(decide(&mut self.folding, driving, &line))
    }

    async fn send(&mut self, prompt: &crate::threads::Prompt) -> std::io::Result<()> {
        // A new prompt means the next `result` is this turn's rather than a
        // duplicate of the last one's. See `SessionState::completion_reported`.
        self.folding.completion_reported = false;
        self.agent.send(&prompt.text).await
    }

    async fn interrupt(&mut self, request_id: &str) -> std::io::Result<()> {
        self.agent.interrupt(request_id).await
    }

    async fn answer(&mut self, asked: &ApprovalRequest, reply: Reply<'_>) -> std::io::Result<()> {
        // The contract's vocabulary on the way in, the CLI's on the way out.
        // Which is the encoder, and it is per-driver for ADR-0001's reason: a
        // `cancel` is a denial carrying `interrupt: true` *here*, and something
        // else wherever the second driver is.
        let answer = match reply {
            Reply::Decided(decision) => decision.answer(asked),
            Reply::Answers(answers) => crate::worklog::answers_for(asked, answers),
            Reply::Rejected => crate::worklog::Decision::Decline.answer(asked),
        };
        self.agent.answer(&asked.request_id, &answer).await
    }

    async fn measure(&mut self, request_id: &str) -> std::io::Result<()> {
        self.agent.measure_context(request_id).await
    }

    async fn retune(&mut self, request_id: &str, asked: &Pushed) -> std::io::Result<()> {
        match asked {
            Pushed::Mode { asked, .. } => {
                let mode = crate::agent::pushed_permission_mode_for(asked);
                self.agent.set_permission_mode(request_id, mode).await
            }
            Pushed::Model { asked, .. } => self.agent.set_model(request_id, asked).await,
        }
    }

    fn close_input(&mut self) {
        self.agent.close_input();
    }

    /// Reap the child, then read the two sentences only it can say.
    ///
    /// Reaped first, and the complaint comes back from the reaping. A session
    /// that was asked to continue an old conversation and never announced itself
    /// did not continue it: the CLI writes its reason to stderr and exits without
    /// a line of NDJSON, so the agent's own words are the only account of it —
    /// and they are only final once the child has gone. See [`Agent::stop`].
    async fn stop(self, driving: &mut Driving, asked_to_stop: bool) -> Reaped {
        let folding = self.folding;
        let complaint = self.agent.stop().await;

        let refused = self
            .resume
            .as_ref()
            .filter(|_| folding.session_id.is_none())
            .map(|session| resume_refused(session, complaint.as_deref()));

        // A turn still in flight when the agent went is a turn that will never
        // finish, and this is the only moment anybody can say so.
        //
        // The drift goes here too, and this is the only place it can: a turn that
        // never ends emits no `turn.completed`, so a session that died having also
        // been talking in a dialect this build could not read would otherwise
        // report the death and nothing about the dialect — which is the more
        // likely explanation of the two.
        //
        // **Unless the developer ended the session**, which is the one way a turn
        // is cut short that nobody needs telling about: they asked for the process
        // to go and it went. A stop mid-turn therefore takes that turn's
        // unreported drift with it, and that is the accepted cost rather than an
        // oversight — the tally is reported on every turn that ends on its own,
        // and inventing a row to carry it out of a session the developer ended
        // would put a diagnostic about this build in front of somebody who had
        // just pressed stop.
        let unread = driving.drift_to_report(folding.drift());
        let death = (driving.turn.is_some() && !asked_to_stop)
            .then(|| died_mid_turn(complaint.as_deref(), unread));

        Reaped { refused, death }
    }
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

/// How full the context window is, as the row the composer's meter reads.
///
/// The client never renders this row in the work log — `session-logic.ts` skips
/// the kind in `deriveWorkLogEntries` — it walks the activity list backwards for
/// the newest one and draws the meter from it
/// (`apps/web/src/lib/contextWindow.ts`, `deriveLatestContextWindowSnapshot`).
/// So the row is a carrier rather than something a developer reads, and its
/// summary exists only because the contract requires a non-empty one.
pub(crate) fn context_window_row(usage: &TokenUsage, turn_id: Option<String>) -> Activity {
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

/// Fold one line and say what it turned out to be.
///
/// No [`crate::threads::Threads`], no lock, no clock and no child process: a
/// [`SessionState`], a [`Driving`] and a line in, a [`Decided`] out. That is the whole of what this
/// commit bought and the whole of why the tests at the bottom of this file can
/// exist — see [`Decided`] and `docs/adr/0027`.
///
/// It is **not** pure, and the difference is worth being exact about rather than
/// claiming otherwise in a doc comment. It advances the reducer, it mutates the
/// turn in flight, and the five [`Activity`] constructors it calls read a clock
/// and mint identifiers exactly as ADR-0025 left them. What changed is that it
/// returns its results instead of applying them.
/// A turn for something the agent said when nothing had asked it to speak.
///
/// **The background-subagent case, and it is the developer's whole experience of
/// one.** A subagent launched with `run_in_background` outlives the turn that
/// spawned it: that turn settles, the session goes idle, and some seconds later
/// the CLI hands the agent a `task_notification` and the agent replies with what
/// the subagent found. Until this existed, both arms that publish assistant text
/// returned early on a session with no turn in flight, so that reply — the entire
/// answer — was discarded. The developer saw the work log tick along and then
/// nothing, and the only way to recover the answer was to ask a question that
/// opened a turn by hand.
///
/// Opening one is a smaller change than it sounds: the turn is real by every test
/// the rest of this file applies to a turn. The agent is working, the work has an
/// id, its text belongs to that id, and the trailing `result` ends it through the
/// ordinary [`Folded::Completed`] arm. What is unusual is only who started it, and
/// the client needs no new vocabulary for that — `thread.session-set` naming an
/// active turn is enough for its reducer to draw one.
///
/// Two facts have to move together, which is why this is a function rather than a
/// literal in two arms:
///
/// - the id has to reach the loop, because announcing a turn needs a [`Start`]
///   this module does not have. It travels in [`Decided::opens`].
/// - `completion_reported` has to come off, or the duplicate-`result` guard
///   swallows the very `result` that would end this turn — the previous turn's
///   ending was reported, and without this the flag still says so. That would
///   leave the session `running` on a turn nothing can settle, which is the
///   spinner-forever failure and worse than the one being fixed.
fn unprompted(folding: &mut SessionState, decided: &mut Decided) -> InFlight {
    let turn_id = crate::threads::fresh_turn_id();
    folding.completion_reported = false;
    decided.opens = Some(turn_id.clone());
    InFlight {
        turn_id,
        assistant_message_id: None,
        tools: std::collections::HashMap::new(),
        stopped: None,
    }
}

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
    let moved = driving.usage_to_report(folding.token_usage.clone());
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
            decided.provider_resume_cursor = folding
                .session_id
                .as_deref()
                .map(|session_id| resume_cursor(&driving.provider, session_id));
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
        // A subagent moved. One row per subagent, kept up to date — the work the
        // developer could not see at all before, because every `task_*` event
        // reached `SystemEvent::Other` and was dropped in silence.
        Folded::SubagentProgress(task) => {
            let turn_id = turn.as_ref().map(|turn| turn.turn_id.clone());
            decided
                .changes
                .push(Change::Activity(crate::worklog::subagent(&task, turn_id)));
        }

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
            let active = match turn {
                Some(active) => active,
                // Nobody asked for this, so a turn is opened to hold it rather
                // than the words being dropped — see [`unprompted`].
                None => turn.insert(unprompted(folding, &mut decided)),
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
            let active = match turn {
                Some(active) => active,
                // [`Folded::Streamed`]'s reason, and reached when a message
                // buffers without having streamed — the deltas are a flag this
                // server passes rather than something the CLI owes it, so the
                // between-turns message this catches is the same message.
                None => turn.insert(unprompted(folding, &mut decided)),
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

        // A *second* terminal `result` for a turn already reported ended. One
        // invocation emits two when a background subagent ran — see
        // `fixtures/claude-cli/22-background-subagent.ndjson`, whose recording
        // ends `result, result` — and the second arrived with the turn already
        // taken, reporting a completion for nothing.
        //
        // Both halves of the condition are load-bearing. A `result` with no turn
        // in flight is *not* on its own a duplicate: it still settles the
        // session's idea of one, and the `None` it names is a value that matches
        // the session's own `activeTurnId` — which is what
        // `a_result_with_no_turn_behind_it_names_the_turn_it_is_about_anyway`
        // pins. What makes this one a duplicate is that a completion has already
        // been reported since the last prompt went out.
        Folded::Completed if driving.turn.is_none() && folding.completion_reported => {
            folding.bump("result/duplicate");
        }

        Folded::Completed => {
            folding.completion_reported = true;
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
            let drift = driving.drift_to_report(folding.drift());

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
            // finished until the agent got round to it.
            // [`crate::session::spend`] asks.
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
            resume_cursor: None,
            provider: crate::provider::registration(crate::provider::CLAUDE_DRIVER)
                .expect("registered")
                .identity(crate::provider::CLAUDE_INSTANCE_ID),
            driver: crate::session::DriverStart::Claude(ClaudeSettings {
                enabled: true,
                binary_path: "claude".to_string(),
                home_path: String::new(),
                launch_args: String::new(),
                custom_models: Vec::new(),
            }),
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
            provider: crate::provider::registration(crate::provider::CLAUDE_DRIVER)
                .expect("registered")
                .identity(crate::provider::CLAUDE_INSTANCE_ID),
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

        assert_eq!(
            decided.provider_resume_cursor.as_ref().map(|cursor| &cursor.value),
            Some(&json!({"version": 1, "sessionId": "s-1"}))
        );
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

    /// One invocation emits two `result` lines when a background subagent ran —
    /// `fixtures/claude-cli/22-background-subagent.ndjson` ends `result, result`.
    /// The first ends the turn; the second would report a second ending for a turn
    /// already over, which is a row saying something untrue.
    #[test]
    fn a_second_result_does_not_end_the_turn_twice() {
        let mut folding = SessionState::new();
        let mut driving = driver(Some(in_flight(None)));

        let first = decide(
            &mut folding,
            &mut driving,
            r#"{"type":"result","subtype":"success","duration_ms":5674,"num_turns":2}"#,
        );
        assert_eq!(kinds(&first), vec!["turn.completed"]);
        assert_eq!(
            first.settles.expect("the first result ends the turn").turn_id,
            Some("turn-1".to_string())
        );

        let second = decide(
            &mut folding,
            &mut driving,
            r#"{"type":"result","subtype":"success","duration_ms":2082,"num_turns":1}"#,
        );
        assert!(kinds(&second).is_empty(), "{:?}", kinds(&second));
        assert!(second.settles.is_none(), "the turn was already settled");

        // And the next prompt clears it, so that turn's own result still lands.
        folding.completion_reported = false;
        driving.turn = Some(in_flight(None));
        let next = decide(
            &mut folding,
            &mut driving,
            r#"{"type":"result","subtype":"success","duration_ms":7}"#,
        );
        assert_eq!(kinds(&next), vec!["turn.completed"]);
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

    /// An acknowledgement is the one line left that needs a turn and says nothing
    /// without one, and it is the right answer for that line: it answers an
    /// interrupt *this server* sent, so one arriving with no turn behind it is
    /// about a turn that has already ended.
    ///
    /// **Assistant text used to be in this list.** Both arms that publish it gave
    /// up the same way, which is what made a background subagent's report vanish —
    /// see [`unprompted`] and the test below.
    #[test]
    fn an_acknowledgement_with_no_turn_behind_it_publishes_nothing() {
        let mut folding = SessionState::new();
        let mut driving = driver(None);

        let decided = decide(
            &mut folding,
            &mut driving,
            r#"{"type":"control_response","response":{"subtype":"error","request_id":"interrupt-1","error":"No active turn"}}"#,
        );
        assert!(decided.changes.is_empty(), "{:?}", kinds(&decided));
        assert!(decided.settles.is_none());
        assert!(decided.opens.is_none(), "nothing was said, so nothing began");
    }

    /// A background subagent outlives the turn that spawned it: that turn settles,
    /// and some seconds later the CLI hands the agent a `task_notification` and the
    /// agent answers into a session with no turn in flight. That answer was being
    /// dropped, so the only way to learn what a subagent found was to ask again —
    /// which opened a turn by hand and let the CLI say it a second time.
    ///
    /// The turn it opens has to end, too. The assertion on the trailing `result`
    /// is the one that matters most here: the duplicate-`result` guard reads a
    /// flag set by the *previous* turn's ending, and a turn opened without
    /// clearing it would be a session left running with nothing able to settle it.
    #[test]
    fn a_report_arriving_after_the_turn_settled_opens_one_to_land_in() {
        let mut folding = SessionState::new();
        let mut driving = driver(Some(in_flight(None)));

        let ended = decide(
            &mut folding,
            &mut driving,
            r#"{"type":"result","subtype":"success","duration_ms":5674,"num_turns":2}"#,
        );
        assert_eq!(kinds(&ended), vec!["turn.completed"]);
        assert!(driving.turn.is_none(), "the turn that spawned it is over");

        let spoken = decide(
            &mut folding,
            &mut driving,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"The subagent finished"}}}"#,
        );
        assert_eq!(kinds(&spoken), vec!["<delta>"]);
        let opened = spoken.opens.clone().expect("the text opened a turn");
        assert_ne!(
            opened, "turn-1",
            "a turn of its own, not the settled one reopened"
        );
        assert_eq!(
            driving.turn.as_ref().map(|turn| turn.turn_id.clone()),
            Some(opened.clone()),
            "and the driver is now working on it"
        );
        match spoken.changes.as_slice() {
            [Change::AssistantDelta { turn_id, text, .. }] => {
                assert_eq!(turn_id, &opened, "the words belong to the turn they opened");
                assert_eq!(text, "The subagent finished");
            }
            other => panic!("expected one delta, got {other:?}"),
        }

        let settled = decide(
            &mut folding,
            &mut driving,
            r#"{"type":"result","subtype":"success","duration_ms":28963,"num_turns":1}"#,
        );
        assert_eq!(kinds(&settled), vec!["turn.completed"]);
        assert_eq!(
            settled
                .settles
                .expect("the opened turn ends like any other")
                .turn_id,
            Some(opened)
        );
    }

    /// The buffered half of the same story, reached when a message arrives without
    /// having streamed — the partial-message flag is one this server passes rather
    /// than something the CLI owes it.
    #[test]
    fn a_buffered_message_after_the_turn_settled_opens_one_too() {
        let mut folding = SessionState::new();
        let mut driving = driver(None);

        let decided = decide(
            &mut folding,
            &mut driving,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"the subagent found three"}]}}"#,
        );

        let opened = decided.opens.clone().expect("the message opened a turn");
        match decided.changes.as_slice() {
            [Change::AssistantMessage { turn_id, text, .. }] => {
                assert_eq!(turn_id, &opened);
                assert_eq!(text, "the subagent found three");
            }
            other => panic!("expected one message, got {other:?}"),
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

        // And on a line that does give up early: an acknowledgement with no turn
        // behind it, arriving after a reading that has moved. (A delta stood here
        // until assistant text stopped giving up — see [`unprompted`].)
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
            r#"{"type":"control_response","response":{"subtype":"error","request_id":"interrupt-1","error":"No active turn"}}"#,
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
