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
use serde_json::Value;

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
    /// The CLI asking its host something, over the same stdout the events arrive
    /// on. The only line that expects a reply, and the only one the conversation
    /// stops for until it gets one.
    ControlRequest {
        request_id: String,
        request: Ask,
    },
    /// Terminal event for the turn.
    Result(ResultEvent),
    #[serde(other)]
    Unknown,
}

/// What a `control_request` is asking.
///
/// The CLI has several — `hook_callback`, `mcp_message`, `elicitation`,
/// `request_user_dialog`, `oauth_token_refresh` — and this server registers for
/// exactly one by passing `--permission-prompt-tool stdio` and nothing else.
/// Everything else arrives as [`Ask::Other`] and is counted as drift, which is
/// the honest report for a question with no answer: a request this build made up
/// an answer to would be worse than one it admits it cannot take.
#[derive(Debug, Deserialize)]
#[serde(tag = "subtype", rename_all = "snake_case")]
pub enum Ask {
    CanUseTool(Box<Permission>),
    #[serde(other)]
    Other,
}

/// The agent asking to use a tool, and everything needed to answer.
///
/// Recorded rather than read off a contract: `fixtures/claude-cli/07`–`10` are
/// real requests from `claude` 2.1.220, and the optionality below is theirs —
/// only `tool_name` and `input` were present on every one.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Permission {
    /// The envelope's `request_id`, copied in by [`SessionState::reduce`] so a
    /// request is one thing rather than two.
    ///
    /// Never read from the body: the id lives on the envelope, and a body that
    /// carried one of its own would be describing a different request.
    #[serde(default, skip_deserializing)]
    pub request_id: String,
    pub tool_name: String,
    /// What the tool would be run with. Sent back verbatim on an approval —
    /// `updatedInput` is where a host that wanted to *edit* the call would put
    /// its edit, and this server does not.
    #[serde(default)]
    pub input: Value,
    /// The `tool_use` block this permission is for, when the CLI names one.
    ///
    /// What joins the approval row to the tool row beside it in the work log.
    #[serde(default)]
    pub tool_use_id: Option<String>,
    /// The CLI's own one-line summary of what is being asked — `note.txt` for a
    /// `Write`. Shown in preference to anything this server could derive.
    #[serde(default)]
    pub description: Option<String>,
    /// Permission updates the CLI suggests if the developer wants to stop being
    /// asked. Carried verbatim and handed straight back on "always allow", which
    /// is the whole of how that answer works — see [`Answer::Allow`].
    #[serde(default, rename = "permission_suggestions")]
    pub suggestions: Vec<Value>,
}

/// What the developer decided, in the CLI's own vocabulary.
///
/// Deliberately *not* the client's — the UI offers four decisions
/// (`accept`, `acceptForSession`, `decline`, `cancel`) and the wire has two
/// behaviours with a modifier on each. Translating between them is
/// [`crate::orchestration`]'s job, so that this module stays a description of
/// what `claude` accepts and nothing else.
#[derive(Debug, Clone, PartialEq)]
pub enum Answer {
    /// Let the tool run.
    Allow {
        /// The input to run with — the request's own, unedited.
        input: Value,
        /// Permission updates for the CLI to apply, which is what stops it
        /// asking again. Empty means "this once".
        remember: Vec<Value>,
    },
    /// Do not let the tool run. The message reaches the model as the tool's
    /// result, so it is what the agent is told rather than only what the
    /// developer clicked.
    Deny {
        message: String,
        /// Stop the turn as well as the tool.
        interrupt: bool,
    },
}

impl Answer {
    /// What the CLI records this decision as, for its own telemetry.
    ///
    /// It asks hosts that actually prompt a person to say what happened rather
    /// than leave it unset, and lightcode is one — every answer here came from a
    /// developer clicking something.
    fn classification(&self) -> &'static str {
        match self {
            Answer::Allow { remember, .. } if remember.is_empty() => "user_temporary",
            Answer::Allow { .. } => "user_permanent",
            Answer::Deny { .. } => "user_reject",
        }
    }
}

/// Build the stdin line answering one `control_request`.
///
/// The second half of the outbound protocol, beside [`user_message_line`]. The
/// envelope is doubly nested — a `control_response` carrying a `success` result
/// carrying the permission decision — which is the CLI's shape rather than a
/// choice here; the same envelope with `"subtype": "error"` is how a host says it
/// could not answer at all.
pub fn control_response_line(request_id: &str, answer: &Answer) -> String {
    let mut decision = match answer {
        Answer::Allow { input, remember } => {
            let mut allow = serde_json::json!({
                "behavior": "allow",
                "updatedInput": input,
            });
            // Absent rather than empty: the CLI types this as an optional array,
            // and `[]` is a claim that nothing should be remembered rather than
            // the absence of a claim.
            if !remember.is_empty() {
                allow["updatedPermissions"] = Value::Array(remember.clone());
            }
            allow
        }
        Answer::Deny { message, interrupt } => {
            let mut deny = serde_json::json!({
                "behavior": "deny",
                "message": message,
            });
            if *interrupt {
                deny["interrupt"] = Value::Bool(true);
            }
            deny
        }
    };
    decision["decisionClassification"] = Value::String(answer.classification().to_string());

    serde_json::json!({
        "type": "control_response",
        "response": {
            "subtype": "success",
            "request_id": request_id,
            "response": decision,
        },
    })
    .to_string()
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
    #[serde(default, deserialize_with = "readable_blocks")]
    pub content: Vec<ContentBlock>,
    #[serde(default)]
    pub stop_reason: Option<String>,
}

/// The message's blocks, with anything this build cannot read reduced to
/// [`ContentBlock::Unknown`].
///
/// Needed because `#[serde(other)]` catches an unrecognized `type` and nothing
/// else: a `tool_use` with the *id missing* is a recognized tag whose fields do
/// not fit, and without this the whole line fails to parse — so a block that
/// drifted would cost the reply beside it. The module's own rule is that a format
/// change has the blast radius of one file; this makes it the blast radius of one
/// block.
fn readable_blocks<'de, D>(deserializer: D) -> Result<Vec<ContentBlock>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    /// `untagged` tries the real block first and falls back to swallowing
    /// whatever was there, which is the only way to say "unreadable" without
    /// knowing in advance how it will be unreadable.
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Readable {
        Block(ContentBlock),
        Unreadable(serde::de::IgnoredAny),
    }

    Ok(Vec::<Readable>::deserialize(deserializer)?
        .into_iter()
        .map(|block| match block {
            Readable::Block(block) => block,
            Readable::Unreadable(_) => ContentBlock::Unknown,
        })
        .collect())
}

/// One block of a message's content.
///
/// Ticket 10 needed only the text and reduced everything else to a placeholder.
/// Ticket 12 needs the rest, because a tool call *is* these fields: the id is
/// what pairs an invocation with its result, the name is what the developer reads,
/// and the input is what they need to see it was given.
///
/// Two things are deliberately not read:
///
/// - **A thinking block's `signature`.** It is a few hundred bytes of opaque
///   base64 that only the API has any use for, and nothing here forwards a
///   thinking block back to it.
/// - **`server_tool_use` and `mcp_tool_use`.** Both are tool calls the *API*
///   runs, so their results come back as their own block types rather than as a
///   `tool_result` in the next user message — an invocation this server could
///   announce and never settle. No capture contains one; when one does, it
///   arrives here as [`ContentBlock::Unknown`] and is counted as drift, which is
///   the honest answer until there is something to pair it with.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    /// The model reasoning before it answered.
    Thinking {
        #[serde(default)]
        thinking: String,
    },
    /// The agent invoking a tool. `input` is whatever that tool's schema says,
    /// so it is carried verbatim rather than typed.
    ToolUse {
        id: String,
        name: String,
        #[serde(default)]
        input: Value,
    },
    /// What the tool returned, in the *next* message and with the role `user` —
    /// the Messages API's shape, and the reason [`SessionState::reduce`] folds a
    /// user message rather than ignoring it.
    ///
    /// `content` is a string on the captures here and is permitted to be an array
    /// of blocks, so it too is carried verbatim; [`text_content`] is how it
    /// becomes something to show.
    ToolResult {
        tool_use_id: String,
        #[serde(default)]
        content: Value,
        #[serde(default)]
        is_error: bool,
    },
    #[serde(other)]
    Unknown,
}

/// A `tool_result`'s content as text.
///
/// The Messages API permits a string or an array of blocks, and the CLI produces
/// both — a string for a file read, an array when a tool returns images beside its
/// text. Anything that is not text contributes nothing, because there is nowhere
/// in this contract's work log to render it.
pub fn text_content(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter(|block| block.get("type") == Some(&Value::String("text".to_string())))
            .filter_map(|block| block.get("text").and_then(Value::as_str))
            .collect::<Vec<&str>>()
            .join(""),
        _ => String::new(),
    }
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

    /// Every permission the agent has asked for, in the order it asked.
    ///
    /// Kept rather than only reported, for the same reason `transcript` is: the
    /// driver reads the request back out by index, and a golden file that showed
    /// the requests is how a change to their shape becomes visible.
    pub permissions: Vec<Permission>,

    pub last_result: Option<ResultSummary>,
    /// Protocol-drift telemetry: event types we did not recognize.
    pub unknown_events: usize,
    pub parse_errors: usize,
    #[serde(skip)]
    pub counts: BTreeMap<String, usize>,
}

/// One complete message the agent buffered, as the driver has to publish it.
///
/// The CLI emits one of these **per content block** rather than one per API
/// message — `fixtures/claude-cli/04-tool-use.ndjson` shows a single
/// `message_start` producing an `assistant` line for its thinking block and
/// another for its tool call — so a turn that uses a tool arrives here as several
/// of these, in the order the blocks closed. That order is what makes the work
/// log's order right without anything having to sort it.
#[derive(Debug, Serialize)]
pub struct Turn {
    pub role: String,
    /// The message's *visible* text and nothing else.
    ///
    /// Derivable from `content`, and kept beside it because this is the string
    /// that has to equal the delta accumulation for `from_deltas` to mean
    /// anything, and the string the driver puts in the transcript. Before ticket
    /// 12 it also carried `[thinking]` and `[tool_use: Read]` placeholders for the
    /// blocks it could not describe — which put both in the developer's chat
    /// bubble, verbatim.
    pub text: String,
    /// True when the text came from accumulated deltas rather than the
    /// buffered `assistant` event — see `reduce`'s reconcile step.
    pub from_deltas: bool,
    /// Every block the message carried, in order. What the driver reads to find
    /// the tool calls, their results and the reasoning between them.
    pub content: Vec<ContentBlock>,
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
    /// The agent is waiting for permission before it can go on. The request
    /// joined `permissions` at this index; answering it is
    /// [`control_response_line`], and until something does the agent is stopped.
    PermissionRequested { index: usize },
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
                self.note_unreadable_blocks(&env.message.content);
                // Reconcile: the buffered message is authoritative.
                let text = visible(&env.message);
                let from_deltas = !self.live_text.is_empty() && self.live_text == text;
                self.transcript.push(Turn {
                    role: env.message.role,
                    text,
                    from_deltas,
                    content: env.message.content,
                });
                self.live_text.clear();
                Folded::Turn {
                    index: self.transcript.len() - 1,
                }
            }

            // Folded rather than dropped, and not because this server asked for
            // the developer's own turns back — it does not pass
            // `--replay-user-messages`. A **tool result** arrives as a user
            // message, which is the Messages API's shape, so this is the only
            // place a tool call can be seen to have returned.
            Event::User(env) => {
                self.bump("user");
                self.note_unreadable_blocks(&env.message.content);
                let text = visible(&env.message);
                self.transcript.push(Turn {
                    role: env.message.role,
                    text,
                    from_deltas: false,
                    content: env.message.content,
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

            // The one line the agent stops for. Nothing else it says needs a
            // reply, and nothing else it says leaves the turn unable to
            // continue until one arrives.
            Event::ControlRequest {
                request_id,
                request: Ask::CanUseTool(asked),
            } => {
                self.bump("control_request/can_use_tool");
                self.permissions.push(Permission {
                    request_id,
                    ..*asked
                });
                Folded::PermissionRequested {
                    index: self.permissions.len() - 1,
                }
            }
            Event::ControlRequest {
                request: Ask::Other,
                ..
            } => {
                self.bump("control_request/UNKNOWN");
                self.unknown_events += 1;
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

    /// Count the blocks in a message this build could not read.
    ///
    /// A block arrives as [`ContentBlock::Unknown`] either because its `type` is
    /// one this build does not know — `server_tool_use` — or because a type it does
    /// know did not carry the fields that make it usable. Either way the driver
    /// publishes nothing for it, so without a count the only account of a format
    /// change would be a row missing from a work log nobody was watching. This is
    /// the same drift number an unrecognized event type feeds, and `turn.completed`
    /// carries the total to where a developer is already looking.
    ///
    /// Before ticket 12 an unreadable block was *visible* instead: the flattened
    /// text carried a `[?]` where it had been. Counting it is what replaces that.
    fn note_unreadable_blocks(&mut self, content: &[ContentBlock]) {
        let unreadable = content
            .iter()
            .filter(|block| matches!(block, ContentBlock::Unknown))
            .count();
        if unreadable > 0 {
            self.unknown_events += unreadable;
            *self
                .counts
                .entry("content_block/UNKNOWN".to_string())
                .or_insert(0) += unreadable;
        }
    }

    /// What the UI would render right now for the in-flight turn.
    pub fn visible_text(&self) -> &str {
        &self.live_text
    }
}

/// A message's visible text: its text blocks, joined, and nothing else.
///
/// Everything else the message carried is on [`Turn::content`] for the driver to
/// publish as what it actually is. Describing those blocks *in the text* — which
/// is what this used to do, with `[thinking]` and `[tool_use: Read]` placeholders
/// — put the model's reasoning and its tool arguments in the developer's chat
/// bubble as prose, and made the reply the transcript held not the reply the agent
/// gave.
fn visible(message: &Message) -> String {
    message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
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
    use serde_json::json;

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

        // The text is the message's text and nothing else. A block this build
        // cannot describe is on `content` as `Unknown`, where the driver ignores
        // it — rather than described *in the reply*, which is where a `[?]`
        // placeholder used to put it.
        assert_eq!(state.transcript[0].text, "hi ");
        assert_eq!(
            state.transcript[0].content,
            vec![
                ContentBlock::Text {
                    text: "hi ".to_string()
                },
                ContentBlock::Unknown,
            ]
        );
        // And it is counted, because that placeholder was the only account of it
        // there used to be. A block the driver silently skips is drift, and drift
        // is a number here rather than a row nobody noticed was missing.
        assert_eq!(state.unknown_events, 1);
    }

    // -- tool use ------------------------------------------------------------
    //
    // Whole captured tool-use sessions are covered by the golden files. What
    // lives here is the shape of one block, because that is what the driver
    // matches on, and the degradation no capture contains.

    /// The three fields a tool call is: the id that pairs it with its result, the
    /// name the developer reads, and the input they need to see it was given.
    #[test]
    fn a_tool_call_carries_its_id_its_name_and_its_input() {
        let state = fold(&[
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_1","name":"Read","input":{"file_path":"note.txt"},"caller":{"type":"direct"}}]}}"#,
        ]);

        assert_eq!(
            state.transcript[0].content,
            vec![ContentBlock::ToolUse {
                id: "toolu_1".to_string(),
                name: "Read".to_string(),
                input: json!({"file_path": "note.txt"}),
            }]
        );
        // And nothing of it reaches the reply. A message that is only a tool call
        // has no text, which is what stops it appearing as an empty chat bubble.
        assert_eq!(state.transcript[0].text, "");
    }

    /// A result names the call it answers and says whether the tool failed. Both
    /// come off the block itself, and `is_error` is *absent* on a call that went
    /// well — so the successful case is the one that has to default correctly.
    ///
    /// A result arrives with the role `user`, which is why it has to be reported as
    /// something rather than skipped: this server does not pass
    /// `--replay-user-messages`, so a user message is never the developer's own
    /// turn coming back.
    #[test]
    fn a_tool_result_names_its_call_and_whether_it_failed() {
        let state = fold(&[
            r#"{"type":"user","message":{"role":"user","content":[{"tool_use_id":"toolu_1","type":"tool_result","content":"1\tthe answer is 42\n"}]}}"#,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_2","content":"File does not exist.","is_error":true}]}}"#,
        ]);

        assert_eq!(
            state.transcript[0].content,
            vec![ContentBlock::ToolResult {
                tool_use_id: "toolu_1".to_string(),
                content: json!("1\tthe answer is 42\n"),
                is_error: false,
            }]
        );
        assert_eq!(
            state.transcript[1].content,
            vec![ContentBlock::ToolResult {
                tool_use_id: "toolu_2".to_string(),
                content: json!("File does not exist."),
                is_error: true,
            }]
        );
        // And neither is the user talking, so neither is the user's text.
        assert_eq!(state.transcript[0].text, "");
        assert_eq!(state.transcript[1].text, "");
    }

    /// The model's reasoning, without the signature beside it — a few hundred
    /// bytes of base64 nothing here forwards anywhere.
    #[test]
    fn a_thinking_block_carries_the_reasoning_and_not_the_signature() {
        let state = fold(&[
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"thinking","thinking":"read the file first","signature":"Ev8CCpMBCBAY"}]}}"#,
        ]);

        assert_eq!(
            state.transcript[0].content,
            vec![ContentBlock::Thinking {
                thinking: "read the file first".to_string()
            }]
        );
        assert_eq!(state.transcript[0].text, "");
    }

    /// A `tool_result`'s content is a string on every capture here and is
    /// permitted to be an array of blocks. Both have to become the same kind of
    /// answer, and anything that is not text contributes nothing — there is
    /// nowhere in the work log to render it.
    #[test]
    fn a_results_content_reads_as_text_whichever_shape_it_arrived_in() {
        assert_eq!(text_content(&json!("plain output")), "plain output");
        assert_eq!(
            text_content(&json!([
                {"type": "text", "text": "first "},
                {"type": "image", "source": {}},
                {"type": "text", "text": "second"},
            ])),
            "first second"
        );
        assert_eq!(text_content(&Value::Null), "");
        assert_eq!(text_content(&json!({"unexpected": true})), "");
    }

    /// A tool call missing the fields that make it one is drift rather than a
    /// half-built call: publishing an invocation with no id would announce
    /// something no result could ever be paired with.
    ///
    /// The cost of it is a *block*, not the message around it — `#[serde(other)]`
    /// catches an unrecognized `type` and nothing else, so without the fallback the
    /// whole line would fail to parse and take the reply beside it.
    #[test]
    fn a_tool_block_without_the_fields_that_pair_it_is_drift() {
        let state = fold(&[
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"still said"},{"type":"tool_use","name":"Read"},{"type":"tool_result","content":"x"}]}}"#,
        ]);

        assert_eq!(
            state.transcript[0].content,
            vec![
                ContentBlock::Text {
                    text: "still said".to_string()
                },
                ContentBlock::Unknown,
                ContentBlock::Unknown,
            ],
            "a block that cannot be paired must not look like one that can"
        );
        assert_eq!(
            state.transcript[0].text, "still said",
            "a drifted block cost the reply beside it"
        );
        assert_eq!((state.unknown_events, state.parse_errors), (2, 0));
    }

    // -- permission requests --------------------------------------------------
    //
    // The one place the CLI *asks* rather than tells, and the only inbound
    // message this server sends besides a turn. Whole captured sessions are in
    // the golden files; what lives here is the shape of one request and the
    // shape of each answer, because those are what the driver matches on and
    // writes back.

    /// The fields a permission request is, off
    /// `fixtures/claude-cli/08-permission-declined.ndjson`: the id the answer has
    /// to name, the tool, what it was given, and the call it will become.
    #[test]
    fn a_permission_request_carries_the_id_the_answer_names_and_the_call_it_is_for() {
        let mut state = SessionState::new();
        let told = state.fold_line(
            r#"{"type":"control_request","request_id":"req-1","request":{"subtype":"can_use_tool","tool_name":"Write","display_name":"Write","input":{"file_path":"note.txt","content":"hello"},"description":"note.txt","permission_suggestions":[{"type":"setMode","mode":"acceptEdits","destination":"session"}],"tool_use_id":"toolu_1"}}"#,
        );

        assert_eq!(told, Folded::PermissionRequested { index: 0 });
        let asked = &state.permissions[0];
        assert_eq!(asked.request_id, "req-1");
        assert_eq!(asked.tool_name, "Write");
        assert_eq!(asked.tool_use_id.as_deref(), Some("toolu_1"));
        assert_eq!(asked.description.as_deref(), Some("note.txt"));
        assert_eq!(asked.input["file_path"], "note.txt");
        // Carried verbatim, because this is what an "always allow" answer sends
        // back — see `Answer::Allow`.
        assert_eq!(
            asked.suggestions,
            vec![json!({"type": "setMode", "mode": "acceptEdits", "destination": "session"})]
        );
    }

    /// A request with only the fields the CLI guarantees. `description`,
    /// `tool_use_id` and the suggestions are all optional on the wire, and a
    /// request missing them still has to be answerable — an unanswered one wedges
    /// the turn.
    #[test]
    fn a_permission_request_with_only_its_required_fields_is_still_one() {
        let mut state = SessionState::new();
        let told = state.fold_line(
            r#"{"type":"control_request","request_id":"req-2","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"ls"}}}"#,
        );

        assert_eq!(told, Folded::PermissionRequested { index: 0 });
        assert_eq!(state.permissions[0].tool_use_id, None);
        assert_eq!(state.permissions[0].description, None);
        assert!(state.permissions[0].suggestions.is_empty());
    }

    /// A control request this build cannot act on is drift rather than a
    /// permission nobody answers. The CLI sends several — `hook_callback`,
    /// `mcp_message`, `request_user_dialog` — and each of them is a question this
    /// server has no answer for, so counting them is the honest report.
    #[test]
    fn a_control_request_that_is_not_a_permission_is_counted_rather_than_asked() {
        let mut state = SessionState::new();
        let told = state.fold_line(
            r#"{"type":"control_request","request_id":"req-3","request":{"subtype":"request_user_dialog","dialog_kind":"plan"}}"#,
        );

        assert_eq!(told, Folded::Nothing);
        assert!(state.permissions.is_empty());
        assert_eq!(state.unknown_events, 1);
    }

    /// The three answers, as the CLI's own schema spells them. Each was recorded
    /// against the real binary before it was written down here — see
    /// `fixtures/claude-cli/07`, `08` and `10`.
    #[test]
    fn each_answer_is_the_control_response_the_cli_expects() {
        let approved = Answer::Allow {
            input: json!({"file_path": "note.txt"}),
            remember: Vec::new(),
        };
        assert_eq!(
            serde_json::from_str::<Value>(&control_response_line("req-1", &approved)).unwrap(),
            json!({
                "type": "control_response",
                "response": {
                    "subtype": "success",
                    "request_id": "req-1",
                    "response": {
                        "behavior": "allow",
                        "updatedInput": {"file_path": "note.txt"},
                        "decisionClassification": "user_temporary",
                    },
                },
            })
        );

        // "Always allow this session" is the request's own suggestions handed
        // back. The CLI applies them itself, which is what stops it asking again.
        let for_the_session = Answer::Allow {
            input: json!({"file_path": "note.txt"}),
            remember: vec![json!({"type": "setMode", "mode": "acceptEdits", "destination": "session"})],
        };
        let line: Value =
            serde_json::from_str(&control_response_line("req-1", &for_the_session)).unwrap();
        assert_eq!(
            line["response"]["response"]["updatedPermissions"],
            json!([{"type": "setMode", "mode": "acceptEdits", "destination": "session"}])
        );
        assert_eq!(
            line["response"]["response"]["decisionClassification"],
            "user_permanent"
        );

        let declined = Answer::Deny {
            message: "The developer declined this action.".to_string(),
            interrupt: false,
        };
        assert_eq!(
            serde_json::from_str::<Value>(&control_response_line("req-1", &declined)).unwrap(),
            json!({
                "type": "control_response",
                "response": {
                    "subtype": "success",
                    "request_id": "req-1",
                    "response": {
                        "behavior": "deny",
                        "message": "The developer declined this action.",
                        "decisionClassification": "user_reject",
                    },
                },
            })
        );

        // Cancelling the turn is a denial that also stops the agent, which is one
        // flag on the same answer rather than a second mechanism.
        let cancelled = Answer::Deny {
            message: "The developer cancelled the turn.".to_string(),
            interrupt: true,
        };
        let line: Value =
            serde_json::from_str(&control_response_line("req-1", &cancelled)).unwrap();
        assert_eq!(line["response"]["response"]["interrupt"], json!(true));
    }

    /// An allow with no `updatedPermissions` must not carry the key at all: the
    /// CLI's schema types it as an optional array, and an empty one is a claim
    /// that nothing should be remembered rather than the absence of a claim.
    #[test]
    fn an_allow_that_remembers_nothing_says_nothing_about_permissions() {
        let line: Value = serde_json::from_str(&control_response_line(
            "req-1",
            &Answer::Allow {
                input: Value::Null,
                remember: Vec::new(),
            },
        ))
        .unwrap();

        assert!(
            line["response"]["response"].get("updatedPermissions").is_none(),
            "{line}"
        );
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
