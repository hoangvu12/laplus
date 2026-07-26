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
//! | a buffered `assistant` message | `thread.message-sent` with `streaming: false` — the client replaces with it |
//! | `result` | an activity carrying the duration and the cost, and the session is ready again |
//!
//! The second and third rows *are* accumulate-and-reconcile. Nothing decides
//! between them here: [`crate::protocol::Folded`] says which of the two a line
//! was, and the rule that makes the buffered message authoritative lives in the
//! reducer the golden files check.
//!
//! Nothing in that table makes the session `running`, and that is deliberate:
//! `init` is printed once for the whole conversation, so the transition belongs
//! to the prompt being sent rather than to anything the agent says.

use serde_json::json;

use crate::agent::{permission_mode_for, Agent, Launch};
use crate::clock::now_iso;
use crate::config::ClaudeSettings;
use crate::process::Search;
use crate::protocol::{Folded, SessionState};
use crate::threads::{Activity, Change, Prompt, Session, Thread, Threads};

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
    let prompts = threads.attach(&start.thread_id, move |incoming| {
        tokio::spawn(drive(driving, starting, incoming))
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

/// The session: start an agent, feed it turns, publish what it says, reap it.
async fn drive(
    threads: Threads,
    start: Start,
    mut prompts: tokio::sync::mpsc::Receiver<Prompt>,
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
    let mut turn: Option<InFlight> = None;
    // A turn that arrived while another was still running. Held rather than
    // sent: sending it would orphan the turn in flight — that turn would never
    // settle, and the finished one's duration and cost would be attributed to
    // the wrong turn.
    let mut waiting: Option<Prompt> = None;
    // False once the prompt channel has closed. The agent is then told there
    // will be no more turns and the loop keeps draining what it still owes.
    let mut accepting = true;

    loop {
        // Whatever is waiting goes next, as soon as the turn before it is done.
        if accepting && turn.is_none() {
            if let Some(prompt) = waiting.take() {
                if let Err(error) = agent.send(&prompt.text).await {
                    eprintln!("lightcode: cannot send a turn to the agent: {error}");
                    break;
                }
                turn = Some(InFlight {
                    turn_id: prompt.turn_id.clone(),
                    assistant_message_id: None,
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
        let next = tokio::select! {
            line = agent.next_line() => Next::Line(line),
            prompt = prompts.recv(), if accepting && waiting.is_none() => Next::Prompt(prompt),
        };

        match next {
            Next::Line(Some(line)) => {
                publish(&threads, &start, &mut folding, &mut turn, &line);
            }
            // The agent stopped producing: it exited, or its output was
            // abandoned. Either way there is nothing more to publish.
            Next::Line(None) => break,
            Next::Prompt(Some(prompt)) => waiting = Some(prompt),
            Next::Prompt(None) => {
                accepting = false;
                agent.close_input();
            }
        }
    }

    agent.stop().await;

    // A turn still in flight when the agent went is a turn that will never
    // finish, and saying "stopped" would let it sit in the UI as running
    // forever. Which of the two it is decides how the client settles the turn.
    let unfinished = turn.is_some();
    threads.apply(
        &start.thread_id,
        Change::Session(Session {
            status: if unfinished { "error" } else { "stopped" },
            runtime_mode: start.runtime_mode.clone(),
            active_turn_id: None,
            last_error: unfinished
                .then(|| "The agent stopped before the turn finished.".to_string()),
            updated_at: now_iso(),
        }),
    );
    threads.detach(&start.thread_id);
}

/// Which of the two sources the loop heard from. A named value rather than
/// bodies inside `select!`, because sending to the agent needs the same mutable
/// borrow the line future is holding until the select is over.
enum Next {
    Line(Option<String>),
    Prompt(Option<Prompt>),
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
    /// tool calls, which ticket 12 brings — gives each its own id rather than
    /// appending them all into one.
    assistant_message_id: Option<String>,
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
    turn: &mut Option<InFlight>,
    line: &str,
) {
    match folding.fold_line(line) {
        Folded::Nothing => {}

        // The agent announcing itself. Once per process rather than per turn, so
        // this only ever appends the activity; the session's `running` comes
        // from the prompt being sent, in `drive`.
        Folded::Initialized => {
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
            // Only the assistant's. The CLI echoes the user's turn back under
            // `--replay-user-messages`, which this server does not ask for, and
            // publishing one would put the prompt in the transcript twice.
            if completed.role != "assistant" {
                return;
            }
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
