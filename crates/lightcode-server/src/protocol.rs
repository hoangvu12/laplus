//! The `claude` CLI stdio wire format, as a pure module.
//!
//! This module parses one NDJSON line into an [`Event`] and folds events into a
//! [`SessionState`]. It is pure: no I/O, no printing, no process handling. The
//! agent driver that spawns `claude` and pumps its stdio owns all of that and
//! feeds lines in here.
//!
//! Isolating the wire format is the mitigation for the CLI-drift risk: when the
//! format shifts, the blast radius is this file, and `tests/protocol_golden.rs`
//! tells you it shifted before any server code notices.
//!
//! Lifted from `spike-claude-protocol/src/protocol.rs`, whose README records the
//! evidence behind the shapes below.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Wire format
// ---------------------------------------------------------------------------

/// One line of the CLI's `--output-format stream-json` output.
///
/// `Unknown` is load-bearing, not defensive padding: the CLI's envelope is not
/// a stability-guaranteed contract, so an unrecognized `type` must degrade to a
/// counted event rather than killing the session. The reducer surfaces those
/// counts so protocol drift shows up as a number instead of a crash.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// Session lifecycle: `init`, `status`, `hook_started`, `hook_response`.
    System(SystemEvent),
    /// A complete assistant turn. `message` is a standard Messages API object.
    Assistant(MessageEnvelope),
    /// Echoed user turn (with `--replay-user-messages`).
    User(MessageEnvelope),
    /// A verbatim Messages API SSE event, for token-level rendering.
    StreamEvent { event: StreamEvent },
    RateLimitEvent(serde_json::Value),
    /// Terminal event for the turn.
    Result(ResultEvent),
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "subtype", rename_all = "snake_case")]
pub enum SystemEvent {
    Init(Box<InitEvent>),
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
pub struct InitEvent {
    pub session_id: String,
    pub model: String,
    pub cwd: String,
    #[serde(rename = "permissionMode")]
    pub permission_mode: String,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub slash_commands: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct MessageEnvelope {
    pub message: Message,
}

/// The inner payload is the standard Anthropic Messages API `Message`.
/// This is the spike's most useful finding: the parts most likely to carry
/// real complexity are a documented, versioned schema, not a bespoke one.
#[derive(Debug, Deserialize)]
pub struct Message {
    pub role: String,
    #[serde(default)]
    pub content: Vec<ContentBlock>,
    #[serde(default)]
    pub stop_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text { text: String },
    Thinking {},
    ToolUse { name: String },
    ToolResult {},
    #[serde(other)]
    Unknown,
}

/// Verbatim Messages API streaming events, delivered under `--include-partial-messages`.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    MessageStart {},
    ContentBlockStart { index: usize },
    ContentBlockDelta { index: usize, delta: Delta },
    ContentBlockStop { index: usize },
    MessageDelta {},
    MessageStop,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Delta {
    TextDelta { text: String },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Deserialize)]
pub struct ResultEvent {
    #[serde(default)]
    pub subtype: Option<String>,
    #[serde(default)]
    pub is_error: bool,
    #[serde(default)]
    pub stop_reason: Option<String>,
    #[serde(default)]
    pub num_turns: Option<u32>,
    #[serde(default)]
    pub duration_ms: Option<u64>,
    #[serde(default)]
    pub total_cost_usd: Option<f64>,
}

/// Parse one NDJSON line. A malformed or unrecognized line is an `Err`/`Unknown`
/// rather than a panic — the session must survive protocol drift.
pub fn parse_line(line: &str) -> Result<Event, serde_json::Error> {
    serde_json::from_str(line)
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// The folded view of one `claude` session.
///
/// Everything here is what a client could observe, with one exception:
/// `counts` is per-event-type bookkeeping for diagnostics and is deliberately
/// left out of the serialized form, so the golden tests pin outcomes rather
/// than the reducer's internal tallies.
#[derive(Debug, Default, Serialize)]
pub struct SessionState {
    pub session_id: Option<String>,
    pub model: Option<String>,
    pub cwd: Option<String>,
    pub permission_mode: Option<String>,
    pub tool_count: usize,

    /// Completed turns, in order.
    pub transcript: Vec<Turn>,
    /// Text accumulated from `content_block_delta` for the in-flight turn.
    pub live_text: String,
    pub streaming: bool,

    pub last_result: Option<ResultSummary>,
    /// Protocol-drift telemetry: event types we did not recognize.
    pub unknown_events: usize,
    pub parse_errors: usize,
    #[serde(skip)]
    pub counts: BTreeMap<String, usize>,
}

#[derive(Debug, Serialize)]
pub struct Turn {
    pub role: String,
    pub text: String,
    /// True when the text came from accumulated deltas rather than the
    /// buffered `assistant` event — see `reduce`'s reconcile step.
    pub from_deltas: bool,
}

/// What folding one line changed, for a caller that has to tell somebody.
///
/// The reducer was written for the spike, where the whole of the answer was the
/// final state and nothing needed to know *when* a piece of it arrived. A server
/// streaming a turn does: it has to publish a delta the moment it lands and the
/// reconciled message the moment that does.
///
/// It is a return value rather than a callback, and rather than a second reducer
/// in the driver, because the accumulate-and-reconcile rule has to exist in
/// exactly one place. A driver that re-implemented "the buffered message wins"
/// alongside this one would be two rules that agree until they do not, and the
/// one that the golden files check would not be the one the UI sees.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Folded {
    /// A line that changed nothing a client would notice — a hook event, a
    /// block boundary, a rate-limit notice, drift, or a line that did not parse.
    Nothing,
    /// `system`/`init`: the session id, model, working directory and permission
    /// mode are now known, and are on the state.
    Initialized,
    /// Live text arrived. Carries the delta itself rather than a position,
    /// because the accumulation is cleared out from under the caller when the
    /// turn reconciles.
    Streamed(String),
    /// A complete message joined `transcript` at this index. Reading it there
    /// gives the role, the authoritative text, and whether the deltas agreed.
    Turn { index: usize },
    /// The terminal `result` for the turn. `last_result` holds the duration and
    /// the cost.
    Completed,
}

#[derive(Debug, Serialize)]
pub struct ResultSummary {
    pub is_error: bool,
    pub stop_reason: Option<String>,
    pub num_turns: Option<u32>,
    pub duration_ms: Option<u64>,
    pub total_cost_usd: Option<f64>,
}

impl SessionState {
    pub fn new() -> Self {
        Self::default()
    }

    fn bump(&mut self, key: &str) {
        *self.counts.entry(key.to_string()).or_insert(0) += 1;
    }

    /// Fold one raw NDJSON line, malformed ones included.
    ///
    /// This is the entry point a stdio pump wants: a blank line is nothing (the
    /// CLI's output ends with one), and a line that does not parse is counted
    /// rather than returned, because one bad line must not end a session any
    /// more than one unrecognized event type does.
    pub fn fold_line(&mut self, line: &str) -> Folded {
        if line.trim().is_empty() {
            return Folded::Nothing;
        }
        match parse_line(line) {
            Ok(event) => self.reduce(event),
            Err(_) => {
                self.note_parse_error();
                Folded::Nothing
            }
        }
    }

    /// Fold one event into the state.
    ///
    /// The interesting decision lives here. Assistant text arrives twice: once
    /// as incremental `content_block_delta`s (good for live rendering) and
    /// again as a complete buffered `assistant` message. We render deltas for
    /// responsiveness, then let the buffered message replace the accumulated
    /// text when it lands — deltas are best-effort and may be shed, so the
    /// buffered message is authoritative. This is the accumulate-and-reconcile
    /// pattern, and it is the shape the real server should use too.
    pub fn reduce(&mut self, event: Event) -> Folded {
        match event {
            Event::System(SystemEvent::Init(init)) => {
                self.bump("system/init");
                self.session_id = Some(init.session_id);
                self.model = Some(init.model);
                self.cwd = Some(init.cwd);
                self.permission_mode = Some(init.permission_mode);
                self.tool_count = init.tools.len();
                Folded::Initialized
            }
            Event::System(SystemEvent::Other) => {
                self.bump("system/other");
                Folded::Nothing
            }

            Event::StreamEvent { event } => match event {
                StreamEvent::MessageStart {} => {
                    self.bump("stream/message_start");
                    self.streaming = true;
                    self.live_text.clear();
                    Folded::Nothing
                }
                StreamEvent::ContentBlockDelta { delta, .. } => {
                    self.bump("stream/content_block_delta");
                    match delta {
                        Delta::TextDelta { text } => {
                            self.live_text.push_str(&text);
                            Folded::Streamed(text)
                        }
                        // Thinking, signature and tool-input deltas all land
                        // here. They are not the assistant's visible text, so
                        // there is nothing to append and nothing to publish.
                        Delta::Unknown => Folded::Nothing,
                    }
                }
                StreamEvent::MessageStop => {
                    self.bump("stream/message_stop");
                    self.streaming = false;
                    Folded::Nothing
                }
                StreamEvent::ContentBlockStart { .. } => {
                    self.bump("stream/content_block_start");
                    Folded::Nothing
                }
                StreamEvent::ContentBlockStop { .. } => {
                    self.bump("stream/content_block_stop");
                    Folded::Nothing
                }
                StreamEvent::MessageDelta {} => {
                    self.bump("stream/message_delta");
                    Folded::Nothing
                }
                StreamEvent::Unknown => {
                    self.bump("stream/UNKNOWN");
                    self.unknown_events += 1;
                    Folded::Nothing
                }
            },

            Event::Assistant(env) => {
                self.bump("assistant");
                // Reconcile: the buffered message is authoritative.
                let text = flatten(&env.message);
                let from_deltas = !self.live_text.is_empty() && self.live_text == text;
                self.transcript.push(Turn {
                    role: env.message.role,
                    text,
                    from_deltas,
                });
                self.live_text.clear();
                Folded::Turn {
                    index: self.transcript.len() - 1,
                }
            }

            Event::User(env) => {
                self.bump("user");
                let text = flatten(&env.message);
                self.transcript.push(Turn {
                    role: env.message.role,
                    text,
                    from_deltas: false,
                });
                Folded::Turn {
                    index: self.transcript.len() - 1,
                }
            }

            Event::Result(r) => {
                self.bump("result");
                self.streaming = false;
                self.last_result = Some(ResultSummary {
                    is_error: r.is_error,
                    stop_reason: r.stop_reason,
                    num_turns: r.num_turns,
                    duration_ms: r.duration_ms,
                    total_cost_usd: r.total_cost_usd,
                });
                Folded::Completed
            }

            Event::RateLimitEvent(_) => {
                self.bump("rate_limit_event");
                Folded::Nothing
            }

            Event::Unknown => {
                self.bump("UNKNOWN");
                self.unknown_events += 1;
                Folded::Nothing
            }
        }
    }

    pub fn note_parse_error(&mut self) {
        self.parse_errors += 1;
        self.bump("PARSE_ERROR");
    }

    /// What the UI would render right now for the in-flight turn.
    pub fn visible_text(&self) -> &str {
        &self.live_text
    }
}

fn flatten(message: &Message) -> String {
    let mut out = String::new();
    for block in &message.content {
        match block {
            ContentBlock::Text { text } => out.push_str(text),
            ContentBlock::ToolUse { name } => out.push_str(&format!("[tool_use: {name}]")),
            ContentBlock::Thinking {} => out.push_str("[thinking]"),
            ContentBlock::ToolResult {} => out.push_str("[tool_result]"),
            ContentBlock::Unknown => out.push_str("[?]"),
        }
    }
    out
}

/// Build the stdin line for a user turn. The CLI reads NDJSON on stdin under
/// `--input-format stream-json`; this is the whole of the outbound protocol.
pub fn user_message_line(text: &str) -> String {
    serde_json::json!({
        "type": "user",
        "message": { "role": "user", "content": [{ "type": "text", "text": text }] }
    })
    .to_string()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
//
// Whole captured sessions are covered by the golden files in
// `tests/protocol_golden.rs`. What lives here is the degradation behaviour that
// no real capture contains, because a healthy CLI never emits it.

#[cfg(test)]
mod tests {
    use super::*;

    fn fold(lines: &[&str]) -> SessionState {
        let mut state = SessionState::new();
        for line in lines {
            state.fold_line(line);
        }
        state
    }

    /// What each line changed, in order — the driver's view of the same fold.
    fn outcomes(lines: &[&str]) -> Vec<Folded> {
        let mut state = SessionState::new();
        lines.iter().map(|line| state.fold_line(line)).collect()
    }

    #[test]
    fn unrecognized_event_type_becomes_a_drift_count() {
        let state = fold(&[r#"{"type":"telemetry_event","payload":{"x":1}}"#]);

        assert_eq!(state.unknown_events, 1);
        assert_eq!(state.parse_errors, 0);
        assert!(state.transcript.is_empty());
    }

    #[test]
    fn unrecognized_stream_event_becomes_a_drift_count() {
        let state = fold(&[
            r#"{"type":"stream_event","event":{"type":"citation_delta","index":0}}"#,
        ]);

        assert_eq!(state.unknown_events, 1);
        assert_eq!(state.parse_errors, 0);
    }

    #[test]
    fn malformed_line_becomes_a_parse_error_count() {
        let state = fold(&["{not json", "", "   "]);

        assert_eq!(state.parse_errors, 1);
        assert_eq!(state.unknown_events, 0);
    }

    #[test]
    fn a_session_keeps_folding_across_drift_and_malformed_lines() {
        let state = fold(&[
            r#"{"type":"telemetry_event"}"#,
            "}{",
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"still here"}]}}"#,
            r#"{"type":"result","subtype":"success","is_error":false,"stop_reason":"end_turn"}"#,
        ]);

        assert_eq!(state.transcript.len(), 1);
        assert_eq!(state.transcript[0].text, "still here");
        assert_eq!(
            state.last_result.as_ref().and_then(|r| r.stop_reason.clone()),
            Some("end_turn".to_string())
        );
        assert_eq!((state.unknown_events, state.parse_errors), (1, 1));
    }

    #[test]
    fn an_unrecognized_content_block_still_yields_a_turn() {
        let state = fold(&[
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"hi "},{"type":"holograph"}]}}"#,
        ]);

        assert_eq!(state.transcript[0].text, "hi [?]");
    }

    #[test]
    fn the_buffered_message_replaces_the_delta_accumulation() {
        let state = fold(&[
            r#"{"type":"stream_event","event":{"type":"message_start"}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hel"}}}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"hello"}]}}"#,
        ]);

        // Deltas were shed mid-turn; the buffered message wins and says so.
        assert_eq!(state.transcript[0].text, "hello");
        assert!(!state.transcript[0].from_deltas);
        assert_eq!(state.visible_text(), "");
    }

    #[test]
    fn deltas_that_agree_with_the_buffered_message_are_recorded_as_agreeing() {
        let state = fold(&[
            r#"{"type":"stream_event","event":{"type":"message_start"}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hello"}}}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"hello"}]}}"#,
        ]);

        assert!(state.transcript[0].from_deltas);
    }

    // -- what the driver is told ---------------------------------------------

    /// One turn, line by line, as the thing that has to publish it sees it: the
    /// session announces itself, text arrives in pieces, a whole message lands,
    /// and the turn ends. Everything in between is a line with nothing to say.
    #[test]
    fn a_streamed_turn_reports_each_line_that_a_client_would_notice() {
        let told = outcomes(&[
            r#"{"type":"system","subtype":"init","session_id":"s","model":"m","cwd":"/tmp","permissionMode":"default"}"#,
            r#"{"type":"stream_event","event":{"type":"message_start"}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_start","index":0}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hel"}}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"lo"}}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_stop","index":0}}"#,
            r#"{"type":"stream_event","event":{"type":"message_stop"}}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"hello"}]}}"#,
            r#"{"type":"result","subtype":"success","duration_ms":12,"total_cost_usd":0.5}"#,
        ]);

        assert_eq!(
            told,
            vec![
                Folded::Initialized,
                Folded::Nothing,
                Folded::Nothing,
                Folded::Streamed("hel".to_string()),
                Folded::Streamed("lo".to_string()),
                Folded::Nothing,
                Folded::Nothing,
                Folded::Turn { index: 0 },
                Folded::Completed,
            ]
        );
    }

    /// The deltas a healthy turn is full of that are *not* the visible reply —
    /// reasoning, block signatures, tool arguments. Publishing one as assistant
    /// text would put the model's thinking in the transcript.
    #[test]
    fn a_delta_that_is_not_visible_text_reports_nothing() {
        assert_eq!(
            outcomes(&[
                r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"hmm"}}}"#,
                r#"{"type":"stream_event","event":{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"a\":"}}}"#,
            ]),
            vec![Folded::Nothing, Folded::Nothing]
        );
    }

    /// Drift and malformed lines report nothing rather than something a driver
    /// would try to publish. This is the same survival property the counters
    /// record, seen from the side that has a client attached.
    #[test]
    fn drift_and_malformed_lines_report_nothing_to_publish() {
        assert_eq!(
            outcomes(&[
                r#"{"type":"telemetry_event"}"#,
                "}{",
                "",
                r#"{"type":"stream_event","event":{"type":"citation_delta","index":0}}"#,
            ]),
            vec![Folded::Nothing; 4]
        );
    }
}
