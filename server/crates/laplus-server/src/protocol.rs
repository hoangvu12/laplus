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
    /// The account's rate-limit standing changed. Emitted whenever the CLI's
    /// view of it moves, so most of them say everything is fine — see
    /// [`RateLimit::worth_reporting`].
    RateLimitEvent {
        #[serde(default)]
        rate_limit_info: Option<RateLimit>,
    },
    /// The CLI asking its host something, over the same stdout the events arrive
    /// on. The only line that expects a reply, and the only one the conversation
    /// stops for until it gets one.
    ControlRequest {
        request_id: String,
        request: Ask,
    },
    /// The CLI answering something the *host* asked it. The other direction of
    /// the same envelope, and the reason it is read rather than ignored: an
    /// interrupt is a request this server makes, and this is the only line that
    /// says whether it was accepted.
    ControlResponse { response: Acknowledgement },
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

/// The CLI's answer to a request this server made.
///
/// Recorded rather than read off a contract:
/// `fixtures/claude-cli/11`–`14` are real acknowledgements from `claude`
/// 2.1.220, and every one of them is
/// `{"subtype": "success", "request_id": …, "response": {"still_queued": []}}`.
/// The inner `response` is deliberately not read — `still_queued` is the list of
/// turns the CLI is holding for a host that queues several, and this server
/// sends one at a time.
///
/// The failing shape is the same envelope with `"subtype": "error"` and a
/// sentence, which is what the CLI answers a control request it does not
/// understand. It has never been seen from this server, and it is read anyway:
/// a stop button that quietly did nothing is the one outcome worse than a stop
/// button that reports it failed.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Acknowledgement {
    /// The id this server minted for the request being answered. Defaulted
    /// rather than required so a malformed answer is still an answer — the
    /// alternative is the whole line failing to parse and reading as drift.
    #[serde(default)]
    pub request_id: String,
    #[serde(default)]
    pub subtype: String,
    #[serde(default)]
    pub error: Option<String>,
    /// What the CLI answered *with*, for the one request whose answer is worth
    /// more than the fact that it succeeded.
    ///
    /// An interrupt's is `{"still_queued": []}` and is deliberately unread; a
    /// [`context_usage_line`]'s is the whole reading. Both land here, and which
    /// one this is comes off the shape rather than off the id — see
    /// [`Acknowledgement::reading`].
    #[serde(default)]
    pub response: Option<ContextUsage>,
}

impl Acknowledgement {
    /// Why the CLI would not do what it was asked, if it would not.
    ///
    /// Anything that is not `success` is a refusal, including a subtype this
    /// build has never seen: the question being answered is "did the turn stop",
    /// and an answer that cannot be read as yes has to be read as no.
    pub fn refusal(&self) -> Option<String> {
        if self.subtype == "success" {
            return None;
        }
        Some(match &self.error {
            Some(said) => said.clone(),
            None => format!("the agent answered '{}'", self.subtype),
        })
    }

    /// How full the window is, when this is the answer to a
    /// [`context_usage_line`].
    ///
    /// Told apart by what it carries rather than by the id it names, which is
    /// what makes this safe to ask on an envelope shared with the interrupt: an
    /// interrupt's answer has no `totalTokens` in it and no reading can be made
    /// of one, so the two cannot be confused by a server that reads the shape.
    /// Correlating on the id would say the same thing and would additionally
    /// have to be kept in step with the ids the driver mints.
    pub fn reading(&self) -> Option<TokenUsage> {
        self.response.as_ref()?.reading()
    }
}

/// The CLI's own account of how full the context window is.
///
/// The answer to `get_context_usage`, recorded in
/// `fixtures/claude-cli/19-context-usage.ndjson`. The reply carries seventeen
/// fields — a category breakdown, a grid the CLI's own `/context` draws with,
/// per-agent and per-skill token counts — and three are read, because three are
/// what the client's meter is made of.
///
/// **`isAutoCompactEnabled` is why this request exists at all.** Nothing in the
/// event stream mentions auto-compact, so it is the one part of the reading this
/// server cannot infer, and the client renders a sentence from it.
///
/// Every field is optional for the reason the rest of this module's are: the
/// shape is the CLI's rather than a contract, and a reply missing a field should
/// narrow what can be said rather than fail to parse.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ContextUsage {
    /// What the conversation is carrying — system prompt, tools, skills and
    /// messages. Not the same arithmetic as the token counts on an assistant
    /// message, and closer to the truth: this is the CLI counting its own
    /// window rather than this server adding up what an API call reported.
    #[serde(default, rename = "totalTokens")]
    pub total_tokens: Option<u64>,
    /// The window. Available here on the *first* reading of a session, which is
    /// the whole of why asking beats waiting — `modelUsage` carries it only on
    /// the `result` that ends a turn.
    #[serde(default, rename = "maxTokens")]
    pub max_tokens: Option<u64>,
    /// Whether the agent will summarise the conversation by itself when the
    /// window fills, rather than failing.
    #[serde(default, rename = "isAutoCompactEnabled")]
    pub compacts_automatically: Option<bool>,
}

impl ContextUsage {
    /// This answer as a reading of the meter.
    ///
    /// `None` when the CLI said nothing about the size of the conversation,
    /// which is the same rule the inferred path follows: a reading with no
    /// number in it would blank a meter that the counts had filled.
    ///
    /// `input_tokens` and `output_tokens` are left unset rather than zeroed.
    /// This answer is a picture of the *window*, and it has no side to it — the
    /// reply's own `apiUsage` is the session's running API total, which is a
    /// different question. The reference server drops them here too.
    ///
    /// Mirrors `normalizeClaudeContextUsageApiSnapshot` in
    /// `pingdotgg/t3code:apps/server/src/provider/Layers/ClaudeAdapter.ts`.
    pub fn reading(&self) -> Option<TokenUsage> {
        let active_tokens = self.total_tokens.filter(|total| *total > 0)?;
        Some(TokenUsage {
            used_tokens: self
                .max_tokens
                .map_or(active_tokens, |window| active_tokens.min(window)),
            total_processed_tokens: None,
            max_tokens: self.max_tokens.filter(|window| *window > 0),
            input_tokens: None,
            output_tokens: None,
            compacts_automatically: self.compacts_automatically,
        })
    }
}

/// Build the stdin line that stops the turn in flight.
///
/// The third and last thing this server ever says to the agent, beside
/// [`user_message_line`] and [`control_response_line`], and the only one that is
/// a *request* rather than a statement or a reply — so it carries an id, and the
/// CLI answers it with an [`Acknowledgement`] naming the same id.
///
/// `reason` is in the CLI's schema for this request and is deliberately not
/// sent: it is forwarded to the turn's abort signal, where tool implementations
/// branch on it, and the developer pressing stop has no reason to give beyond
/// having pressed stop.
///
/// Found in the binary rather than in documentation, the same way the permission
/// channel was — see `fixtures/claude-cli/README.md`.
pub fn interrupt_line(request_id: &str) -> String {
    serde_json::json!({
        "type": "control_request",
        "request_id": request_id,
        "request": { "subtype": "interrupt" },
    })
    .to_string()
}

/// Build the stdin line that asks the agent how full its context window is.
///
/// The fourth thing this server says to the agent, and the second that is a
/// question. It takes no arguments — the CLI describes it as "requests a
/// breakdown of current context window usage by category", and the conversation
/// it is about is the one the process is already holding.
///
/// **Asking beats inferring on three counts**, and each was a defect before this
/// existed: the reply carries `isAutoCompactEnabled`, which appears nowhere in
/// the event stream; it carries the window on the first reading of a session,
/// where `modelUsage` arrives only on the `result` that ends a turn; and it can
/// be asked at a moment of this server's choosing rather than only when the CLI
/// happens to report counts.
///
/// Found in the binary rather than in documentation, the same way the permission
/// and interrupt channels were. It answers *while a turn is running* — the
/// request is served off the control channel rather than queued behind the turn,
/// which is what makes the first-turn reading possible at all.
pub fn context_usage_line(request_id: &str) -> String {
    serde_json::json!({
        "type": "control_request",
        "request_id": request_id,
        "request": { "subtype": "get_context_usage" },
    })
    .to_string()
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
    /// than leave it unset, and laplus is one — every answer here came from a
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
    /// The agent summarised its own conversation to make room and carried on.
    /// The one `system` subtype besides `init` that a developer needs told
    /// about — see [`Compaction`].
    CompactBoundary(Box<CompactBoundaryEvent>),
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
pub struct CompactBoundaryEvent {
    #[serde(default)]
    pub compact_metadata: Compaction,
}

/// What the agent threw away, and why.
///
/// Read off the CLI's own `compact_metadata`: `trigger` is `auto` when the
/// context filled up and `manual` when somebody asked, and the two token counts
/// are the size before and after. Every field is optional because the shape is
/// the CLI's rather than a contract, and a boundary that arrived with none of
/// them is still a boundary — the developer needs telling that the agent's
/// memory of the conversation was rewritten either way.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct Compaction {
    #[serde(default)]
    pub trigger: Option<String>,
    #[serde(default)]
    pub pre_tokens: Option<u64>,
    #[serde(default)]
    pub post_tokens: Option<u64>,
}

/// The account's standing with the API, as the CLI reports it.
///
/// Read off the binary rather than off documentation, the same way the
/// permission and interrupt channels were: the event is
/// `{"type": "rate_limit_event", "rate_limit_info": {…}}`, and the field names
/// inside it are the API's response headers in the CLI's own camel case.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RateLimit {
    /// `allowed`, `allowed_warning` or `rejected`.
    ///
    /// A string rather than an enum, unlike the contract's own closed
    /// vocabularies in [`crate::settling`]: this is the *CLI's* vocabulary, and
    /// the rest of this module keeps those as strings for the same reason
    /// (`subtype`, `role`, `stop_reason`). A standing this build has never seen
    /// is then reported verbatim, which is better than a counter — the developer
    /// is told the word the agent used rather than that something moved.
    #[serde(default)]
    pub status: String,
    /// Which limit — the CLI's own `five_hour`, `seven_day` and so on.
    #[serde(default, rename = "rateLimitType")]
    pub limit: Option<String>,
    /// Unix seconds at which the limit resets.
    #[serde(default, rename = "resetsAt")]
    pub resets_at: Option<i64>,
}

impl RateLimit {
    /// Is this worth telling the developer about?
    ///
    /// The CLI emits one of these whenever its view of the account moves, which
    /// includes moving back to fine. Publishing those would put a row in the
    /// work log saying nothing is wrong, on a schedule nobody chose — so what is
    /// surfaced is the two standings that change what the developer can do, and
    /// `allowed` is counted like any other line and otherwise left alone.
    pub fn worth_reporting(&self) -> bool {
        self.status != "allowed"
    }

    /// Is the account actually being refused, rather than warned?
    pub fn rejected(&self) -> bool {
        self.status == "rejected"
    }
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
    /// This message's token counts, which are the *conversation's* — every
    /// request re-sends the whole thing, so what went into this message is what
    /// the context is carrying. Read to move the meter while the turn is still
    /// running; the `result` line corrects it at the end.
    #[serde(default)]
    pub usage: Option<UsageCounts>,
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
    /// The end of a streamed message, and the finest-grained reading of the
    /// context window this protocol offers: it arrives while the turn is still
    /// running and carries the whole count set, not just the output side.
    MessageDelta {
        #[serde(default)]
        usage: Option<UsageCounts>,
    },
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
    /// The CLI's own diagnostics for a turn that went wrong.
    /// `fixtures/claude-cli/11-interrupted-turn.ndjson` carries one.
    #[serde(default)]
    pub errors: Vec<String>,
    /// The turn's final text on a success, and on some failures the sentence
    /// saying what failed. Read only in the second case — see
    /// [`ResultEvent::complaint`].
    #[serde(default)]
    pub result: Option<String>,
    /// The turn's token counts. Read for the context meter — see
    /// [`ResultEvent::token_usage`].
    #[serde(default)]
    pub usage: Option<ResultUsage>,
    /// Per-model totals, keyed by the model as the CLI names it
    /// (`"claude-opus-5[1m]"`). The only place in the wire format that says how
    /// large the context window *is*, which is what turns a token count into a
    /// meter.
    #[serde(default, rename = "modelUsage")]
    pub model_usage: BTreeMap<String, ModelUsage>,

    // The rest of the line. Declared rather than consumed — ticket 40 asked for
    // them while the struct was open, on the argument that a field serde drops
    // silently is a field nobody can discover is there. Nothing reads these yet,
    // and because they do not reach [`ResultSummary`] the golden files cannot
    // see them either: declaring stops the silent drop, it does not buy drift
    // detection. That comes when something consumes them.
    /// Tools the developer refused, which the CLI reports even though the
    /// refusals were already seen one at a time as control responses.
    #[serde(default)]
    pub permission_denials: Vec<PermissionDenial>,
    /// How the turn ended, said outright — `completed`, `aborted_streaming`,
    /// `error`.
    ///
    /// [`crate::turn`]'s `Ending` infers the same thing from what did or did not
    /// happen. Preferring this over the inference is deliberately **not** done
    /// here: ticket 40 put the field in scope and the behaviour change out of it.
    #[serde(default)]
    pub terminal_reason: Option<String>,
    /// The API's own error standing. Every capture has it as `null`, so its type
    /// when it is *not* null has never been observed — a status number and a
    /// status string are both plausible. Left as a `Value` rather than guessed
    /// at, so whichever it turns out to be parses instead of failing the line.
    #[serde(default)]
    pub api_error_status: Option<Value>,
    #[serde(default)]
    pub fast_mode_state: Option<String>,
    /// Time to first token, and the API's own share of the turn's duration.
    /// Beside `duration_ms`, which is the whole turn including this server.
    #[serde(default)]
    pub ttft_ms: Option<u64>,
    #[serde(default)]
    pub duration_api_ms: Option<u64>,
}

/// One tool the developer would not allow.
///
/// Every field optional: this is declared rather than consumed, and a shape that
/// drifted should not cost the `result` line it rides on — the module's rule
/// that a format change has the blast radius of one block, applied to a field.
#[derive(Debug, Default, Deserialize)]
pub struct PermissionDenial {
    #[serde(default)]
    pub tool_name: Option<String>,
    #[serde(default)]
    pub tool_use_id: Option<String>,
    #[serde(default)]
    pub tool_input: Option<Value>,
}

/// The four token counts the CLI reports, in the one shape it reports them in.
///
/// Shared between the `usage` object and each of its `iterations`, because they
/// carry the same fields and are read the same way — the difference is what they
/// *mean*, which is [`ResultEvent::token_usage`]'s problem rather than this
/// struct's.
///
/// Every field is optional: the CLI omits a count it has nothing to say about,
/// and a missing count is zero rather than a parse failure.
#[derive(Debug, Default, Deserialize)]
pub struct UsageCounts {
    #[serde(default)]
    pub input_tokens: Option<u64>,
    #[serde(default)]
    pub cache_creation_input_tokens: Option<u64>,
    #[serde(default)]
    pub cache_read_input_tokens: Option<u64>,
    #[serde(default)]
    pub output_tokens: Option<u64>,
    #[serde(default)]
    pub total_tokens: Option<u64>,
}

impl UsageCounts {
    /// Everything that went *in*, cache included.
    ///
    /// The cached counts are added rather than passed over because the context
    /// window does not care how a token got there: a cache read occupies the
    /// same room as a fresh one, and leaving them out reports a window that is
    /// nearly empty on a conversation that is nearly full.
    fn input(&self) -> u64 {
        self.input_tokens.unwrap_or(0)
            + self.cache_creation_input_tokens.unwrap_or(0)
            + self.cache_read_input_tokens.unwrap_or(0)
    }

    fn output(&self) -> u64 {
        self.output_tokens.unwrap_or(0)
    }

    /// The CLI's own total when it gave one, and the sum when it did not.
    ///
    /// `None` rather than `0` for a set of counts that is entirely absent or
    /// entirely zero, so a caller can tell "nothing was reported" from "nothing
    /// was used".
    fn total(&self) -> Option<u64> {
        if let Some(total) = self.total_tokens.filter(|total| *total > 0) {
            return Some(total);
        }
        let summed = self.input() + self.output();
        (summed > 0).then_some(summed)
    }

    /// These counts as a reading of a window of `max_tokens`.
    ///
    /// `None` for counts that are absent or all zero, so a turn that ended
    /// before the API was reached reports nothing rather than blanking a meter
    /// the previous turn had filled in.
    ///
    /// `total_processed_tokens` is left unset: it is not a property of one set
    /// of counts but of how a *turn's* counts compare with the conversation's,
    /// which only [`ResultEvent::token_usage`] is in a position to say.
    fn reading(&self, max_tokens: Option<u64>) -> Option<TokenUsage> {
        let active_tokens = self.total()?;
        Some(TokenUsage {
            used_tokens: max_tokens.map_or(active_tokens, |window| active_tokens.min(window)),
            total_processed_tokens: None,
            max_tokens,
            input_tokens: Some(self.input()),
            output_tokens: Some(self.output()),
            // Nothing in the event stream says. Stamped afterwards from what the
            // CLI answered when it was asked — see [`SessionState::remembering`].
            compacts_automatically: None,
        })
    }
}

/// The `usage` object on a `result` line.
///
/// `iterations` is the part that matters and the part that is easy to miss: the
/// top level accumulates *the whole turn*, so on a turn that made ten tool calls
/// it is roughly ten times the context actually in use. The last iteration is
/// the state the conversation is actually in. See [`ResultEvent::token_usage`].
#[derive(Debug, Default, Deserialize)]
pub struct ResultUsage {
    #[serde(flatten)]
    pub counts: UsageCounts,
    #[serde(default)]
    pub iterations: Vec<UsageCounts>,
}

/// One model's entry in `modelUsage`. Only the window is read; the costs and
/// call counts beside it are the CLI's accounting rather than this server's.
#[derive(Debug, Deserialize)]
pub struct ModelUsage {
    #[serde(default, rename = "contextWindow")]
    pub context_window: Option<u64>,
}

/// How full the context window is, as the client's meter reads it.
///
/// Field names follow the contract's `ThreadTokenUsageSnapshot`
/// (`packages/contracts/src/providerRuntime.ts`) rather than the CLI's wire
/// names, because this is the shape that leaves the server — the translation
/// happens once, here, at the point where the CLI's vocabulary is still in view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TokenUsage {
    /// What the conversation is carrying now, clamped to the window.
    pub used_tokens: u64,
    /// Everything the turn processed, tool iterations included, and **only when
    /// it exceeds `used_tokens`** — otherwise it is the same number twice and
    /// the client would show a second figure that says nothing.
    pub total_processed_tokens: Option<u64>,
    /// The window itself. `None` when nothing has said — neither an answer to
    /// [`context_usage_line`] nor a `modelUsage` — which the client renders as a
    /// token count without a percentage rather than as a full bar.
    pub max_tokens: Option<u64>,
    /// The two sides of what the conversation is carrying, when the reading came
    /// from counts that had sides.
    ///
    /// `None` on a reading taken from the CLI's own answer, which describes the
    /// window rather than an API call and has no input or output to it. The
    /// client carries both through its snapshot and renders neither, so this is
    /// about not claiming a zero that nobody reported.
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    /// Whether the agent summarises the conversation by itself when the window
    /// fills. The client turns it into a sentence in the meter's tooltip.
    ///
    /// **The one field no amount of inference can fill.** It is only ever the
    /// CLI's own answer to [`context_usage_line`], carried onto later readings by
    /// [`SessionState::remembering`] so that the sentence does not blink out
    /// every time a token count moves the meter.
    pub compacts_automatically: Option<bool>,
}

impl ResultEvent {
    /// What the CLI said went wrong, when it says something did.
    ///
    /// Three sources in order of how much they tell a developer, because the CLI
    /// uses whichever fits: an `errors` array, a `result` string, and — when it
    /// gave neither — the subtype, which at least distinguishes
    /// `error_max_turns` from `error_during_execution`.
    ///
    /// Not read on a successful turn. `result` is the reply itself there, and
    /// the reply is already the transcript.
    fn complaint(&self) -> Option<String> {
        if !self.is_error {
            return None;
        }
        if !self.errors.is_empty() {
            return Some(self.errors.join("; "));
        }
        self.result
            .as_deref()
            .map(str::trim)
            .filter(|said| !said.is_empty())
            .map(str::to_string)
            .or_else(|| self.subtype.clone())
    }

    /// How full the context window is, from the counts on this line.
    ///
    /// Two numbers come out of `usage` and they are not interchangeable. The
    /// **last iteration** is the context the conversation is actually carrying,
    /// because each iteration re-sends the whole conversation and the last one
    /// therefore *is* the conversation; the **top level** is everything the turn
    /// processed, which on a turn with tool calls counts the same context over
    /// and over. The meter wants the first. The second is reported beside it
    /// only when it is the larger of the two, which is what makes it worth
    /// saying.
    ///
    /// The window comes from the largest `modelUsage[*].contextWindow`: a turn
    /// that changed model has two entries, and the conversation has to fit the
    /// one still in use.
    ///
    /// `None` when nothing was reported — a turn that failed before the API was
    /// reached has a `usage` of zeroes, and publishing that would blank a meter
    /// the previous turn had filled in.
    ///
    /// Mirrors `normalizeClaudeActiveTokenUsage` in the reference server
    /// (`pingdotgg/t3code:apps/server/src/provider/Layers/ClaudeAdapter.ts`).
    ///
    /// `known_window` is what the session has been told before now, used when
    /// this line did not say — see [`SessionState::context_window`].
    pub fn token_usage(&self, known_window: Option<u64>) -> Option<TokenUsage> {
        let usage = self.usage.as_ref()?;
        // The last iteration, because each one re-sends the whole conversation
        // and the last one therefore *is* the conversation. The top level is the
        // turn's running total, which counts the same context once per tool call.
        let active = usage.iterations.last().unwrap_or(&usage.counts);
        // `total_processed_tokens` is left for the session to stamp — see
        // [`SessionState::remembering`]. It outlives the line it arrived on.
        active.reading(self.context_window().or(known_window))
    }

    /// Everything this turn processed, tool iterations included.
    ///
    /// The top level of `usage` rather than its last iteration: this is the one
    /// place the *sum* is wanted rather than the conversation, because it is the
    /// question "what did this turn cost" instead of "how full is the window".
    pub fn total_processed(&self) -> Option<u64> {
        self.usage.as_ref().and_then(|usage| usage.counts.total())
    }

    /// The largest window any model on this turn was running with.
    ///
    /// Largest rather than first: a turn that changed model has two entries, and
    /// the conversation has to fit the one still in use.
    pub fn context_window(&self) -> Option<u64> {
        self.model_usage
            .values()
            .filter_map(|model| model.context_window)
            .filter(|window| *window > 0)
            .max()
    }
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

    /// How full the context window is, as of the last line that said anything
    /// about it — an assistant message during a turn, a `result` at the end of
    /// one.
    ///
    /// Per session rather than per turn, because the meter is about the
    /// *conversation* rather than about the turn that last moved it: a thread
    /// sitting idle between turns is still holding its context, and a reading
    /// that expired with the turn would blank the meter every time the agent
    /// stopped talking.
    pub token_usage: Option<TokenUsage>,
    /// How large the window is, remembered from the last `result` that said.
    ///
    /// Held separately from the reading above because the two arrive on
    /// different lines and only one of them repeats: `modelUsage` appears on the
    /// terminal `result` and nowhere else, while the counts that move the meter
    /// arrive on every assistant message. Without this, every mid-turn reading
    /// would be a token count with no window to measure it against, and the
    /// meter would show a percentage only at the moment each turn ended.
    ///
    /// The reference server keeps the same thing for the same reason —
    /// `lastKnownContextWindow` in `ClaudeAdapter.ts`.
    pub context_window: Option<u64>,
    /// What the last turn processed in total, remembered for the same reason the
    /// window is: it arrives only on a `result`, and the readings that move the
    /// meter in between arrive on every message.
    ///
    /// Without this the client's "Total processed" row appears for one reading
    /// at the end of a turn and vanishes again — and never appears at all after
    /// a turn that used no tools, where the total equals what the conversation
    /// is carrying. `lastKnownTotalProcessedTokens` in `ClaudeAdapter.ts`, which
    /// is likewise the *last* turn's figure carried forward rather than a sum
    /// over the session.
    pub total_processed_tokens: Option<u64>,
    /// Whether the agent compacts by itself, remembered from the last answer to
    /// [`context_usage_line`].
    ///
    /// Remembered for a reason the other two do not have. They are carried
    /// forward because they arrive rarely; this is carried forward because it
    /// arrives on a *different kind of line altogether* — the CLI's answer to a
    /// question, rather than anything it says on its own. Every reading in
    /// between is inferred from token counts, and the client reads only the
    /// newest one (`deriveLatestContextWindowSnapshot`, which does not merge), so
    /// a reading that dropped this would take the tooltip's sentence down with it
    /// until the next answer arrived.
    pub compacts_automatically: Option<bool>,

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
    /// The agent rewrote its own memory of the conversation to make room, and
    /// carried on. Nothing in `transcript` changes: this server's copy of what
    /// was said is its own, and compaction is a fact about what the *agent* can
    /// still see.
    Compacted(Compaction),
    /// The account's standing with the API changed for the worse. Reported
    /// rather than swallowed, because it is the difference between a turn that
    /// is slow and a turn that is not going to happen.
    RateLimited(RateLimit),
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
    /// The agent answered something this server asked it — today, an interrupt.
    /// Carried rather than swallowed so a *refused* request can be reported: see
    /// [`Acknowledgement::refusal`].
    Acknowledged(Acknowledgement),
    /// The agent said how full its context window is, and `token_usage` now
    /// holds what it said.
    ///
    /// Carries nothing, because there is nothing for a caller to do with it: the
    /// reading is already folded, and the driver publishes the reading rather
    /// than the event. It is a variant of its own only so that the arm which
    /// reports a *refused* request does not have to wonder whether this was one.
    Measured,
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
    /// The agent's own account of what went wrong, on a turn that did.
    ///
    /// Carried because it is the only thing in a failed `result` a developer can
    /// act on: without it the conversation says "Turn failed" and stops there,
    /// and the whole point of reporting an error rather than crashing is that
    /// the developer can decide whether to retry.
    pub error: Option<String>,
    /// How full the context window is after this turn. `None` when the CLI
    /// reported no counts — see [`ResultEvent::token_usage`].
    pub token_usage: Option<TokenUsage>,
}

/// A session's tally of what this build could not read.
///
/// Two numbers rather than one because they are two different failures with two
/// different fixes: an unrecognised event type is the CLI having grown something
/// new, and an unparseable line is a line that is not JSON at all. Copyable and
/// subtractable, so a caller can ask what *one turn* drifted rather than only
/// what the session has drifted since it started — see
/// [`crate::turn`], where the difference is what reaches the developer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Drift {
    pub unknown_events: usize,
    pub parse_errors: usize,
}

impl Drift {
    /// What has drifted since `earlier` — the counts are monotonic, so this is
    /// the difference.
    pub fn since(self, earlier: Drift) -> Drift {
        Drift {
            unknown_events: self.unknown_events.saturating_sub(earlier.unknown_events),
            parse_errors: self.parse_errors.saturating_sub(earlier.parse_errors),
        }
    }

    /// Nothing went unread.
    pub fn is_clean(self) -> bool {
        self.unknown_events == 0 && self.parse_errors == 0
    }
}

impl SessionState {
    pub fn new() -> Self {
        Self::default()
    }

    fn bump(&mut self, key: &str) {
        *self.counts.entry(key.to_string()).or_insert(0) += 1;
    }

    /// A fresh reading, with what the session already knows written onto it.
    ///
    /// Two of the three remembered figures belong here: the window is folded in
    /// earlier, when the reading is built, because it *changes the reading* —
    /// `used_tokens` is clamped to it. These two change nothing and are carried
    /// alongside, which is why they can be stamped afterwards and why every
    /// reading gets them rather than only the one that arrived with them.
    ///
    /// The total is dropped when it does not exceed what the conversation is
    /// carrying, which is the client's own rule for showing the row at all: the
    /// same number twice tells a reader nothing. Auto-compact has no such rule —
    /// it is a fact about the agent rather than about this reading, and the
    /// reading it arrived on is not more entitled to it than the next one.
    fn remembering(&self, mut reading: TokenUsage) -> TokenUsage {
        reading.total_processed_tokens = self
            .total_processed_tokens
            .filter(|total| *total > reading.used_tokens);
        reading.compacts_automatically = reading
            .compacts_automatically
            .or(self.compacts_automatically);
        reading
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
            // `transcript` is deliberately not touched here. See
            // [`Folded::Compacted`] for why.
            Event::System(SystemEvent::CompactBoundary(boundary)) => {
                self.bump("system/compact_boundary");
                Folded::Compacted(boundary.compact_metadata)
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
                StreamEvent::MessageDelta { usage } => {
                    self.bump("stream/message_delta");
                    // The earliest the meter can move. The buffered `assistant`
                    // message says the same thing later — this is the same
                    // conversation counted before the message it belongs to has
                    // finished arriving.
                    //
                    // Only a reading with an input side is taken. The Messages
                    // API is free to send a `message_delta` carrying nothing but
                    // `output_tokens`, and this CLI does send the full set; the
                    // guard is for the version that does not, because a reading
                    // built from the output side alone is not a picture of the
                    // conversation and would collapse the meter from tens of
                    // thousands of tokens to tens.
                    let window = self.context_window;
                    let reading = usage
                        .filter(|counts| counts.input() > 0)
                        .and_then(|counts| counts.reading(window))
                        .map(|reading| self.remembering(reading));
                    if reading.is_some() {
                        self.token_usage = reading;
                    }
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
                // Before `message` is moved into the transcript. A turn that
                // uses tools emits several of these, each carrying a larger
                // conversation than the last, which is what makes the meter
                // climb during the turn rather than jump at the end of it.
                let window = self.context_window;
                let reading = env
                    .message
                    .usage
                    .as_ref()
                    .and_then(|usage| usage.reading(window))
                    .map(|reading| self.remembering(reading));
                if reading.is_some() {
                    self.token_usage = reading;
                }

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
                let error = r.complaint();
                // Both remembered *before* the reading is taken, so this turn's
                // own figures reach this turn's reading and the next turn's
                // mid-turn readings have something to measure against. A
                // `result` that carried neither leaves the last known ones alone
                // rather than forgetting them — `fixtures/claude-cli/15` is one,
                // and forgetting would drop the meter to a bare token count.
                if let Some(window) = r.context_window() {
                    self.context_window = Some(window);
                }
                if let Some(total) = r.total_processed() {
                    self.total_processed_tokens = Some(total);
                }
                let token_usage = r
                    .token_usage(self.context_window)
                    .map(|reading| self.remembering(reading));
                // Kept only when this line said something. A turn that failed
                // before the API was reached reports zeroes, and letting those
                // through would blank a meter the previous turn had filled in.
                if let Some(reading) = &token_usage {
                    self.token_usage = Some(reading.clone());
                }
                self.last_result = Some(ResultSummary {
                    is_error: r.is_error,
                    stop_reason: r.stop_reason,
                    num_turns: r.num_turns,
                    duration_ms: r.duration_ms,
                    total_cost_usd: r.total_cost_usd,
                    error,
                    token_usage,
                });
                Folded::Completed
            }

            Event::RateLimitEvent { rate_limit_info } => {
                self.bump("rate_limit_event");
                match rate_limit_info {
                    // The CLI said the account is fine. True, and not something
                    // to put in front of a developer.
                    Some(limit) if !limit.worth_reporting() => Folded::Nothing,
                    Some(limit) => Folded::RateLimited(limit),
                    // The event arrived and its payload did not. Counted rather
                    // than passed over in the same silence as the healthy case:
                    // this build can then say *nothing* about the account, and
                    // `rate_limit_info` is the likeliest field in this module to
                    // move — it was read off the binary rather than recorded, so
                    // the counter is the only thing that would notice.
                    None => {
                        self.bump("rate_limit_event/UNKNOWN");
                        self.unknown_events += 1;
                        Folded::Nothing
                    }
                }
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

            // Not drift, and that is the point of recognizing it: an interrupted
            // turn produces one of these, and before ticket 14 it was counted as
            // a format change on every turn a developer stopped.
            Event::ControlResponse { response } => {
                self.bump("control_response");
                // The CLI answering how full its window is. Taken here rather
                // than handed to the driver because it is a *reading*, and every
                // other reading in this module is folded where it lands — the
                // driver publishes what the fold arrived at, and would have
                // nowhere else to put this one.
                //
                // Preferred over whatever the counts had inferred, which is the
                // precedence `completeTurn` uses in the reference server: this is
                // the CLI counting its own window, and the inferred reading is
                // this server adding up what an API call happened to report.
                if let Some(reading) = response.reading() {
                    self.bump("control_response/get_context_usage");
                    // Both before the reading is stamped, so this answer's own
                    // figures reach this answer — and so the readings inferred
                    // between here and the next answer have them too.
                    if let Some(window) = reading.max_tokens {
                        self.context_window = Some(window);
                    }
                    if let Some(compacts) = reading.compacts_automatically {
                        self.compacts_automatically = Some(compacts);
                    }
                    self.token_usage = Some(self.remembering(reading));
                    return Folded::Measured;
                }
                Folded::Acknowledged(response)
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

    /// What this session has failed to read so far.
    pub fn drift(&self) -> Drift {
        Drift {
            unknown_events: self.unknown_events,
            parse_errors: self.parse_errors,
        }
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

    // -- interrupts ------------------------------------------------------------
    //
    // The one request this server makes of the agent. Whole captured interrupts
    // are in the golden files (`11`–`14`); what lives here is the line that goes
    // out and how each shape of answer to it reads.

    /// The request the CLI's own schema declares, and nothing beside it. The
    /// optional `reason` is deliberately absent — see [`interrupt_line`].
    #[test]
    fn the_interrupt_is_the_control_request_the_cli_expects() {
        assert_eq!(
            serde_json::from_str::<Value>(&interrupt_line("interrupt-1")).unwrap(),
            json!({
                "type": "control_request",
                "request_id": "interrupt-1",
                "request": {"subtype": "interrupt"},
            })
        );
    }

    /// The acknowledgement, verbatim in shape from
    /// `fixtures/claude-cli/11-interrupted-turn.ndjson`. It names the id this
    /// server minted, which is the whole of how a driver knows the answer is to
    /// its own request rather than to somebody else's.
    #[test]
    fn an_accepted_interrupt_reads_as_an_acknowledgement_naming_the_request() {
        let mut state = SessionState::new();
        let told = state.fold_line(
            r#"{"type":"control_response","response":{"subtype":"success","request_id":"interrupt-1","response":{"still_queued":[]}}}"#,
        );

        let Folded::Acknowledged(acknowledged) = told else {
            panic!("{told:?}");
        };
        assert_eq!(acknowledged.request_id, "interrupt-1");
        assert_eq!(acknowledged.refusal(), None);
        // And it is not drift. Before this was recognized, every interrupted
        // turn reported a format change it had caused itself.
        assert_eq!(state.unknown_events, 0);
        assert_eq!(state.parse_errors, 0);
    }

    /// A refusal has to carry something a developer can read, whichever way it
    /// arrives — the CLI's own sentence when there is one, and the subtype when
    /// there is not. A stop that silently did nothing is the outcome this exists
    /// to prevent.
    #[test]
    fn a_refused_interrupt_says_why_however_little_it_was_given() {
        let mut state = SessionState::new();

        let told = state.fold_line(
            r#"{"type":"control_response","response":{"subtype":"error","request_id":"interrupt-1","error":"No active turn"}}"#,
        );
        let Folded::Acknowledged(refused) = told else {
            panic!("{told:?}");
        };
        assert_eq!(refused.refusal().as_deref(), Some("No active turn"));

        let told = state.fold_line(
            r#"{"type":"control_response","response":{"subtype":"deferred","request_id":"interrupt-1"}}"#,
        );
        let Folded::Acknowledged(unreadable) = told else {
            panic!("{told:?}");
        };
        assert!(
            unreadable.refusal().is_some_and(|why| why.contains("deferred")),
            "an answer that cannot be read as yes has to be read as no"
        );
    }

    // -- asking how full the window is ------------------------------------------
    //
    // Ticket 76. The recorded exchange is `fixtures/claude-cli/19-context-usage`
    // and the reading taken from it is in that capture's golden; what lives here
    // is the line that goes out, and the four shapes of answer that come back.

    /// The request the CLI's own schema declares. It carries no arguments —
    /// the conversation it is about is the one the process is already holding.
    #[test]
    fn the_context_question_is_the_control_request_the_cli_expects() {
        assert_eq!(
            serde_json::from_str::<Value>(&context_usage_line("context-1")).unwrap(),
            json!({
                "type": "control_request",
                "request_id": "context-1",
                "request": {"subtype": "get_context_usage"},
            })
        );
    }

    /// The three fields the meter is made of, out of the seventeen the reply
    /// carries — and the reading is the CLI's own count rather than a sum this
    /// server made of somebody else's.
    #[test]
    fn the_answer_becomes_the_reading_the_meter_shows() {
        let mut state = SessionState::new();
        let told = state.fold_line(
            r#"{"type":"control_response","response":{"subtype":"success","request_id":"context-1","response":{"totalTokens":26937,"maxTokens":200000,"isAutoCompactEnabled":true,"percentage":13,"categories":[],"gridRows":[]}}}"#,
        );

        assert!(matches!(told, Folded::Measured), "{told:?}");
        assert_eq!(
            state.token_usage,
            Some(TokenUsage {
                used_tokens: 26_937,
                total_processed_tokens: None,
                max_tokens: Some(200_000),
                // The window has no input or output side. See
                // [`ContextUsage::reading`].
                input_tokens: None,
                output_tokens: None,
                compacts_automatically: Some(true),
            })
        );
        // Remembered, so the readings inferred between here and the next answer
        // have a window to measure against and a sentence to carry.
        assert_eq!(state.context_window, Some(200_000));
        assert_eq!(state.compacts_automatically, Some(true));
        assert_eq!(state.unknown_events, 0);
    }

    /// The first turn of a session, which is the case inference cannot reach:
    /// `modelUsage` arrives only on the `result` that ends a turn, so until this
    /// request existed the opening turn drew a token count with no percentage
    /// and no bar.
    #[test]
    fn the_window_is_known_before_any_turn_has_finished() {
        let mut state = SessionState::new();
        state.fold_line(
            r#"{"type":"control_response","response":{"subtype":"success","request_id":"context-1","response":{"totalTokens":24102,"maxTokens":200000,"isAutoCompactEnabled":false}}}"#,
        );

        let reading = state.token_usage.clone().expect("a reading");
        assert_eq!(reading.max_tokens, Some(200_000));
        assert_eq!(reading.used_tokens, 24_102);
        // Said, and said to be off — which the client renders by leaving the
        // sentence out. Not the same as never having asked.
        assert_eq!(reading.compacts_automatically, Some(false));
    }

    /// What the CLI said outlives the answer it arrived on.
    ///
    /// The client reads the newest `context-window.updated` row and does not
    /// merge it with the ones before it, so a later reading that dropped this
    /// would take the tooltip's sentence down with it — every time a token count
    /// moved the meter, which is several times a turn.
    #[test]
    fn auto_compact_is_carried_onto_the_readings_after_it() {
        let mut state = SessionState::new();
        state.fold_line(
            r#"{"type":"control_response","response":{"subtype":"success","request_id":"context-1","response":{"totalTokens":24102,"maxTokens":200000,"isAutoCompactEnabled":true}}}"#,
        );
        state.fold_line(
            &json!({
                "type": "stream_event",
                "event": {
                    "type": "message_delta",
                    "usage": { "input_tokens": 30_000, "output_tokens": 40 },
                },
            })
            .to_string(),
        );

        let inferred = state.token_usage.clone().expect("a reading");
        assert_eq!(
            inferred.used_tokens, 30_040,
            "the counts still move the meter between answers"
        );
        assert_eq!(inferred.compacts_automatically, Some(true));
    }

    /// A CLI that will not answer leaves the ticket-40 meter exactly as it was.
    ///
    /// The acceptance the fallback rests on, and the one shape of answer this
    /// build may actually meet in the field: `get_context_usage` is an SDK
    /// control request, and an older `claude` answers it with an error naming a
    /// callback it has never registered.
    #[test]
    fn a_refused_question_leaves_the_inferred_meter_standing() {
        let mut state = SessionState::new();
        state.fold_line(
            &json!({
                "type": "stream_event",
                "event": {
                    "type": "message_delta",
                    "usage": { "input_tokens": 30_000, "output_tokens": 40 },
                },
            })
            .to_string(),
        );
        let inferred = state.token_usage.clone().expect("a reading");

        let told = state.fold_line(
            r#"{"type":"control_response","response":{"subtype":"error","request_id":"context-1","error":"get_context_usage is not supported in this context (onGetContextUsage callback not registered)"}}"#,
        );

        // An acknowledgement rather than a reading, which is what keeps it out of
        // the meter. The driver then ignores it: it names no interrupt this turn
        // is waiting on, so nothing is published and no error reaches the
        // conversation.
        assert!(matches!(told, Folded::Acknowledged(_)), "{told:?}");
        assert_eq!(state.token_usage, Some(inferred));
        // And it is not drift. An answer this server asked for is not a format
        // change, however unwelcome the answer.
        assert_eq!(state.unknown_events, 0);
        assert_eq!(state.parse_errors, 0);
    }

    /// An interrupt's acknowledgement travels the same envelope and must not be
    /// mistaken for a reading. `{"still_queued": []}` has no window in it, and
    /// the shape is what tells the two apart — see [`Acknowledgement::reading`].
    #[test]
    fn an_interrupts_acknowledgement_is_not_mistaken_for_a_reading() {
        let mut state = SessionState::new();
        let told = state.fold_line(
            r#"{"type":"control_response","response":{"subtype":"success","request_id":"interrupt-1","response":{"still_queued":[]}}}"#,
        );

        assert!(matches!(told, Folded::Acknowledged(_)), "{told:?}");
        assert_eq!(state.token_usage, None);
    }

    // -- long sessions and bad weather -----------------------------------------
    //
    // The three things that happen to a session rather than to a turn:
    // the agent rewriting its own memory, the account running out of room, and
    // the CLI reporting that the turn itself went wrong.

    /// Compaction is a fact about what the *agent* can still see. The
    /// transcript here is this server's own copy, so folding a boundary must
    /// leave it exactly as it was — that is the whole of "compaction does not
    /// lose the visible transcript" at this layer.
    #[test]
    fn compaction_is_reported_and_leaves_the_transcript_alone() {
        let mut state = SessionState::new();
        state.fold_line(
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"before"}]}}"#,
        );

        let told = state.fold_line(
            r#"{"type":"system","subtype":"compact_boundary","session_id":"s","compact_metadata":{"trigger":"auto","pre_tokens":154000,"post_tokens":21000}}"#,
        );

        assert_eq!(
            told,
            Folded::Compacted(Compaction {
                trigger: Some("auto".to_string()),
                pre_tokens: Some(154_000),
                post_tokens: Some(21_000),
            })
        );
        assert_eq!(state.transcript.len(), 1);
        assert_eq!(state.transcript[0].text, "before");
        // And it is not drift. Before this was recognised it fell to
        // `SystemEvent::Other`, which is silence — a developer whose agent had
        // just forgotten half the conversation was told nothing.
        assert_eq!((state.unknown_events, state.parse_errors), (0, 0));

        // A boundary carrying no metadata is still a boundary.
        assert_eq!(
            state.fold_line(r#"{"type":"system","subtype":"compact_boundary","session_id":"s"}"#),
            Folded::Compacted(Compaction::default())
        );
    }

    /// The two standings a developer can act on are reported; the one that says
    /// everything is fine is not. The CLI emits an event whenever its view
    /// moves, so surfacing all of them would put "you are not rate limited" in
    /// the work log on a schedule nobody chose.
    #[test]
    fn only_a_rate_limit_worth_acting_on_is_reported() {
        let mut state = SessionState::new();

        let warned = state.fold_line(
            r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed_warning","rateLimitType":"five_hour","resetsAt":1764547200,"utilization":0.92,"unifiedRateLimitFallbackAvailable":false,"isUsingOverage":false},"uuid":"u","session_id":"s"}"#,
        );
        let Folded::RateLimited(limit) = warned else {
            panic!("{warned:?}");
        };
        assert_eq!(limit.status, "allowed_warning");
        assert_eq!(limit.limit.as_deref(), Some("five_hour"));
        assert_eq!(limit.resets_at, Some(1_764_547_200));
        assert!(!limit.rejected(), "a warning is not a refusal");

        let refused = state.fold_line(
            r#"{"type":"rate_limit_event","rate_limit_info":{"status":"rejected","resetsAt":1764547200},"session_id":"s"}"#,
        );
        let Folded::RateLimited(limit) = refused else {
            panic!("{refused:?}");
        };
        assert!(limit.rejected());

        // And the ordinary case says nothing, without becoming drift.
        assert_eq!(
            state.fold_line(
                r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed"},"session_id":"s"}"#
            ),
            Folded::Nothing
        );
        assert_eq!((state.unknown_events, state.parse_errors), (0, 0));

        // A payload this build cannot read is *not* the same silence. Nothing is
        // published — inventing a standing would be worse — but it is counted,
        // because this shape was read off the binary rather than recorded and
        // the counter is the only thing that would notice it moving.
        assert_eq!(
            state.fold_line(r#"{"type":"rate_limit_event","session_id":"s"}"#),
            Folded::Nothing
        );
        assert_eq!((state.unknown_events, state.parse_errors), (1, 0));
    }

    /// A failed turn carries the agent's own account of the failure. Without it
    /// the conversation says "Turn failed" and stops there, which is not
    /// something a developer can act on.
    #[test]
    fn a_failed_result_carries_what_the_agent_said_went_wrong() {
        // The `errors` array, which is what a real aborted turn carries — see
        // `fixtures/claude-cli/11-interrupted-turn.ndjson`.
        let mut state = SessionState::new();
        state.fold_line(
            r#"{"type":"result","subtype":"error_during_execution","is_error":true,"errors":["upstream connect error","retry budget exhausted"]}"#,
        );
        assert_eq!(
            state.last_result.as_ref().and_then(|r| r.error.clone()),
            Some("upstream connect error; retry budget exhausted".to_string())
        );

        // The `result` string, which is what the CLI puts a sentence in.
        let mut state = SessionState::new();
        state.fold_line(
            r#"{"type":"result","subtype":"error_during_execution","is_error":true,"result":"Claude's response exceeded the output token limit."}"#,
        );
        assert_eq!(
            state.last_result.as_ref().and_then(|r| r.error.clone()),
            Some("Claude's response exceeded the output token limit.".to_string())
        );

        // Neither, which still leaves the subtype — `error_max_turns` and
        // `error_during_execution` are different problems.
        let mut state = SessionState::new();
        state.fold_line(r#"{"type":"result","subtype":"error_max_turns","is_error":true}"#);
        assert_eq!(
            state.last_result.as_ref().and_then(|r| r.error.clone()),
            Some("error_max_turns".to_string())
        );

        // And a turn that went well says nothing went wrong, even though its
        // `result` is full of text — that text is the reply, and the reply is
        // already the transcript.
        let mut state = SessionState::new();
        state.fold_line(
            r#"{"type":"result","subtype":"success","is_error":false,"result":"here is your answer"}"#,
        );
        assert_eq!(state.last_result.as_ref().and_then(|r| r.error.clone()), None);
    }

    /// Drift is subtractable, so a caller can ask what one turn failed to read
    /// rather than only what the session has failed to read since it started.
    #[test]
    fn drift_is_counted_per_session_and_readable_per_turn() {
        let mut state = SessionState::new();
        state.fold_line(r#"{"type":"telemetry_event"}"#);
        let after_the_first_turn = state.drift();
        assert_eq!(
            after_the_first_turn,
            Drift {
                unknown_events: 1,
                parse_errors: 0
            }
        );

        state.fold_line("}{");
        state.fold_line(r#"{"type":"holograph_event"}"#);

        assert_eq!(
            state.drift(),
            Drift {
                unknown_events: 2,
                parse_errors: 1
            }
        );
        assert_eq!(
            state.drift().since(after_the_first_turn),
            Drift {
                unknown_events: 1,
                parse_errors: 1
            },
            "the second turn's drift is its own, not the session's running total"
        );

        assert!(Drift::default().is_clean());
        assert!(!state.drift().is_clean());
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

    /// The reading that separates a meter from a wrong meter.
    ///
    /// The counts below are one conversation of ~26k sent twice, because the
    /// turn made a tool call. Reading the top level reports 53k of a 200k
    /// window — twice the truth, and it climbs with every tool call a turn
    /// makes, so a long turn would show the context nearly full on a
    /// conversation with plenty of room. The last iteration is the
    /// conversation.
    #[test]
    fn a_turn_with_tool_calls_measures_the_last_iteration_not_the_whole_turn() {
        let state = fold(&[&json!({
            "type": "result",
            "subtype": "success",
            "is_error": false,
            "usage": {
                "input_tokens": 6,
                "cache_creation_input_tokens": 1_183,
                "cache_read_input_tokens": 51_206,
                "output_tokens": 43,
                "iterations": [
                    { "input_tokens": 4, "cache_read_input_tokens": 25_000, "output_tokens": 20 },
                    { "input_tokens": 2, "cache_read_input_tokens": 26_210, "output_tokens": 43 },
                ],
            },
            "modelUsage": { "claude-opus-5": { "contextWindow": 200_000 } },
        })
        .to_string()]);

        assert_eq!(
            state.last_result.and_then(|result| result.token_usage),
            Some(TokenUsage {
                used_tokens: 26_255,
                total_processed_tokens: Some(52_438),
                max_tokens: Some(200_000),
                input_tokens: Some(26_212),
                output_tokens: Some(43),
                compacts_automatically: None,
            })
        );
    }

    /// A turn that ended before the API was reached reports zeroes. Publishing
    /// those would blank a meter the previous turn had filled in, so nothing is
    /// reported and the last good reading stands.
    /// `fixtures/claude-cli/11-interrupted-turn.ndjson` is a recorded one.
    #[test]
    fn a_turn_that_used_nothing_reports_no_usage_at_all() {
        let state = fold(&[&json!({
            "type": "result",
            "subtype": "success",
            "is_error": false,
            "usage": {
                "input_tokens": 0,
                "cache_creation_input_tokens": 0,
                "cache_read_input_tokens": 0,
                "output_tokens": 0,
            },
            "modelUsage": {},
        })
        .to_string()]);

        assert_eq!(state.last_result.and_then(|result| result.token_usage), None);
    }

    /// No `modelUsage` is a count without a ceiling, which is worth reporting:
    /// the client renders a token figure and no percentage. The alternative —
    /// dropping the reading — leaves the meter empty on a turn whose size is
    /// perfectly well known.
    #[test]
    fn a_turn_that_did_not_say_how_large_the_window_is_still_reports_its_size() {
        let state = fold(&[&json!({
            "type": "result",
            "subtype": "success",
            "is_error": false,
            "usage": { "input_tokens": 900, "output_tokens": 100 },
        })
        .to_string()]);

        assert_eq!(
            state.last_result.and_then(|result| result.token_usage),
            Some(TokenUsage {
                used_tokens: 1_000,
                total_processed_tokens: None,
                max_tokens: None,
                input_tokens: Some(900),
                output_tokens: Some(100),
                compacts_automatically: None,
            })
        );
    }

    /// The meter moves while the turn is still running.
    ///
    /// Each assistant message carries the conversation as it stood when that
    /// message was requested, so a turn using tools reports a larger one each
    /// time. Without this the meter would sit still for the length of a turn and
    /// jump at the end of it — which on a long turn is the whole time a
    /// developer is watching.
    #[test]
    fn each_assistant_message_moves_the_reading_before_the_turn_ends() {
        let message = |cache_read: u64, output: u64| {
            json!({
                "type": "assistant",
                "message": {
                    "role": "assistant",
                    "content": [{ "type": "text", "text": "working" }],
                    "usage": {
                        "input_tokens": 10,
                        "cache_read_input_tokens": cache_read,
                        "output_tokens": output,
                    },
                },
            })
            .to_string()
        };

        let mut state = SessionState::new();
        state.fold_line(&message(21_481, 4));
        let first = state.token_usage.clone().expect("a reading");
        state.fold_line(&message(26_075, 2));
        let second = state.token_usage.clone().expect("a reading");

        assert_eq!(first.used_tokens, 21_495);
        assert_eq!(second.used_tokens, 26_087);
        // No `modelUsage` on an assistant message, so the first turn of a session
        // has a count and no window to measure it against. The client draws a
        // token figure and no percentage until the `result` says.
        assert_eq!(first.max_tokens, None);

        // The `result` both corrects the count and supplies the window.
        state.fold_line(
            &json!({
                "type": "result",
                "subtype": "success",
                "is_error": false,
                "usage": { "input_tokens": 10, "cache_read_input_tokens": 26_075, "output_tokens": 41 },
                "modelUsage": { "claude-opus-5": { "contextWindow": 200_000 } },
            })
            .to_string(),
        );
        let settled = state.token_usage.clone().expect("a reading");
        assert_eq!(settled.used_tokens, 26_126);
        assert_eq!(settled.max_tokens, Some(200_000));

        // And the window is remembered, so the *next* turn's mid-turn readings
        // have a percentage from their first message.
        assert_eq!(state.context_window, Some(200_000));
        state.fold_line(&message(30_000, 5));
        assert_eq!(
            state.token_usage.expect("a reading").max_tokens,
            Some(200_000)
        );
    }

    /// `message_delta` is the earliest the meter can move — while the message it
    /// belongs to is still arriving, rather than once it has.
    #[test]
    fn a_streamed_message_moves_the_reading_before_the_message_is_complete() {
        let mut state = SessionState::new();
        state.fold_line(
            &json!({
                "type": "stream_event",
                "event": {
                    "type": "message_delta",
                    "delta": { "stop_reason": "end_turn" },
                    "usage": {
                        "input_tokens": 2,
                        "cache_creation_input_tokens": 6_911,
                        "cache_read_input_tokens": 20_504,
                        "output_tokens": 4,
                    },
                },
            })
            .to_string(),
        );

        assert_eq!(
            state.token_usage.expect("a reading").used_tokens,
            27_421
        );
    }

    /// A `message_delta` carrying only the output side is not a picture of the
    /// conversation. The Messages API is free to send one, and taking it would
    /// collapse the meter from tens of thousands of tokens to single digits.
    #[test]
    fn a_streamed_reading_with_no_input_side_is_not_taken() {
        let mut state = SessionState::new();
        state.fold_line(
            &json!({
                "type": "assistant",
                "message": {
                    "role": "assistant",
                    "content": [{ "type": "text", "text": "hi" }],
                    "usage": { "input_tokens": 30_000, "output_tokens": 5 },
                },
            })
            .to_string(),
        );
        state.fold_line(
            &json!({
                "type": "stream_event",
                "event": {
                    "type": "message_delta",
                    "delta": { "stop_reason": "end_turn" },
                    "usage": { "output_tokens": 9 },
                },
            })
            .to_string(),
        );

        // The full reading stands; the partial one is ignored.
        assert_eq!(
            state.token_usage.expect("a reading").used_tokens,
            30_005
        );
    }

    /// What a turn processed outlives the line it arrived on.
    ///
    /// The client draws a "Total processed" row from it, and the figure reaches
    /// this server only on a `result` — so a reading that carried only its own
    /// would show the row for one update at the end of a turn and drop it again
    /// on the next message. Upstream keeps `lastKnownTotalProcessedTokens` for
    /// exactly this, and the row is otherwise absent on a turn that used no
    /// tools, where the total equals what the conversation is carrying.
    #[test]
    fn what_a_turn_processed_is_carried_onto_the_readings_after_it() {
        let mut state = SessionState::new();
        state.fold_line(
            &json!({
                "type": "result",
                "subtype": "success",
                "is_error": false,
                "usage": {
                    "input_tokens": 10,
                    "cache_read_input_tokens": 26_000,
                    "output_tokens": 41,
                    "iterations": [
                        { "input_tokens": 10, "cache_read_input_tokens": 12_000, "output_tokens": 20 },
                        { "input_tokens": 10, "cache_read_input_tokens": 26_000, "output_tokens": 41 },
                    ],
                },
                "modelUsage": { "claude-opus-5": { "contextWindow": 200_000 } },
            })
            .to_string(),
        );
        assert_eq!(state.total_processed_tokens, Some(26_051));

        // The next turn's first message. It says nothing about totals, and the
        // row survives anyway.
        state.fold_line(
            &json!({
                "type": "assistant",
                "message": {
                    "role": "assistant",
                    "content": [{ "type": "text", "text": "on it" }],
                    "usage": { "input_tokens": 10, "cache_read_input_tokens": 3_000, "output_tokens": 5 },
                },
            })
            .to_string(),
        );
        let carried = state.token_usage.clone().expect("a reading");
        assert_eq!(carried.used_tokens, 3_015);
        assert_eq!(carried.total_processed_tokens, Some(26_051));

        // Until the conversation grows past it, at which point the row would be
        // the same number twice and the client drops it — so this server stops
        // sending it rather than leaving a figure that says nothing.
        state.fold_line(
            &json!({
                "type": "assistant",
                "message": {
                    "role": "assistant",
                    "content": [{ "type": "text", "text": "still going" }],
                    "usage": { "input_tokens": 10, "cache_read_input_tokens": 90_000, "output_tokens": 5 },
                },
            })
            .to_string(),
        );
        assert_eq!(
            state
                .token_usage
                .expect("a reading")
                .total_processed_tokens,
            None
        );
    }

    /// A `result` that carried no `modelUsage` does not make the session forget
    /// the window it already knew. `fixtures/claude-cli/15` is one, and
    /// forgetting would drop a filled meter back to a bare token count.
    #[test]
    fn a_result_without_a_window_leaves_the_known_one_alone() {
        let mut state = SessionState::new();
        state.fold_line(
            &json!({
                "type": "result",
                "subtype": "success",
                "is_error": false,
                "usage": { "input_tokens": 1_000, "output_tokens": 10 },
                "modelUsage": { "claude-opus-5": { "contextWindow": 200_000 } },
            })
            .to_string(),
        );
        state.fold_line(
            &json!({
                "type": "result",
                "subtype": "success",
                "is_error": false,
                "usage": { "input_tokens": 0, "output_tokens": 0 },
                "modelUsage": {},
            })
            .to_string(),
        );

        assert_eq!(state.context_window, Some(200_000));
        // And the reading that turn could not take is the one from before it,
        // rather than nothing.
        assert_eq!(
            state.token_usage.expect("a reading").used_tokens,
            1_010
        );
    }

    /// A turn that changed model has two `modelUsage` entries, and the
    /// conversation has to fit the window still in use.
    #[test]
    fn a_turn_that_changed_model_is_measured_against_the_larger_window() {
        let state = fold(&[&json!({
            "type": "result",
            "subtype": "success",
            "is_error": false,
            "usage": { "input_tokens": 300_000, "output_tokens": 100 },
            "modelUsage": {
                "claude-sonnet-5": { "contextWindow": 200_000 },
                "claude-opus-5[1m]": { "contextWindow": 1_000_000 },
            },
        })
        .to_string()]);

        let usage = state
            .last_result
            .and_then(|result| result.token_usage)
            .expect("a reading");
        assert_eq!(usage.max_tokens, Some(1_000_000));
        assert_eq!(usage.used_tokens, 300_100);
    }
}
