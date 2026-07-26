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

use crate::protocol::text_content;
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
                "detail": truncate(&self.request(), DETAIL),
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
    ///
    /// Trimmed, because this is for *display* — a row headed by leading whitespace
    /// says nothing, and the client trims it again anyway (`asTrimmedString`). The
    /// record is `data.input`, which is the block's input verbatim.
    fn command(&self) -> Option<&str> {
        self.input
            .get("command")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|command| !command.is_empty())
    }

    /// The tool named and what it was given, in one sentence.
    ///
    /// Upstream's `summarizeToolRequest`: the command when there is one, because
    /// that is what a developer is looking for, and the input as JSON otherwise.
    fn request(&self) -> String {
        match self.command() {
            Some(command) => format!("{}: {}", self.name, truncate(command, COMMAND)),
            None => format!("{}: {}", self.name, truncate(&self.input.to_string(), COMMAND)),
        }
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
}
