//! The work log: what the developer reads about what the agent *did*, as
//! distinct from what it said.
//!
//! A message is the conversation. Everything else a turn produces — a tool
//! invoked, its result, a pause to reason — is an activity, and the UI folds the
//! activities into the work log beside the transcript
//! (`apps/web/src/session-logic.ts`, `deriveWorkLogEntries`). This module owns
//! the translation from "the agent called `Read` on `note.txt`" to the payload
//! that log renders, and [`crate::turn`] owns *when*.
//!
//! ## The vocabulary is the client's, not this server's
//!
//! `kind`, `payload.itemType`, `payload.status`, `payload.title`,
//! `payload.detail` and `payload.data` are all read by name in
//! `session-logic.ts`. So the classification here is upstream's
//! `ClaudeAdapter.ts` (`classifyToolItemType`, `titleForTool`,
//! `summarizeToolRequest`) rather than something invented: a conversation held
//! through this server has to look like a conversation held through that one, and
//! the UI in between is the same code.
//!
//! Three places deliberately diverge, and each is a defect in what upstream shows
//! rather than a difference of taste:
//!
//! - **An invocation is announced as `tool.updated`, not `tool.started`.** The
//!   work log *drops* `tool.started` (`deriveWorkLogEntries`, first line of the
//!   loop), so upstream shows nothing at all while a tool runs — a developer
//!   watching a minute-long `Bash` sees the turn spinning and no reason for it.
//!   `tool.updated` carrying `status: "inProgress"` is the same information in the
//!   kind the log renders, and it collapses into the completed row when the result
//!   lands rather than adding a second one.
//! - **`data.toolCallId` is set.** The log pairs an invocation with its result by
//!   collapse key, and prefers `tool:<toolCallId>` when there is one
//!   (`deriveToolLifecycleCollapseKey`). Without it the key is the title and the
//!   detail, so two `Bash` calls running the same command in one turn would
//!   collapse into a single row. The id the agent already minted is the honest key.
//! - **`status` is on the completed payload.** Upstream's `tool.completed` omits
//!   it, which leaves the client defaulting the row to `completed`
//!   (`toDerivedWorkLogEntry`) and a *failed* tool call recognizable only by
//!   `toolDetailTextLooksLikeFailure` guessing from the output text. The tool said
//!   whether it failed; saying so is better than a regular expression over prose.
//!
//!   It fixes one direction and not both, and the asymmetry is worth being precise
//!   about. `workEntryIndicatesToolFailure` short-circuits on
//!   `status: "failed"`, so a real failure is now *never* missed. It does **not**
//!   short-circuit on `"completed"` — it falls through to the same prose heuristic
//!   — so a call that succeeded while printing something failure-shaped
//!   (`No files found` from a `Grep` that matched nothing) still renders with a
//!   failure affordance. That is the price of the row showing the output at all:
//!   every field the row can display is a field that heuristic reads, so the only
//!   way to avoid it is to not show the result, which is the criterion. A wrong
//!   icon on a successful step is the better failure of the two.
//!
//! ## What the row shows, and what is kept behind it
//!
//! `detail` is display: one line, truncated to the same 180 characters upstream
//! truncates to (`ProviderRuntimeIngestion.ts`, `truncateDetail`). `data` is the
//! record: the whole input and the whole output, untruncated, so the transcript
//! holds what actually happened even where the row cannot show it.
//!
//! The invocation's detail names the tool and its input; the result's detail is
//! the *output*, because that is the new information and the row it collapses into
//! already showed the input. `data.command` is set when the tool has one, so a
//! command execution's row reads `git status` rather than repeating the whole
//! `Bash: git status` sentence — `extractToolCommand` looks there first and falls
//! back to parsing the detail.
//!
//! One consequence of the result's detail being the output rather than the request:
//! upstream suppresses a plan-mode row by looking for `ExitPlanMode:` at the front
//! of the detail (`isPlanBoundaryToolActivity`). The invocation still matches that
//! and is dropped; the result no longer does, so an `ExitPlanMode` call leaves its
//! result row behind. Today that row is the only place a plan appears at all, so it
//! is more useful than not — and rendering a plan as a plan is ticket 13's, which
//! is where the suppression starts to mean something.

use serde_json::{json, Value};

use crate::protocol::{text_content, Answer, Permission};
use crate::threads::Activity;

/// How much of a detail a row shows. Upstream's `truncateDetail` limit, matched
/// so that the same tool call reads the same way through either server.
const DETAIL: usize = 180;

/// How much of a command the request summary carries before the detail limit
/// takes over. Upstream's, and it only bites for a command longer than the row
/// could show anyway.
const COMMAND: usize = 400;

/// One tool call, from the `tool_use` block that announced it.
///
/// Held by the driver until the result arrives, because the result names only the
/// call's id — what the tool *was* and what it was *given* is on this side of the
/// pair, and the completed row needs both.
#[derive(Debug, Clone)]
pub struct Call {
    /// The agent's own id for this call, and the key its result is paired by.
    pub id: String,
    pub name: String,
    /// Whatever the tool's schema says, verbatim.
    pub input: Value,
}

/// What a tool returned, from the `tool_result` block that carried it.
#[derive(Debug, Clone, Copy)]
pub struct Returned<'a> {
    /// The block's content, verbatim — a string on every capture here and
    /// permitted to be an array of blocks.
    pub content: &'a Value,
    /// The tool's own account of whether it worked, off `is_error`.
    pub failed: bool,
}

impl Call {
    /// The tool call as the work log will render it, while it is still running.
    pub fn invoked(&self, turn_id: Option<String>) -> Activity {
        Activity::tool(
            "tool.updated",
            self.title(),
            json!({
                "itemType": self.item_type(),
                "status": "inProgress",
                "title": self.title(),
                "detail": truncate(&request_line(&self.name, &self.input), DETAIL),
                "data": self.data(None),
            }),
            turn_id,
        )
    }

    /// The same call, once the tool has answered.
    ///
    /// The result's own text becomes the row's detail; an empty one is left off
    /// entirely, so the row keeps showing what the tool was asked rather than
    /// replacing it with nothing.
    pub fn returned(&self, result: Returned<'_>, turn_id: Option<String>) -> Activity {
        let output = text_content(result.content);
        let mut payload = json!({
            "itemType": self.item_type(),
            "status": if result.failed { "failed" } else { "completed" },
            "title": self.title(),
            "data": self.data(Some(result.content)),
        });
        if !output.trim().is_empty() {
            payload["detail"] = json!(truncate(&output, DETAIL));
        }

        Activity::tool("tool.completed", self.title(), payload, turn_id)
    }

    /// A call whose invocation this driver never saw.
    ///
    /// Cannot happen against a healthy CLI — the buffered `assistant` message
    /// carrying the `tool_use` always precedes the `user` message carrying its
    /// result. What it protects against is a result arriving for a turn this
    /// process did not start, where the alternative is dropping the result
    /// silently and leaving the developer with a tool that never came back.
    pub fn untracked(id: &str) -> Call {
        Call {
            id: id.to_string(),
            name: "Tool".to_string(),
            input: Value::Null,
        }
    }

    /// The record behind the row: the whole input, and the whole output once
    /// there is one.
    fn data(&self, result: Option<&Value>) -> Value {
        let mut data = json!({
            "toolCallId": self.id,
            "toolName": self.name,
            "input": self.input,
        });
        // Looked for by name in `extractToolCommand`, so a command execution's row
        // shows the command itself rather than the sentence describing it.
        if let Some(command) = self.command() {
            data["command"] = json!(command);
        }
        if let Some(result) = result {
            data["result"] = result.clone();
        }
        data
    }

    /// The shell command this call runs, if it is that kind of call.
    fn command(&self) -> Option<&str> {
        command_in(&self.input)
    }

    fn item_type(&self) -> &'static str {
        Kind::of(&self.name).item_type()
    }

    fn title(&self) -> &'static str {
        Kind::of(&self.name).title()
    }
}

/// What kind of work a tool call is.
///
/// The contract expresses this as a string and the UI reads two of them — the
/// item type it branches its icons on, and the title it heads the row with — so
/// the two have to agree. A named kind is what makes that structural: matching the
/// *string* an earlier match produced would mean a typo in either cascade fell
/// silently through to `Tool call`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Command,
    FileChange,
    Mcp,
    Subagent,
    WebSearch,
    ImageView,
    /// A tool with no category the UI has an affordance for, which for Claude Code
    /// is most of them: `Read`, `Grep`, `Glob`.
    Other,
}

impl Kind {
    /// Upstream's `classifyToolItemType`, substring rules and order included.
    ///
    /// The order is load-bearing rather than incidental: `Task` is an agent before
    /// it is anything else, and `TaskCreate` reaches the file rules on `create`
    /// before it reaches anything about tasks. Both are upstream's answers, and a
    /// port that tidied the order would quietly re-classify tools.
    fn of(tool_name: &str) -> Kind {
        let name = tool_name.to_lowercase();
        let has = |needle: &str| name.contains(needle);

        if has("agent") || name == "task" || has("subagent") || has("sub-agent") {
            Kind::Subagent
        } else if has("bash") || has("command") || has("shell") || has("terminal") {
            Kind::Command
        } else if has("edit")
            || has("write")
            || has("file")
            || has("patch")
            || has("replace")
            || has("create")
            || has("delete")
        {
            Kind::FileChange
        } else if has("mcp") {
            Kind::Mcp
        } else if has("websearch") || has("web search") {
            Kind::WebSearch
        } else if has("image") {
            Kind::ImageView
        } else {
            Kind::Other
        }
    }

    /// The contract's tool-lifecycle item type. The UI drops a row whose item type
    /// it does not recognise (`isToolLifecycleItemType`), so every one of these is
    /// a member of `TOOL_LIFECYCLE_ITEM_TYPES`.
    fn item_type(self) -> &'static str {
        match self {
            Kind::Command => "command_execution",
            Kind::FileChange => "file_change",
            Kind::Mcp => "mcp_tool_call",
            Kind::Subagent => "collab_agent_tool_call",
            Kind::WebSearch => "web_search",
            Kind::ImageView => "image_view",
            Kind::Other => "dynamic_tool_call",
        }
    }

    /// The row's heading. Upstream's `titleForTool`, which names the *kind* of
    /// work rather than the tool — the tool's own name leads the detail beside it.
    fn title(self) -> &'static str {
        match self {
            Kind::Command => "Command run",
            Kind::FileChange => "File change",
            Kind::Mcp => "MCP tool call",
            Kind::Subagent => "Subagent task",
            Kind::WebSearch => "Web search",
            Kind::ImageView => "Image view",
            Kind::Other => "Tool call",
        }
    }
}

/// The agent reasoning, as a row of its own.
///
/// `task.progress` because that is the one kind this UI renders with its
/// *thinking* affordance — `deriveWorkLogEntries` maps it to `tone: "thinking"`
/// and a robot icon, and gives it no success tick, which is exactly right for
/// something that is neither a message nor a step that can succeed. A
/// lightcode-specific kind would render as a generic info row with a tick, which
/// is precisely not distinguishing thinking from doing.
///
/// The reasoning arrives whole, when the block closes, so this row appears after
/// the thinking rather than during it. That is the honest trade and the ticket's
/// concern — that a pause not read as a hang — is answered by the session being
/// `running` throughout, which is what the UI's own active-work indicator is
/// driven by.
///
/// **The reasoning is the record and not the row**, which is the one place this
/// deliberately shows less than it could. `payload.detail` is what the work log
/// previews under a heading, and it is also what
/// `workEntryIndicatesToolFailure` runs its prose heuristic over — a list of
/// failure-shaped strings including `no such file` and `command not found`. A
/// thinking row is tool-like as far as that function is concerned
/// (`workLogEntryIsToolLike` returns true for `tone: "thinking"`), so reasoning
/// that quoted an error the agent was working through would put a failure
/// affordance on a row where nothing failed. A bare `Thinking` row with the
/// reasoning behind it in `payload.thinking` says the true thing and says nothing
/// false.
pub fn thinking(reasoning: &str, turn_id: Option<String>) -> Option<Activity> {
    let reasoning = reasoning.trim();
    if reasoning.is_empty() {
        return None;
    }

    Some(Activity::info(
        "task.progress",
        "Thinking",
        json!({
            // Read as the row's label. There is deliberately no `detail` beside
            // it; see above.
            "summary": "Thinking",
            // The record, whole, for the same reason a tool call's `data` is.
            "thinking": reasoning,
        }),
        turn_id,
    ))
}

// ---------------------------------------------------------------------------
// Permission requests
// ---------------------------------------------------------------------------
//
// The one row the developer is expected to *answer*. Everything else here is a
// record of what happened; a pending approval is a question, and the composer
// stops accepting a message until it has been answered
// (`ChatComposer.tsx`, `isComposerApprovalState`).
//
// It reaches the panel entirely through activities. `derivePendingApprovals`
// folds the thread's work log looking for [`REQUESTED`] and closes each one on
// the [`RESOLVED`] — or the [`UNANSWERABLE`] — naming the same
// [`REQUEST_ID`], so those three kinds and that one field are the whole
// contract. There is no separate pending-approvals collection on the wire and
// nothing in `OrchestrationThread` describes one.
//
// The kinds and the key are constants because they are read in two directions:
// this module writes them, and [`unanswered`] folds them back. Matching the
// string an earlier match produced is the thing [`Kind`] exists to avoid, and it
// would be no better here.

/// The agent is waiting on a decision about this request.
pub const REQUESTED: &str = "approval.requested";

/// The decision was made and sent. Closes the panel.
pub const RESOLVED: &str = "approval.resolved";

/// The decision could not be sent, because nothing was waiting for it. Also
/// closes the panel, and is the *only* thing that can close one left behind by a
/// session that died without settling — see [`unanswerable`].
pub const UNANSWERABLE: &str = "provider.approval.respond.failed";

/// The field all three are joined by, and the one the client will not fold an
/// approval without.
const REQUEST_ID: &str = "requestId";

/// The wording the client recognises as "this request is gone".
///
/// `isStalePendingRequestFailureDetail` matches a short list of phrases against
/// the activity's `detail`, lower-cased, by substring. This is one of them, and
/// it has to appear verbatim: the sentence *is* the mechanism, so a rewording
/// that read better would silently make a stale panel permanent.
const STALE: &str = "Unknown pending permission request";

/// Permission requests still waiting on an answer, oldest first.
///
/// The client's own fold (`derivePendingApprovals`), run over the same
/// activities, so the flag this server publishes and the panel the client
/// renders cannot disagree. Deriving rather than counting is also what makes it
/// survive a restart, because the activities do.
pub fn unanswered(activities: &[Activity]) -> Vec<&str> {
    let mut open: Vec<&str> = Vec::new();
    for activity in activities {
        let Some(request_id) = activity.payload.get(REQUEST_ID).and_then(Value::as_str) else {
            continue;
        };
        match activity.kind.as_str() {
            REQUESTED => open.push(request_id),
            RESOLVED | UNANSWERABLE => open.retain(|open| *open != request_id),
            _ => {}
        }
    }
    open
}

/// What the developer decided, in the vocabulary the composer's four buttons
/// send (`ComposerPendingApprovalActions.tsx`).
///
/// The wire has two behaviours with a modifier on each — see
/// [`crate::protocol::Answer`] — and this is deliberately not that: the CLI's
/// vocabulary is what `claude` accepts and this one is what the developer
/// clicked, and [`Decision::answer`] is the single place the two meet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Approve once. The tool runs and the next one like it asks again.
    Accept,
    /// Approve, and take the CLI up on its offer to stop asking.
    AcceptForSession,
    /// Refuse this tool. The turn continues.
    Decline,
    /// Refuse this tool and stop the turn.
    Cancel,
}

impl Decision {
    /// The contract's `ProviderApprovalDecision`, as the client spells it.
    ///
    /// Exact rather than case-folded: these are schema literals, and a client
    /// sending something else is sending something this server should refuse
    /// rather than round to the nearest decision — the nearest decision to an
    /// unreadable one might be the one that runs it.
    pub fn parse(decision: &str) -> Option<Decision> {
        match decision {
            "accept" => Some(Decision::Accept),
            "acceptForSession" => Some(Decision::AcceptForSession),
            "decline" => Some(Decision::Decline),
            "cancel" => Some(Decision::Cancel),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Decision::Accept => "accept",
            Decision::AcceptForSession => "acceptForSession",
            Decision::Decline => "decline",
            Decision::Cancel => "cancel",
        }
    }

    /// This decision as the agent has to be told it.
    ///
    /// Two things are worth naming. The input is handed back **unedited**:
    /// `updatedInput` is where a host that wanted to alter the call would put its
    /// alteration, and approving a call the developer did not read would be the
    /// opposite of what this ticket is for. And "for the session" is the CLI's
    /// *own* suggestions handed back rather than a rule this server composed —
    /// the CLI knows what would stop it asking about this call, and inventing a
    /// broader rule than it offered would grant latitude nobody chose.
    pub fn answer(self, request: &Permission) -> Answer {
        match self {
            Decision::Accept => Answer::Allow {
                input: request.input.clone(),
                remember: Vec::new(),
            },
            Decision::AcceptForSession => Answer::Allow {
                input: request.input.clone(),
                remember: request.suggestions.clone(),
            },
            Decision::Decline => Answer::Deny {
                message: "The developer declined this action.".to_string(),
                interrupt: false,
            },
            Decision::Cancel => Answer::Deny {
                message: "The developer cancelled the turn.".to_string(),
                interrupt: true,
            },
        }
    }

    /// Will the agent be left with a rule, rather than a one-off permission?
    ///
    /// Not simply "was the decision `acceptForSession`": a request the CLI
    /// offered no way to remember is approved *once* however the developer
    /// answered it, because [`Decision::answer`] has nothing to send. The two
    /// have to be one fact, or a row would claim a session-wide rule the agent
    /// never received and the developer would expect silence they will not get.
    fn remembers(self, request: &Permission) -> bool {
        matches!(self, Decision::AcceptForSession) && !request.suggestions.is_empty()
    }

    /// What the resolved row says happened, given what the agent was actually
    /// told.
    fn outcome(self, remembered: bool) -> &'static str {
        match self {
            Decision::Accept => "Approved",
            Decision::AcceptForSession if remembered => "Approved for this session",
            Decision::AcceptForSession => "Approved",
            Decision::Decline => "Declined",
            Decision::Cancel => "Cancelled",
        }
    }
}

/// The agent asking, as the pending-approval panel will read it.
pub fn requested(request: &Permission, turn_id: Option<String>) -> Activity {
    Activity::approval(
        REQUESTED,
        &format!("{} needs permission", request.tool_name),
        json!({
            // The three the panel reads. Without the id nothing can be answered;
            // without the kind the request is dropped from the panel outright.
            REQUEST_ID: request.request_id,
            "requestKind": request_kind(&request.tool_name),
            "detail": truncate(&summarize(request), DETAIL),
            // The call it is about, so the row and the tool row beside it are
            // visibly one piece of work. Whole, like every other `data` here.
            "data": {
                "toolName": request.tool_name,
                "toolCallId": request.tool_use_id,
                "input": request.input,
            },
        }),
        turn_id,
    )
}

/// The developer having answered, which is what closes the panel.
pub fn resolved(request: &Permission, decision: Decision, turn_id: Option<String>) -> Activity {
    Activity::approval(
        RESOLVED,
        &format!(
            "{}: {}",
            decision.outcome(decision.remembers(request)),
            request.tool_name
        ),
        json!({
            REQUEST_ID: request.request_id,
            "requestKind": request_kind(&request.tool_name),
            "decision": decision.as_str(),
        }),
        turn_id,
    )
}

/// A decision for a request nothing is waiting on.
///
/// The escape hatch, and the only one there is. A session killed without the
/// chance to settle leaves `approval.requested` rows in a *stored* work log, so
/// the panel comes back with the conversation and nothing that has already
/// happened will ever close it. This is what the developer's next click produces,
/// and the client folds it as "that request is gone" — which needs the id and the
/// [`STALE`] wording, both, or it renders as an error beside a panel that stays.
///
/// An `error` tone rather than an `approval` one, because it is this server
/// reporting that it could not do what it was asked, which is what that tone is
/// for.
pub fn unanswerable(request_id: &str) -> Activity {
    let detail = format!("{STALE} {request_id}, so the decision was not sent.");
    Activity {
        tone: "error",
        // Repeated under `detail` as well as being the summary: `detail` is the
        // field the client matches the wording against, and the one the work log
        // renders a body out of.
        payload: json!({ REQUEST_ID: request_id, "detail": detail }),
        ..Activity::info(UNANSWERABLE, &detail, Value::Null, None)
    }
}

/// A tool named and what it was given, in one sentence.
///
/// Upstream's `summarizeToolRequest`: the command when there is one, because that
/// is what a developer is looking for, and the input as JSON otherwise. A free
/// function rather than a method on [`Call`] because a permission request is the
/// same sentence about a call that has not happened yet, and building a `Call`
/// with an invented id to reach it would be a lie for the sake of a borrow.
fn request_line(tool_name: &str, input: &Value) -> String {
    match command_in(input) {
        Some(command) => format!("{tool_name}: {}", truncate(command, COMMAND)),
        None => format!("{tool_name}: {}", truncate(&input.to_string(), COMMAND)),
    }
}

/// The shell command in a tool's input, if it is that kind of input.
///
/// Trimmed, because both callers want it for *display* — a row headed by leading
/// whitespace says nothing, and the client trims it again anyway
/// (`asTrimmedString`). The record is the input, verbatim.
fn command_in(input: &Value) -> Option<&str> {
    input
        .get("command")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|command| !command.is_empty())
}

/// What the developer is being asked to allow, in one line.
///
/// The CLI's own `description` when it sent one, because it knows what it is
/// asking about — `note.txt` for a `Write` whose input is the whole file — and
/// this server only knows how to print the arguments.
fn summarize(request: &Permission) -> String {
    match request.description.as_deref().map(str::trim) {
        Some(description) if !description.is_empty() => {
            format!("{}: {description}", request.tool_name)
        }
        _ => request_line(&request.tool_name, &request.input),
    }
}

/// Which of the three kinds of approval this is.
///
/// Upstream's `classifyRequestType` (`ClaudeAdapter.ts`) collapsed onto the
/// client's own three-value mapping (`requestKindFromRequestType`), because those
/// are the only three the panel renders and it *drops a request that is none of
/// them*. So there is no fourth answer to give: upstream's `dynamic_tool_call`
/// fallback is read by the client as a command, and a command is the label that
/// overstates rather than understates what is at stake.
///
/// Note that it is not the work-log classification beside it. A `Read` is a
/// `dynamic_tool_call` as a row — the UI has no icon for it — and a *file-read*
/// as a question, because the panel labels a question by what is at stake.
fn request_kind(tool_name: &str) -> &'static str {
    if read_only(tool_name) {
        return "file-read";
    }
    match Kind::of(tool_name) {
        Kind::FileChange => "file-change",
        // A command, and so is everything upstream classifies as neither:
        // `dynamic_tool_call` is what the client's own mapping reads as one.
        _ => "command",
    }
}

/// Upstream's `isReadOnlyToolName`, substring rules included.
fn read_only(tool_name: &str) -> bool {
    let name = tool_name.to_lowercase();
    name == "read"
        || name.contains("read file")
        || name.contains("view")
        || name.contains("grep")
        || name.contains("glob")
        || name.contains("search")
}

/// `value`, shortened to `limit` with an ellipsis if it was longer.
///
/// Counted in characters rather than bytes, because a limit that could land
/// inside a multi-byte character would panic on a transcript that happened to
/// contain one.
fn truncate(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        return value.to_string();
    }
    let kept: String = value.chars().take(limit.saturating_sub(3)).collect();
    format!("{kept}...")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(name: &str, input: Value) -> Call {
        Call {
            id: "toolu_1".to_string(),
            name: name.to_string(),
            input,
        }
    }

    /// The classification is upstream's table, so it is pinned as a table —
    /// including the two entries the *order* of the rules decides, which is where
    /// a rewrite of them would go wrong.
    #[test]
    fn a_tool_is_classified_the_way_the_ui_classifies_it() {
        let kind = |name: &str| call(name, Value::Null).item_type();

        assert_eq!(kind("Bash"), "command_execution");
        assert_eq!(kind("PowerShell"), "command_execution");
        assert_eq!(kind("Edit"), "file_change");
        assert_eq!(kind("Write"), "file_change");
        assert_eq!(kind("NotebookEdit"), "file_change");
        assert_eq!(kind("mcp__github__list_issues"), "mcp_tool_call");
        assert_eq!(kind("WebSearch"), "web_search");
        assert_eq!(kind("Read"), "dynamic_tool_call");
        assert_eq!(kind("Grep"), "dynamic_tool_call");

        // Both of these hit an earlier rule than the one their name suggests, and
        // that is upstream's answer rather than an accident of this port.
        assert_eq!(
            kind("Task"),
            "collab_agent_tool_call",
            "a subagent is an agent before it is a task"
        );
        assert_eq!(
            kind("TaskCreate"),
            "file_change",
            "`create` reaches the file rules first, as it does upstream"
        );
    }

    /// The row has to name the tool and what it was given. For a command that is
    /// the command; for everything else it is the input as JSON, because a tool's
    /// input has no shape this server knows.
    #[test]
    fn an_invocation_names_the_tool_and_what_it_was_given() {
        let reading = call("Read", json!({"file_path": "note.txt"})).invoked(None);
        assert_eq!(reading.kind, "tool.updated");
        assert_eq!(reading.tone, "tool");
        assert_eq!(reading.payload["title"], "Tool call");
        assert_eq!(reading.payload["status"], "inProgress");
        assert_eq!(
            reading.payload["detail"],
            r#"Read: {"file_path":"note.txt"}"#
        );
        assert_eq!(reading.payload["data"]["toolName"], "Read");
        assert_eq!(reading.payload["data"]["input"]["file_path"], "note.txt");
        assert_eq!(reading.payload["data"]["toolCallId"], "toolu_1");

        let running = call("Bash", json!({"command": "git status"})).invoked(None);
        assert_eq!(running.payload["title"], "Command run");
        assert_eq!(running.payload["detail"], "Bash: git status");
        // Read by name, so the row shows the command rather than the sentence.
        assert_eq!(running.payload["data"]["command"], "git status");
    }

    /// The pairing key, which is what makes two calls in one turn two rows.
    #[test]
    fn an_invocation_and_its_result_carry_the_same_call_id() {
        let reading = call("Read", json!({"file_path": "note.txt"}));
        let invoked = reading.invoked(None);
        let returned = reading.returned(
            Returned {
                content: &json!("the answer is 42"),
                failed: false,
            },
            None,
        );

        assert_eq!(
            invoked.payload["data"]["toolCallId"],
            returned.payload["data"]["toolCallId"]
        );
        assert_eq!(invoked.payload["title"], returned.payload["title"]);
    }

    /// A result shows the output and says the tool worked; the output itself is
    /// kept whole beside it.
    #[test]
    fn a_result_shows_the_output_and_says_the_call_succeeded() {
        let returned = call("Read", json!({"file_path": "note.txt"})).returned(
            Returned {
                content: &json!("1\tthe answer is 42\n"),
                failed: false,
            },
            Some("turn-1".to_string()),
        );

        assert_eq!(returned.kind, "tool.completed");
        assert_eq!(returned.payload["status"], "completed");
        assert_eq!(returned.payload["detail"], "1\tthe answer is 42\n");
        assert_eq!(returned.payload["data"]["result"], "1\tthe answer is 42\n");
        assert_eq!(returned.turn_id.as_deref(), Some("turn-1"));
    }

    /// The criterion the whole failure half of this rests on: a failed call says
    /// so in a field, rather than leaving the client to recognize failure from the
    /// shape of the output text.
    #[test]
    fn a_failed_result_says_so_rather_than_leaving_it_to_be_inferred() {
        let returned = call("Read", json!({"file_path": "missing.txt"})).returned(
            Returned {
                content: &json!("File does not exist."),
                failed: true,
            },
            None,
        );

        assert_eq!(returned.payload["status"], "failed");
        assert_eq!(returned.payload["detail"], "File does not exist.");
        // Still a tool row rather than a server error: the tool failed, this
        // server did not, and the UI styles the two differently.
        assert_eq!(returned.tone, "tool");
    }

    /// A tool that returned nothing must not blank the row it collapses into: the
    /// detail is left off, so what the tool was asked keeps showing.
    #[test]
    fn a_result_with_no_output_leaves_the_row_saying_what_was_asked() {
        let returned = call("Write", json!({"file_path": "note.txt"})).returned(
            Returned {
                content: &json!("   "),
                failed: false,
            },
            None,
        );

        assert!(returned.payload.get("detail").is_none(), "{}", returned.payload);
        assert_eq!(returned.payload["status"], "completed");
    }

    /// Truncated for display, whole in the record. Both halves, because either on
    /// its own is a different feature: the first stops the work log carrying a
    /// megabyte of file, and the second is what makes the transcript a record.
    #[test]
    fn a_large_input_and_output_are_truncated_for_display_but_not_lost() {
        let huge = "x".repeat(5_000);
        let call = call("Bash", json!({"command": huge.clone()}));

        let invoked = call.invoked(None);
        let shown = invoked.payload["detail"].as_str().expect("a detail");
        assert_eq!(shown.chars().count(), DETAIL);
        assert!(shown.ends_with("..."), "{shown}");
        assert_eq!(invoked.payload["data"]["command"], huge);
        assert_eq!(invoked.payload["data"]["input"]["command"], huge);

        let returned = call.returned(
            Returned {
                content: &json!(huge.clone()),
                failed: false,
            },
            None,
        );
        let shown = returned.payload["detail"].as_str().expect("a detail");
        assert_eq!(shown.chars().count(), DETAIL);
        assert_eq!(returned.payload["data"]["result"], huge);
    }

    /// A limit landing inside a character would panic on a byte slice, and a
    /// transcript is exactly where a multi-byte character turns up.
    #[test]
    fn truncation_counts_characters_rather_than_bytes() {
        let text = "é".repeat(300);
        let shortened = truncate(&text, DETAIL);
        assert_eq!(shortened.chars().count(), DETAIL);
        assert_eq!(truncate("é", DETAIL), "é");
    }

    /// Thinking is its own kind of row rather than text in the reply, and the kind
    /// is the one the UI renders with its thinking affordance.
    #[test]
    fn thinking_is_a_row_of_its_own_in_the_kind_the_ui_renders_as_thinking() {
        let thought = thinking("read the file first", Some("turn-1".to_string()))
            .expect("reasoning becomes a row");

        assert_eq!(thought.kind, "task.progress");
        assert_eq!(thought.summary, "Thinking");
        assert_eq!(thought.payload["summary"], "Thinking");
        assert_eq!(thought.payload["thinking"], "read the file first");
        assert_eq!(thought.turn_id.as_deref(), Some("turn-1"));
    }

    /// The reasoning is the record and not the row, and this is the reason: the
    /// client runs a prose heuristic over any tool-like row's `detail` looking for
    /// failure-shaped text, and a thinking row counts as tool-like. Reasoning that
    /// quoted the error it was working through would flag a step that never ran.
    #[test]
    fn reasoning_is_kept_out_of_the_field_the_client_scans_for_failure() {
        let thought = thinking("the build says no such file, so the path is wrong", None)
            .expect("reasoning becomes a row");

        assert!(
            thought.payload.get("detail").is_none(),
            "reasoning in `detail` would be scanned for failure: {}",
            thought.payload
        );
        assert!(
            text_content(&thought.payload["thinking"]).contains("no such file"),
            "the reasoning still has to be on the record"
        );
    }

    /// An empty thinking block is not a pause worth a row. The CLI opens one with
    /// `"thinking": ""` on every reasoning block, so this is the ordinary case
    /// rather than a defensive one.
    #[test]
    fn an_empty_thinking_block_is_not_a_row() {
        assert!(thinking("", None).is_none());
        assert!(thinking("  \n ", None).is_none());
    }

    // -- permission requests --------------------------------------------------

    fn asking(tool_name: &str, input: Value) -> Permission {
        Permission {
            request_id: "req-1".to_string(),
            tool_name: tool_name.to_string(),
            input,
            tool_use_id: Some("toolu_1".to_string()),
            description: None,
            suggestions: Vec::new(),
        }
    }

    /// The three fields `derivePendingApprovals` needs, and the row is worthless
    /// without any one of them: no `requestId` and the answer names nothing, no
    /// `requestKind` and the request is *dropped from the panel entirely*, no
    /// `detail` and the developer is asked to approve something unnamed.
    #[test]
    fn a_request_carries_what_the_pending_approval_panel_reads() {
        let row = requested(
            &asking("Write", json!({"file_path": "note.txt", "content": "hello"})),
            Some("turn-1".to_string()),
        );

        assert_eq!(row.kind, "approval.requested");
        assert_eq!(row.tone, "approval");
        assert_eq!(row.payload["requestId"], "req-1");
        assert_eq!(row.payload["requestKind"], "file-change");
        assert!(
            text_content(&row.payload["detail"]).starts_with("Write: "),
            "{}",
            row.payload["detail"]
        );
        assert_eq!(row.turn_id.as_deref(), Some("turn-1"));

        // And the call it is about, so the panel and the tool row beside it are
        // visibly the same piece of work.
        assert_eq!(row.payload["data"]["toolName"], "Write");
        assert_eq!(row.payload["data"]["toolCallId"], "toolu_1");
        assert_eq!(row.payload["data"]["input"]["file_path"], "note.txt");
    }

    /// The CLI's own summary wins when it sent one. It knows what it is asking
    /// about — `note.txt` for a `Write` of a long file — and this server only
    /// knows how to print the arguments.
    #[test]
    fn the_clis_own_summary_is_preferred_to_one_derived_from_the_arguments() {
        let row = requested(
            &Permission {
                description: Some("note.txt".to_string()),
                ..asking("Write", json!({"file_path": "note.txt", "content": "hello"}))
            },
            None,
        );

        assert_eq!(row.payload["detail"], "Write: note.txt");
    }

    /// Upstream's `classifyRequestType`, which is not the tool classification: a
    /// `Read` is a *file-read* approval even though it is a `dynamic_tool_call`
    /// as a work-log row, because the panel labels the three by what is at stake.
    ///
    /// The panel drops a request whose kind is none of the three, so there is no
    /// fourth answer — anything unclassifiable is a command, which is upstream's
    /// own fallback and the most cautious label of the three.
    #[test]
    fn a_request_is_classified_by_what_is_at_stake_rather_than_by_the_tool() {
        let kind = |name: &str| {
            requested(&asking(name, Value::Null), None).payload["requestKind"]
                .as_str()
                .expect("every request is one of the three the panel renders")
                .to_string()
        };

        assert_eq!(kind("Read"), "file-read");
        assert_eq!(kind("Grep"), "file-read");
        assert_eq!(kind("Glob"), "file-read");
        assert_eq!(kind("WebSearch"), "file-read", "upstream reads `search`");
        assert_eq!(kind("Bash"), "command");
        assert_eq!(kind("PowerShell"), "command");
        assert_eq!(kind("Write"), "file-change");
        assert_eq!(kind("Edit"), "file-change");
        assert_eq!(
            kind("mcp__github__list_issues"),
            "command",
            "nothing else is renderable, and a command is the cautious label"
        );
    }

    /// Answering closes the panel: the client removes the pending approval when a
    /// resolution names its request, so the id has to be the same one.
    #[test]
    fn a_resolution_names_the_request_it_closes_and_what_was_decided() {
        let request = Permission {
            // A request the CLI offered a way to stop asking about, so
            // "for this session" is a thing that can actually be said.
            suggestions: vec![json!({"type": "setMode", "mode": "acceptEdits"})],
            ..asking("Bash", json!({"command": "rm -rf build"}))
        };

        let approved = resolved(&request, Decision::Accept, Some("turn-1".to_string()));
        assert_eq!(approved.kind, "approval.resolved");
        assert_eq!(approved.tone, "approval");
        assert_eq!(approved.payload["requestId"], "req-1");
        assert_eq!(approved.payload["decision"], "accept");
        assert_eq!(approved.summary, "Approved: Bash");

        let declined = resolved(&request, Decision::Decline, None);
        assert_eq!(declined.payload["decision"], "decline");
        assert_eq!(declined.summary, "Declined: Bash");

        assert_eq!(
            resolved(&request, Decision::AcceptForSession, None).summary,
            "Approved for this session: Bash"
        );
        assert_eq!(
            resolved(&request, Decision::Cancel, None).summary,
            "Cancelled: Bash"
        );
    }

    /// The four decisions the composer's buttons send, and nothing else. An
    /// unrecognized one is refused rather than guessed at — guessing would mean
    /// running something on the developer's code that they did not ask to run.
    #[test]
    fn only_the_decisions_the_composer_sends_are_understood() {
        assert_eq!(Decision::parse("accept"), Some(Decision::Accept));
        assert_eq!(
            Decision::parse("acceptForSession"),
            Some(Decision::AcceptForSession)
        );
        assert_eq!(Decision::parse("decline"), Some(Decision::Decline));
        assert_eq!(Decision::parse("cancel"), Some(Decision::Cancel));
        assert_eq!(Decision::parse("Accept"), None);
        assert_eq!(Decision::parse("allow"), None);
    }

    /// What each decision becomes on the wire. This is the join between the two
    /// vocabularies and the place a mistake would be worst: approving what was
    /// declined is the one failure this whole ticket exists to prevent.
    #[test]
    fn each_decision_becomes_the_answer_the_agent_is_owed() {
        let request = Permission {
            suggestions: vec![json!({"type": "setMode", "mode": "acceptEdits"})],
            ..asking("Write", json!({"file_path": "note.txt"}))
        };

        assert_eq!(
            Decision::Accept.answer(&request),
            Answer::Allow {
                input: json!({"file_path": "note.txt"}),
                remember: Vec::new(),
            }
        );
        // "Always allow this session" is the CLI's own suggestions handed back —
        // this server does not invent a rule, it agrees to the one offered.
        assert_eq!(
            Decision::AcceptForSession.answer(&request),
            Answer::Allow {
                input: json!({"file_path": "note.txt"}),
                remember: vec![json!({"type": "setMode", "mode": "acceptEdits"})],
            }
        );

        // A denial's message reaches the *model* as the tool's result, so it says
        // what happened rather than being a code.
        let Answer::Deny { message, interrupt } = Decision::Decline.answer(&request) else {
            panic!("declining is a denial");
        };
        assert!(message.contains("declined"), "{message}");
        assert!(!interrupt, "declining one tool does not stop the turn");

        let Answer::Deny { message, interrupt } = Decision::Cancel.answer(&request) else {
            panic!("cancelling is a denial too");
        };
        assert!(message.contains("cancel"), "{message}");
        assert!(interrupt, "cancelling the turn is what the button says");
    }

    /// The escape hatch, and the two things it needs to be one.
    ///
    /// `derivePendingApprovals` closes a request on a
    /// `provider.approval.respond.failed` **only** when the activity carries the
    /// request's id *and* its detail matches one of a fixed list of phrases. Miss
    /// either and the row renders as an error beside a panel that never clears —
    /// which is the whole failure this exists to prevent, and is what the first
    /// version of it did.
    #[test]
    fn a_request_nothing_is_waiting_for_is_closed_in_the_words_the_client_reads() {
        let row = unanswerable("req-1");

        assert_eq!(row.kind, UNANSWERABLE);
        assert_eq!(row.payload[REQUEST_ID], "req-1");
        assert!(
            text_content(&row.payload["detail"])
                .to_lowercase()
                .contains("unknown pending permission request"),
            "the client matches this wording by substring: {}",
            row.payload["detail"]
        );
        // The server's own fold agrees with the client's, which is the property
        // `unanswered` exists to keep.
        assert!(unanswered(&[requested(&asking("Write", Value::Null), None), row]).is_empty());
    }

    /// The fold behind `hasPendingApprovals`, which is the client's own — a
    /// request opens on being asked and closes on either kind of answer.
    #[test]
    fn a_request_is_pending_from_being_asked_until_it_is_answered() {
        let first = asking("Write", Value::Null);
        let second = Permission {
            request_id: "req-2".to_string(),
            ..asking("Bash", Value::Null)
        };

        let mut log = vec![requested(&first, None), requested(&second, None)];
        assert_eq!(unanswered(&log), vec!["req-1", "req-2"]);

        log.push(resolved(&first, Decision::Decline, None));
        assert_eq!(unanswered(&log), vec!["req-2"]);

        log.push(resolved(&second, Decision::Accept, None));
        assert!(unanswered(&log).is_empty());

        // A work log with nothing to do with approvals is not a pending one.
        assert!(unanswered(&[thinking("hmm", None).expect("a row")]).is_empty());
    }

    /// A request with nothing to remember cannot be approved for the session in
    /// any meaningful way, so it is approved once and the row says so. Silently
    /// answering "always" with "once" would leave the developer expecting not to
    /// be asked again.
    #[test]
    fn approving_for_the_session_with_nothing_to_remember_is_approving_once() {
        let request = asking("Bash", json!({"command": "ls"}));

        assert_eq!(
            Decision::AcceptForSession.answer(&request),
            Answer::Allow {
                input: json!({"command": "ls"}),
                remember: Vec::new(),
            }
        );
        assert_eq!(
            resolved(&request, Decision::AcceptForSession, None).summary,
            "Approved: Bash",
            "the row must not claim a rule the agent was never given"
        );
    }
}
