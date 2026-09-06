//! Pure bridge-v1 decoding and SDK observation -> native conversation translation.
//! No host-private Mimir types, clocks, credentials or transport live here.

use crate::session::{Decided, Driving, Finished, Settles};
use crate::settling::SessionStatus;
use crate::threads::{Activity, Change};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};

pub const VERSION: u64 = 1;
pub const MAX_FRAME: usize = 4 * 1024 * 1024;
pub const HISTORY_WARNING: &str = "Mimir event history is incomplete. The bridge session was stopped to prevent replies or questions from reaching the wrong turn. Already-observed text and queued messages were retained; unseen text, tools or child history may be missing. Retry a queued message or send again to reopen the saved conversation.";

#[derive(Debug, PartialEq)]
pub struct Frame {
    pub id: Option<String>,
    pub kind: String,
    pub data: Value,
}

/// Incremental SSE decoding, including split UTF-8 and CRLF. Cursors are opaque.
#[derive(Default)]
pub struct SseDecoder {
    buffer: Vec<u8>,
}
impl SseDecoder {
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<Frame>, String> {
        self.buffer.extend_from_slice(bytes);
        let mut frames = Vec::new();
        loop {
            let end = self
                .buffer
                .windows(2)
                .position(|w| w == b"\n\n")
                .map(|p| (p, 2))
                .into_iter()
                .chain(
                    self.buffer
                        .windows(4)
                        .position(|w| w == b"\r\n\r\n")
                        .map(|p| (p, 4)),
                )
                .min_by_key(|(p, _)| *p);
            let Some((end, delimiter)) = end else { break };
            if end > MAX_FRAME {
                return Err("Mimir SSE frame exceeded its limit".into());
            }
            let text =
                std::str::from_utf8(&self.buffer[..end]).map_err(|_| "Mimir SSE is not UTF-8")?;
            let mut id = None;
            let mut kind = String::new();
            let mut data = Vec::new();
            for line in text.lines() {
                let Some((field, value)) = line.split_once(':') else {
                    continue;
                };
                let value = value.strip_prefix(' ').unwrap_or(value);
                match field {
                    "id" if !value.contains('\0') => id = Some(value.to_string()),
                    "event" => kind = value.to_string(),
                    "data" => data.push(value),
                    _ => {}
                }
            }
            if !data.is_empty() {
                let data: Value = serde_json::from_str(&data.join("\n"))
                    .map_err(|_| "Mimir SSE contained malformed JSON")?;
                version(&data)?;
                frames.push(Frame { id, kind, data });
            }
            self.buffer.drain(..end + delimiter);
        }
        if self.buffer.len() > MAX_FRAME {
            return Err("Mimir SSE frame exceeded its limit".into());
        }
        Ok(frames)
    }
    pub fn finish(&self) -> Result<(), String> {
        if self.buffer.iter().all(u8::is_ascii_whitespace) {
            Ok(())
        } else {
            Err("Mimir SSE ended inside a frame".into())
        }
    }
}

pub fn version(value: &Value) -> Result<(), String> {
    if value.get("version").and_then(Value::as_u64) == Some(VERSION) {
        Ok(())
    } else {
        Err("Incompatible Mimir bridge protocol; install/enable a protocol-v1 org.mimir.bridge plugin explicitly.".into())
    }
}

pub fn configuration(
    model: Option<&str>,
    options: &Value,
    interaction: &str,
    runtime: &str,
) -> Result<Value, String> {
    if runtime != "full-access" {
        return Err("Mimir supports only full-access runtime policy; native questions are not tool-permission enforcement.".into());
    }
    let (provider, model) = match model {
        Some(slug) => {
            let (provider, model) = slug
                .split_once('/')
                .filter(|(p, m)| !p.is_empty() && !m.is_empty())
                .ok_or("Mimir model must be provider/model")?;
            (Some(provider), Some(model))
        }
        None => (None, None),
    };
    let reasoning = options.get("reasoning");
    if reasoning.is_some_and(|v| !v.is_string()) {
        return Err("Mimir reasoning must be a string".into());
    }
    let mode = match interaction {
        "default" => "build",
        "plan" => "plan",
        _ => return Err("Unknown Mimir interaction mode".into()),
    };
    Ok(json!({"provider":provider,"model":model,"reasoning":reasoning,"mode":mode}))
}

pub fn models(catalog: &Value) -> Result<Vec<crate::config::ProviderModel>, String> {
    let providers = catalog
        .as_array()
        .ok_or("Mimir returned a malformed model catalog")?;
    let mut result = Vec::new();
    for provider in providers {
        let id = required(provider, "id")?;
        if id.contains('/') {
            return Err("Mimir provider id cannot contain '/'".into());
        }
        for model in provider
            .get("models")
            .and_then(Value::as_array)
            .ok_or("Mimir catalog omitted models")?
        {
            // Native providers without configured models carry an empty choice.
            // It is not selectable, but must not hide other usable providers.
            if model.get("id").and_then(Value::as_str) == Some("") {
                continue;
            }
            let model_id = required(model, "id")?;
            let levels = model
                .get("reasoning_levels")
                .and_then(Value::as_array)
                .ok_or("Mimir catalog omitted reasoning levels")?;
            let options: Vec<Value> = levels
                .iter()
                .filter_map(Value::as_str)
                .map(|level| json!({"id":level,"label":level}))
                .collect();
            result.push(crate::config::ProviderModel {
                slug: format!("{id}/{model_id}"), name: required(model, "name")?.to_string(),
                sub_provider: Some(required(provider, "name")?.to_string()), is_custom: false, is_default: None,
                capabilities: Some(json!({"optionDescriptors": if options.is_empty() { vec![] } else { vec![json!({"id":"reasoning","label":"Reasoning","type":"select","options":options})] }})),
            });
        }
    }
    Ok(result)
}

pub fn required<'a>(value: &'a Value, field: &str) -> Result<&'a str, String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("Mimir response omitted {field}"))
}
fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or("")
}
fn variant(value: &Value) -> Option<(&str, &Value)> {
    let map = value.as_object()?;
    if map.len() != 1 {
        return None;
    }
    map.iter().next().map(|(k, v)| (k.as_str(), v))
}

pub fn activity(
    id: String,
    kind: &str,
    summary: &str,
    payload: Value,
    turn: Option<String>,
    at: &str,
) -> Activity {
    Activity {
        id,
        kind: kind.into(),
        summary: summary.into(),
        payload,
        turn_id: turn,
        tone: "info",
        sequence: None,
        created_at: at.into(),
    }
}

/// The canonical plan value is persisted by the native proposed-plan fold.
pub fn proposed_plan(plan: &Value, turn: Option<String>, at: &str) -> Option<Value> {
    let id = plan.get("id")?.as_str()?;
    let markdown = plan.get("markdown")?.as_str()?.trim();
    if id.is_empty() || markdown.is_empty() {
        return None;
    }
    let mut proposed = json!({"id":id,"turnId":turn,"planMarkdown":markdown,"createdAt":at,"updatedAt":at,
        "implementedAt":null,"implementationThreadId":null,"status":text(plan,"status").replace('_', "-"),
        "name":text(plan,"name"),"path":text(plan,"path"),"provider":"mimir"});
    match text(plan, "status") {
        "saved_stopped" => proposed["decision"] = json!("save-and-stop"),
        "accepted" | "implementing" | "completed" => proposed["decision"] = json!("implement"),
        _ => {}
    }
    Some(proposed)
}

#[derive(Default)]
pub(crate) struct Projection {
    pub accepted_request: Option<String>,
    root_agent: Option<String>,
    blocks: BTreeMap<String, String>,
    unfinished_root_ids: std::collections::BTreeSet<String>,
    children: HashMap<String, Option<String>>,
    pub plan: Option<Value>,
    serial: u64,
}

impl Projection {
    pub fn take_unfinished_message_ids(&mut self) -> Vec<String> {
        std::mem::take(&mut self.unfinished_root_ids)
            .into_iter()
            .collect()
    }

    pub fn snapshot(
        &mut self,
        snapshot: &Value,
        driving: Option<&mut Driving>,
        at: &str,
    ) -> Decided {
        let mut out = Decided::default();
        let turn = driving
            .as_ref()
            .and_then(|d| d.turn.as_ref())
            .map(|t| t.turn_id.clone());
        if let Some(plan) = snapshot.get("plan").filter(|p| !p.is_null()) {
            self.plan = Some(plan.clone());
            if let Some(plan) = proposed_plan(plan, turn.clone(), at) {
                out.changes.push(Change::ProposedPlan(plan));
            }
        }
        if let Some(driving) = driving {
            if let Some(request) = snapshot.get("user_request").filter(|p| !p.is_null()) {
                self.question(request, None, driving, &mut out, at);
            }
        }
        out
    }

    pub fn event(&mut self, event: &Value, driving: &mut Driving, at: &str) -> Decided {
        let mut out = Decided::default();
        let Some((kind, value)) = variant(event) else {
            return out;
        };
        if kind == "completed" {
            if self.accepted_request.as_deref() == Some(text(value, "request_id")) {
                self.complete(
                    driving,
                    &mut out,
                    value
                        .get("error")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    at,
                );
            }
            return out;
        }
        if kind == "configured" {
            out.changes.push(Change::InteractionModeSet {
                interaction_mode: if text(value, "mode") == "plan" {
                    "plan"
                } else {
                    "default"
                }
                .into(),
            });
            return out;
        }
        if kind == "display" {
            if let Some(message) = value.as_str() {
                if let Some(turn) = &driving.turn {
                    // Native command responses are actual observed text, even
                    // when bare /goal finishes without making a model call.
                    out.changes.push(Change::AssistantMessage {
                        message_id: format!("mimir-display:{}:{}", turn.turn_id, self.serial),
                        turn_id: turn.turn_id.clone(),
                        text: message.into(),
                    });
                } else {
                    out.changes.push(Change::Activity(activity(
                        format!("mimir-display-{}", self.serial),
                        "provider.display",
                        message,
                        json!({"detail":message}),
                        None,
                        at,
                    )));
                }
                self.serial += 1;
            }
            return out;
        }
        if kind != "observation" {
            return out;
        }
        let source = &value["source"];
        let agent = text(source, "agent_id");
        let parent = source.get("parent_agent_id").and_then(Value::as_str);
        if parent.is_none() && self.root_agent.is_none() {
            self.root_agent = Some(agent.to_string());
        }
        let Some((kind, data)) = variant(&value["event"]) else {
            return out;
        };
        let turn = driving.turn.as_ref().map(|t| t.turn_id.clone());
        let child = parent.map(|_| agent);
        let key = format!(
            "mimir:{}:{}:{}:{}:{}",
            text(source, "run_id"),
            agent,
            source["turn"],
            source["model_attempt"],
            data["index"]
        );
        if let Some(child) = child {
            let parent_child = parent
                .filter(|p| Some(*p) != self.root_agent.as_deref())
                .map(str::to_string);
            let first = !self.children.contains_key(child);
            self.children.insert(child.into(), parent_child.clone());
            let mut update = crate::subagents::Update::for_child(child);
            update.parent_child_id = parent_child;
            update.state = Some(crate::subagents::State::Working);
            if first {
                out.changes.push(Change::Activity(child_row(
                    child,
                    "running",
                    turn.clone(),
                    at,
                )));
            }
            match kind {
                "text_delta" => {
                    let accumulated = self.blocks.entry(key.clone()).or_default();
                    accumulated.push_str(text(data, "value"));
                    update
                        .entries
                        .push(crate::subagents::NewEntry::said(Some(key), accumulated));
                }
                "tool_started" | "tool_finished" => {
                    let row = tool_row(data, kind == "tool_finished", turn.clone(), at);
                    update.entries.push(crate::subagents::NewEntry {
                        key: Some(format!("tool:{}", text(data, "id"))),
                        kind: child_tool_kind(text(data, "name")),
                        // The subscription's work status differs from a child's
                        // compact row status: `running` fails the client decoder.
                        payload: json!({"title":row.summary,"status":row.payload["status"],"detail":text(data,"output"),"paths":tool_paths(data),"command":input(data).get("command"),"query":input(data).get("pattern")}),
                    });
                }
                "run_complete" | "run_error" => {
                    let error = kind == "run_error"
                        || !matches!(
                            data.pointer("/execution/terminal_cause")
                                .and_then(Value::as_str),
                            None | Some("completed")
                        );
                    update.outcome = Some(if error {
                        crate::subagents::Outcome::failed(Some(text(data, "message").into()))
                    } else {
                        crate::subagents::Outcome::completed(Some(message_text(&data["message"])))
                    });
                    out.changes.push(Change::Activity(child_row(
                        child,
                        if error { "failed" } else { "completed" },
                        turn.clone(),
                        at,
                    )));
                }
                "user_request_ready" => {
                    self.question(&data["request"], Some(child), driving, &mut out, at)
                }
                _ => {}
            }
            // Do not revive a concluded child on an unrelated late observation.
            if !update.entries.is_empty() || update.outcome.is_some() || first {
                out.child_streams.push(update);
            }
            return out;
        }
        match kind {
            "text_delta" => {
                if let Some(turn_id) = turn {
                    self.blocks
                        .entry(key.clone())
                        .or_default()
                        .push_str(text(data, "value"));
                    self.unfinished_root_ids.insert(key.clone());
                    out.changes.push(Change::AssistantDelta {
                        message_id: key,
                        turn_id,
                        text: text(data, "value").into(),
                    });
                }
            }
            "content_block_stop" => {
                self.unfinished_root_ids.remove(&key);
                if let (Some(turn_id), Some(content)) = (turn, self.blocks.remove(&key)) {
                    out.changes.push(Change::AssistantMessage {
                        message_id: key,
                        turn_id,
                        text: content,
                    });
                }
            }
            "thinking_delta" if text(data, "kind") == "summary" => {
                out.changes.push(Change::Activity(activity(
                    key,
                    "thinking",
                    "Thinking",
                    json!({"detail":text(data,"value")}),
                    turn,
                    at,
                )));
            }
            "tool_started" | "tool_finished" => {
                let finished = kind == "tool_finished";
                if text(data, "name") == "update_status" && !finished {
                    // Wait for native validation before replacing the task list.
                    return out;
                }
                let row = status_row(data, turn.clone(), at)
                    .unwrap_or_else(|| tool_row(data, finished, turn, at));
                out.changes.push(Change::Activity(row));
            }
            "user_request_ready" => self.question(&data["request"], None, driving, &mut out, at),
            "context_snapshot" => {
                // The native composer meter consumes this carrier activity; it
                // is not work the agent performed and never belongs in the log.
                if let Some(used) = data["used_prompt_tokens"].as_u64() {
                    let max = data["context_window"].as_u64().filter(|max| *max > 0);
                    let usage = crate::protocol::TokenUsage {
                        used_tokens: max.map_or(used, |max| used.min(max)),
                        max_tokens: max,
                        total_processed_tokens: None,
                        input_tokens: None,
                        output_tokens: None,
                        compacts_automatically: None,
                    };
                    if let Some(usage) = driving.usage_to_report(Some(usage)) {
                        out.changes
                            .push(Change::Activity(crate::turn::context_window_row(
                                &usage, turn,
                            )));
                    }
                }
            }
            "cumulative_usage" => {
                out.changes.push(Change::Activity(activity(format!("mimir-usage-{}",self.serial),"tokens.usage","Token usage",json!({"usage":data["cumulative"],"detail":"Mimir token counts; monetary cost is unavailable."}),turn,at)));
                self.serial += 1;
            }
            "model_retry_scheduled" | "run_error" | "status" => {
                let detail = data
                    .get("message")
                    .or_else(|| data.get("reason"))
                    .or_else(|| data.get("text"))
                    .and_then(Value::as_str)
                    .unwrap_or(kind);
                out.changes.push(Change::Activity(activity(
                    format!("mimir-notice-{}", self.serial),
                    "provider.notice",
                    detail,
                    json!({"detail":detail}),
                    turn,
                    at,
                )));
                self.serial += 1;
            }
            // A root run_complete is not the SDK request-completed boundary.
            // In particular a child's completion cannot settle its parent turn.
            _ => {}
        }
        out
    }

    fn question(
        &mut self,
        request: &Value,
        child: Option<&str>,
        driving: &mut Driving,
        out: &mut Decided,
        at: &str,
    ) {
        let Some(id) = request.get("id").and_then(Value::as_str) else {
            return;
        };
        if driving.outstanding.contains_key(id) {
            return;
        }
        let questions: Vec<Value> = request.get("questions").and_then(Value::as_array).into_iter().flatten().map(|q| json!({
            "id":q["id"],"header":"Mimir","question":q["prompt"],"multiSelect":q["allow_multiple"],"options":q["options"]
        })).collect();
        let asked = crate::approval::ApprovalRequest {
            request_id: id.into(),
            tool_name: "AskUserQuestion".into(),
            input: json!({"questions":questions}),
            tool_use_id: None,
            description: None,
            suggestions: vec![],
            available_decisions: None,
            provider_request_id: Some(request.clone()),
            subagent: child.map(|id| crate::approval::Waiting {
                child_id: id.into(),
                name: None,
            }),
        };
        let turn = driving.turn.as_ref().map(|t| t.turn_id.clone());
        out.changes.push(Change::Activity(activity(
            format!("mimir-question:{id}"),
            "user-input.requested",
            "Mimir has a question",
            json!({"requestId":id,"questions":questions}),
            turn,
            at,
        )));
        driving.outstanding.insert(id.into(), asked);
    }

    pub fn complete(
        &mut self,
        driving: &mut Driving,
        out: &mut Decided,
        error: Option<String>,
        _at: &str,
    ) {
        self.accepted_request = None;
        let Some(turn) = driving.turn.take() else {
            return;
        };
        for (id, _) in std::mem::take(&mut driving.outstanding) {
            out.changes
                .push(Change::Activity(crate::worklog::unanswerable_user_input(
                    &id,
                )));
        }
        for id in self.take_unfinished_message_ids() {
            if let Some(text) = self.blocks.remove(&id) {
                out.changes.push(Change::AssistantMessage {
                    message_id: id,
                    turn_id: turn.turn_id.clone(),
                    text,
                });
            }
        }
        self.blocks.clear();
        // Checkpoints use the contract's ready/error vocabulary, not SDK
        // request statuses. Like the other drivers, a user-stopped turn gets
        // no checkpoint: a ready/error row would relabel its interruption.
        driving.finished = (!turn.was_stopped()).then(|| Finished {
            turn_id: turn.turn_id.clone(),
            status: if error.is_some() { "error" } else { "ready" },
        });
        let session_status = if error.is_some() && !turn.was_stopped() {
            SessionStatus::Error
        } else {
            SessionStatus::Ready
        };
        out.settles = Some(Settles {
            turn_id: Some(turn.turn_id),
            status: session_status,
            last_error: error,
        });
    }
}

fn child_row(id: &str, status: &str, turn: Option<String>, at: &str) -> Activity {
    activity(
        format!("mimir-child:{id}:{status}"),
        "tool.updated",
        "Mimir subagent",
        json!({"itemType":"subagent","status":status,"title":"Mimir subagent","data":{"childId":id,"agentId":id,"status":status,"toolCallId":crate::worklog::subagent_row_key(id)}}),
        turn,
        at,
    )
}
fn input(data: &Value) -> Value {
    serde_json::from_str(text(data, "input_json")).unwrap_or(Value::Null)
}
fn tool_paths(data: &Value) -> Vec<Value> {
    let profile = &data["output_profile"];
    let files = profile
        .pointer("/file_change/files")
        .or_else(|| profile.pointer("/file_read/files"));
    let mut paths: Vec<Value> = files
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|f| f.get("path").cloned())
        .collect();
    if paths.is_empty() {
        let input = input(data);
        if let Some(path) = input.get("path").filter(|path| path.is_string()) {
            paths.push(path.clone());
        }
        if let Some(inputs) = input.get("paths").and_then(Value::as_array) {
            paths.extend(inputs.iter().filter(|path| path.is_string()).cloned());
        }
        if let Some(files) = input.get("files").and_then(Value::as_array) {
            paths.extend(
                files
                    .iter()
                    .filter_map(|file| file.get("path").filter(|path| path.is_string()).cloned()),
            );
        }
    }
    paths
}
fn child_tool_kind(name: &str) -> crate::subagents::EntryKind {
    match name {
        "shell" => crate::subagents::EntryKind::Command,
        "read_file" => crate::subagents::EntryKind::Read,
        "create" | "edit_file" => crate::subagents::EntryKind::Edit,
        "search_contents_by_grep" | "search_paths_by_glob" => crate::subagents::EntryKind::Read,
        _ => crate::subagents::EntryKind::Tool,
    }
}
/// A successful update_status result contains the normalized, complete native
/// snapshot in details_json. Do not turn an attempted or rejected input into
/// progress, or let child scratchpads replace the root's task list.
fn status_row(data: &Value, turn: Option<String>, at: &str) -> Option<Activity> {
    if text(data, "name") != "update_status" || data["is_error"] != false {
        return None;
    }
    let snapshot: Value = serde_json::from_str(text(data, "details_json")).ok()?;
    let plan: Option<Vec<Value>> = snapshot.get("items")?.as_array()?.iter().map(|item| {
        let status = match item.get("status")?.as_str()? {
            "in_progress" => "inProgress",
            status @ ("pending" | "completed" | "failed" | "cancelled") => status,
            _ => return None,
        };
        Some(json!({"step":item.get("step")?.as_str()?,"activeForm":item.get("active_form")?.as_str()?,"status":status,"blockedBy":item.get("blocked_by")?.as_array()?}))
    }).collect();
    Some(activity(
        format!("mimir-status:{}", text(data, "id")),
        "turn.plan.updated",
        "Work status",
        json!({"plan":plan?,"explanation":snapshot["analysis"],"nextStep":snapshot["next_step"]}),
        turn,
        at,
    ))
}

fn tool_row(data: &Value, finished: bool, turn: Option<String>, at: &str) -> Activity {
    let id = text(data, "id");
    let name = text(data, "name");
    let input = input(data);
    let (item, title) = match name {
        "shell" => ("command_execution", "Run"),
        "read_file" | "read_skill" => ("dynamic_tool_call", "Read"),
        "create" => ("file_change", "Create"),
        "edit_file" => ("file_change", "Edit"),
        "search_contents_by_grep" | "search_paths_by_glob" => ("dynamic_tool_call", "Search"),
        "web_search" | "fetch_url" => ("web_search", "Web"),
        "view_image" => ("image_view", "View image"),
        "mcp" => ("mcp_tool_call", "MCP"),
        _ => ("dynamic_tool_call", name),
    };
    let status = if !finished {
        "inProgress"
    } else if data["is_error"] == true {
        "failed"
    } else {
        "completed"
    };
    let paths = tool_paths(data);
    // Finished profiles use canonical paths. Keep the compact target from the
    // call's input; retain canonical paths and full output in the expanded data.
    let input_paths = tool_paths(&json!({"input_json": data["input_json"]}));
    let display_paths = if input_paths.is_empty() {
        &paths
    } else {
        &input_paths
    };
    let path_detail = display_paths
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    let summary = data
        .pointer("/presentation/summary")
        .and_then(Value::as_str)
        .filter(|summary| !summary.trim().is_empty());
    // preview is a policy (auto/hidden), not display text. Prefer the target
    // throughout the lifecycle; keep full output in the expandable record.
    let detail = if finished && data["is_error"] == true {
        text(data, "output").to_string()
    } else if name == "shell" {
        text(&input, "command").to_string()
    } else if matches!(name, "search_contents_by_grep" | "search_paths_by_glob") {
        [text(&input, "pattern"), path_detail.as_str()]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" · ")
    } else if !path_detail.is_empty() {
        path_detail
    } else if let Some(summary) = summary {
        summary.to_string()
    } else if finished {
        text(data, "output").to_string()
    } else {
        String::new()
    };
    let changes = data.pointer("/output_profile/file_change/files");
    let mut record = json!({"toolCallId":id,"toolName":name,"input":input,"command":input["command"],"paths":paths,"query":input["pattern"],"changes":changes,"durationMs":data["duration_ms"],"outputProfile":data["output_profile"]});
    if finished {
        record["result"] = data["output"].clone();
    }
    let mut row = activity(
        format!("mimir-tool:{id}:{status}"),
        if finished {
            "tool.completed"
        } else {
            "tool.updated"
        },
        title,
        json!({"itemType":item,"status":status,"title":title,"detail":detail.chars().take(180).collect::<String>(),"data":record}),
        turn,
        at,
    );
    row.tone = "tool";
    row
}
pub fn message_text(message: &Value) -> String {
    let body = message
        .as_object()
        .and_then(|m| m.values().next())
        .unwrap_or(message);
    let content = &body["content"];
    if let Some(text) = content.get("text").and_then(Value::as_str) {
        return text.into();
    }
    content
        .get("blocks")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|b| {
            b.pointer("/text/text")
                .and_then(Value::as_str)
                .or_else(|| b.get("display_text").and_then(Value::as_str))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn answers(request: &Value, answers: &Value) -> Result<Value, String> {
    let questions = request
        .get("questions")
        .and_then(Value::as_array)
        .ok_or("Mimir question correlation is missing")?;
    let mut result = Vec::new();
    for question in questions {
        let id = required(question, "id")?;
        let answer = answers
            .get(id)
            .ok_or("Answer every Mimir question by its assigned question ID")?;
        let (selected, freeform, none) = if let Some(value) = answer.as_str() {
            (vec![], Some(value.to_string()), false)
        } else {
            let selected: Vec<String> = answer
                .get("selectedOptions")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect();
            let freeform = answer
                .get("freeformText")
                .and_then(Value::as_str)
                .map(str::to_string);
            let none = answer
                .get("noneOfAbove")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            (selected, freeform, none)
        };
        if !question["allow_multiple"].as_bool().unwrap_or(false) && selected.len() > 1 {
            return Err("Mimir question permits one selection".into());
        }
        result.push(json!({"question_id":id,"selected_options":selected,"freeform_text":freeform,"none_of_above":none}));
    }
    Ok(Value::Array(result))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mimir_sse_split_unicode_and_opaque_cursor() {
        let wire = b"id: nonce:42\r\nevent: session\r\ndata: {\"version\":1,\"event\":{\"display\":\"\xc3\xa9\"}}\r\n\r\n";
        for split in 0..wire.len() {
            let mut decoder = SseDecoder::default();
            let mut frames = decoder.push(&wire[..split]).unwrap();
            frames.extend(decoder.push(&wire[split..]).unwrap());
            assert_eq!(frames.len(), 1);
            assert_eq!(frames[0].id.as_deref(), Some("nonce:42"));
            assert_eq!(frames[0].data["event"]["display"], "é");
            decoder.finish().unwrap();
        }
    }
    #[test]
    fn mimir_sse_refuses_version_truncation_and_size() {
        assert!(SseDecoder::default()
            .push(b"event: ready\ndata: {\"version\":2}\n\n")
            .is_err());
        let mut decoder = SseDecoder::default();
        decoder.push(b"data: {").unwrap();
        assert!(decoder.finish().is_err());
        assert!(SseDecoder::default()
            .push(&vec![b'x'; MAX_FRAME + 1])
            .is_err());
    }
    #[test]
    fn mimir_configuration_keeps_model_slashes_and_reasoning() {
        assert_eq!(
            configuration(
                Some("local/org/model"),
                &json!({"reasoning":"high"}),
                "plan",
                "full-access"
            )
            .unwrap(),
            json!({"provider":"local","model":"org/model","reasoning":"high","mode":"plan"})
        );
        assert!(configuration(Some("model"), &Value::Null, "default", "full-access").is_err());
        assert!(configuration(None, &Value::Null, "default", "approval-required").is_err());
    }
    #[test]
    fn mimir_answers_use_sdk_ids_not_question_text() {
        let request = json!({"id":"ask-1","questions":[{"id":"q-9","prompt":"Which?","allow_multiple":false}]});
        let result = answers(
            &request,
            &json!({"q-9":{"selectedOptions":["A"],"freeformText":"because"}}),
        )
        .unwrap();
        assert_eq!(result[0]["question_id"], "q-9");
        assert_eq!(result[0]["selected_options"], json!(["A"]));
        assert!(answers(&request, &json!({"Which?":"A"})).is_err());
    }
    fn driving() -> Driving {
        Driving {
            provider: crate::provider::ProviderIdentity {
                instance_id: "mimirLocal".into(),
                driver: "mimir".into(),
            },
            turn: Some(crate::session::InFlight {
                turn_id: "laplus-turn".into(),
                assistant_message_id: None,
                tools: HashMap::new(),
                stopped: None,
            }),
            outstanding: HashMap::new(),
            interrupts: 0,
            drift_reported: Default::default(),
            measurements: 0,
            retunes: 0,
            pushed: HashMap::new(),
            unmeasured: false,
            finished: None,
            reported_usage: None,
        }
    }
    #[test]
    fn mimir_checkpoint_outcomes_use_contract_status_and_never_relabel_a_user_stop() {
        for (stopped, error, expected) in [
            (false, None, Some("ready")),
            (false, Some("failed"), Some("error")),
            (true, None, None),
            (true, Some("cancelled"), None),
        ] {
            let mut projection = Projection::default();
            let mut driving = driving();
            if stopped {
                driving.turn.as_mut().unwrap().stopped = Some("stop-request".into());
            }
            projection.complete(
                &mut driving,
                &mut Decided::default(),
                error.map(str::to_string),
                "now",
            );
            assert_eq!(driving.finished.as_ref().map(|f| f.status), expected);
        }
    }

    fn observation(agent: &str, parent: Option<&str>, event: Value) -> Value {
        json!({"observation":{"sequence":1,"source":{"session_id":"sdk-session","run_id":"sdk-run","agent_id":agent,"parent_agent_id":parent,"turn":1,"model_attempt":0},"event":event}})
    }
    #[test]
    fn mimir_native_ux_context_updates_meter_without_work_log_spam() {
        let mut projection = Projection::default();
        let mut driving = driving();
        let context = observation(
            "root",
            None,
            json!({"context_snapshot": {
                "used_prompt_tokens": 32000, "context_window": 128000
            }}),
        );
        let first = projection.event(&context, &mut driving, "now");
        let Change::Activity(row) = &first.changes[0] else {
            panic!("missing meter")
        };
        assert_eq!(row.kind, "context-window.updated");
        assert_eq!(row.payload["usedTokens"], 32000);
        assert_eq!(row.payload["maxTokens"], 128000);
        assert!(row.payload["totalProcessedTokens"].is_null());
        assert!(projection
            .event(&context, &mut driving, "later")
            .changes
            .is_empty());
        let mut invalid = context.clone();
        invalid["observation"]["event"]["context_snapshot"]["used_prompt_tokens"] = Value::Null;
        assert!(projection
            .event(&invalid, &mut driving, "later")
            .changes
            .is_empty());
        let cumulative = projection.event(
            &observation(
                "root",
                None,
                json!({"cumulative_usage": {
                    "cumulative": {"input_tokens": 900000, "output_tokens": 8000}
                }}),
            ),
            &mut driving,
            "later",
        );
        assert!(!cumulative.changes.iter().any(
            |change| matches!(change, Change::Activity(row) if row.kind == "context-window.updated")
        ));
    }
    #[test]
    fn mimir_child_tool_lifecycle_uses_the_subscription_contract() {
        let mut projection = Projection::default();
        let mut driving = driving();
        let tool = json!({"id":"read-1","name":"read_file","input_json":"{\"paths\":[\"src/main.rs\"]}","output":"file contents","is_error":false});
        for (kind, expected) in [
            ("tool_started", "inProgress"),
            ("tool_finished", "completed"),
        ] {
            let result = projection.event(
                &observation("child", Some("root"), json!({kind:tool})),
                &mut driving,
                "now",
            );
            let entry = &result.child_streams[0].entries[0];
            assert_eq!(entry.payload["status"], expected);
            assert_eq!(entry.payload["paths"], json!(["src/main.rs"]));
            assert_eq!(entry.key.as_deref(), Some("tool:read-1"));
            assert!(entry.payload["command"].is_null());
            assert!(entry.payload["query"].is_null());
        }
    }

    #[test]
    fn mimir_status_projects_successful_root_snapshots_only() {
        let mut projection = Projection::default();
        let mut driving = driving();
        let items: Vec<Value> = ["in_progress", "pending", "completed", "failed", "cancelled"]
            .into_iter().map(|status| json!({"step":status,"active_form":format!("Doing {status}"),"status":status,"blocked_by":if status == "pending" {vec![1]} else {vec![]}})).collect();
        let snapshot =
            json!({"analysis":"Found the cause","next_step":"Run focused checks","items":items});
        let tool = json!({"id":"status-1","name":"update_status","input_json":"{}","details_json":snapshot.to_string(),"is_error":false});
        let result = projection.event(
            &observation("root", None, json!({"tool_finished":tool})),
            &mut driving,
            "now",
        );
        let row = result
            .changes
            .iter()
            .find_map(|change| match change {
                Change::Activity(row) if row.kind == "turn.plan.updated" => Some(row),
                _ => None,
            })
            .expect("native task snapshot");
        assert_eq!(row.payload["plan"].as_array().unwrap().len(), 5);
        assert_eq!(
            row.payload["plan"][0],
            json!({"step":"in_progress","activeForm":"Doing in_progress","status":"inProgress","blockedBy":[]})
        );
        assert_eq!(row.payload["plan"][1]["blockedBy"], json!([1]));
        assert_eq!(row.payload["plan"][3]["status"], "failed");
        assert_eq!(row.payload["plan"][4]["status"], "cancelled");
        assert_eq!(row.payload["explanation"], snapshot["analysis"]);
        assert_eq!(row.payload["nextStep"], snapshot["next_step"]);
        assert_eq!(row.turn_id.as_deref(), Some("laplus-turn"));
        let mut failed = tool.clone();
        failed["is_error"] = json!(true);
        for event in [
            observation("root", None, json!({"tool_started":tool})),
            observation("child", Some("root"), json!({"tool_finished":tool})),
            observation("root", None, json!({"tool_finished":failed})),
        ] {
            assert!(!projection.event(&event, &mut driving, "later").changes.iter().any(|change| matches!(change, Change::Activity(row) if row.kind == "turn.plan.updated")));
        }
        let empty = projection.event(&observation("root", None, json!({"tool_finished":{"id":"status-2","name":"update_status","details_json":"{\"items\":[]}","is_error":false}})), &mut driving, "later");
        assert!(empty.changes.iter().any(|change| matches!(change, Change::Activity(row) if row.kind == "turn.plan.updated" && row.payload["plan"] == json!([]))));
    }

    #[test]
    fn mimir_native_ux_tools_use_shared_types_and_keep_readable_inputs_and_results() {
        let read = json!({"id":"read-1","name":"read_file","input_json":"{\"paths\":[\"src/main.rs\",\"README.md\"]}","presentation":{"preview":"auto"}});
        let started = tool_row(&read, false, Some("turn".into()), "now");
        assert_eq!(started.tone, "tool");
        assert_eq!(started.payload["itemType"], "dynamic_tool_call");
        assert_eq!(started.summary, "Read");
        assert_eq!(started.payload["detail"], "src/main.rs, README.md");
        assert_eq!(started.payload["data"]["input"]["paths"][0], "src/main.rs");
        let mut finished = read.clone();
        finished["output"] = json!("long file contents".repeat(80));
        finished["is_error"] = json!(false);
        finished["output_profile"] = json!({"file_read":{"files":[{"path":"/workspace/src/main.rs"},{"path":"/workspace/README.md"}]}});
        let ended = tool_row(&finished, true, Some("turn".into()), "later");
        assert_eq!(ended.payload["detail"], started.payload["detail"]);
        assert_eq!(ended.payload["data"]["result"], finished["output"]);
        assert_eq!(ended.payload["data"]["paths"][0], "/workspace/src/main.rs");
        assert_eq!(
            ended.payload["data"]["toolCallId"],
            started.payload["data"]["toolCallId"]
        );
        for (name, expected) in [
            ("shell", "command_execution"),
            ("create", "file_change"),
            ("edit_file", "file_change"),
            ("search_contents_by_grep", "dynamic_tool_call"),
        ] {
            let row = tool_row(
                &json!({"id":"call", "name":name,"input_json":"{}","is_error":true}),
                true,
                None,
                "now",
            );
            assert_eq!(row.payload["itemType"], expected);
            assert_eq!(row.payload["status"], "failed");
        }
    }

    #[test]
    fn mimir_only_matching_sdk_request_completes_root_and_clears_expired_questions() {
        let mut projection = Projection::default();
        projection.accepted_request = Some("root-request".into());
        let mut driving = driving();
        projection.event(
            &observation(
                "root",
                None,
                json!({"text_delta":{"index":0,"value":"Partial"}}),
            ),
            &mut driving,
            "now",
        );
        projection.event(
            &observation(
                "root",
                None,
                json!({"user_request_ready":{"request":{"id":"ask-1","questions":[]}}}),
            ),
            &mut driving,
            "now",
        );
        for event in [
            observation(
                "child",
                Some("root"),
                json!({"run_complete":{"message":{"assistant":{"content":{"text":"done"}}},"execution":{"terminal_cause":"completed"}}}),
            ),
            json!({"completed":{"request_id":"another-request","error":null}}),
        ] {
            assert!(projection
                .event(&event, &mut driving, "now")
                .settles
                .is_none());
            assert!(driving.turn.is_some());
        }
        let result = projection.event(
            &json!({"completed":{"request_id":"root-request","error":null}}),
            &mut driving,
            "now",
        );
        assert!(result.settles.is_some());
        assert!(driving.turn.is_none());
        assert!(result
            .changes
            .iter()
            .any(|c| matches!(c,Change::AssistantMessage{text,..} if text=="Partial")));
        assert!(
            driving.outstanding.is_empty(),
            "completed requests cannot leave an answerable question"
        );
    }
    #[test]
    fn mimir_nested_child_questions_keep_ownership_and_do_not_reach_root_text() {
        let mut projection = Projection::default();
        let mut driving = driving();
        projection.event(
            &observation("root", None, json!({"turn_started":{"turn":1}})),
            &mut driving,
            "now",
        );
        let result = projection.event(
            &observation(
                "grandchild",
                Some("child"),
                json!({"text_delta":{"index":0,"value":"Child evidence"}}),
            ),
            &mut driving,
            "now",
        );
        assert_eq!(
            result.child_streams[0].parent_child_id.as_deref(),
            Some("child")
        );
        assert!(!result.changes.iter().any(|c| matches!(
            c,
            Change::AssistantDelta { .. } | Change::AssistantMessage { .. }
        )));
        projection.event(
            &observation(
                "grandchild",
                Some("child"),
                json!({"user_request_ready":{"request":{"id":"nested-question","questions":[]}}}),
            ),
            &mut driving,
            "now",
        );
        assert_eq!(
            driving.outstanding["nested-question"]
                .subagent
                .as_ref()
                .unwrap()
                .child_id,
            "grandchild"
        );
    }
    #[test]
    fn mimir_catalog_uses_native_option_descriptor_and_plan_decisions_are_terminal() {
        let catalog = json!([{"id":"local","name":"Local","models":[{"id":"org/model","name":"Model","reasoning_levels":["low","high"]}]}]);
        let models = models(&catalog).unwrap();
        assert_eq!(models[0].slug, "local/org/model");
        // packages/contracts/src/model.ts: ProviderOptionChoice requires id,
        // not HTML-select's value. The actual UI decoder rejects the latter.
        assert_eq!(
            models[0].capabilities.as_ref().unwrap()["optionDescriptors"],
            json!([{"id":"reasoning","label":"Reasoning","type":"select",
                "options":[{"id":"low","label":"low"},{"id":"high","label":"high"}]}])
        );
        assert_eq!(
            models[0].capabilities.as_ref().unwrap()["optionDescriptors"][0]["id"],
            "reasoning"
        );
        for (status, decision) in [
            ("review_pending", None),
            ("saved_stopped", Some("save-and-stop")),
            ("accepted", Some("implement")),
            ("completed", Some("implement")),
        ] {
            let plan = proposed_plan(
                &json!({"id":"sdk-plan","markdown":"# Plan","status":status}),
                None,
                "now",
            )
            .unwrap();
            assert_eq!(plan["id"], "sdk-plan");
            assert_eq!(plan["decision"].as_str(), decision);
        }
    }

    #[test]
    fn mimir_catalog_skips_native_unconfigured_model_placeholders() {
        // Captured from the real SDK through integrations/mimir/tests/fixture.py.
        // Unconfigured native providers advertise an empty model placeholder.
        let catalog = json!([
            {"id":"anthropic","name":"Anthropic","models":[{"id":"","name":"","reasoning_levels":["off"]}]},
            {"id":"process-test","name":"Process Test","models":[{"id":"offline","name":"Offline","reasoning_levels":["off"]}]}
        ]);
        let available = models(&catalog).unwrap();
        assert_eq!(available.len(), 1);
        assert_eq!(available[0].slug, "process-test/offline");
        assert_eq!(available[0].name, "Offline");
        assert_eq!(available[0].sub_provider.as_deref(), Some("Process Test"));
        assert!(models(&json!([catalog[0].clone()])).unwrap().is_empty());
        assert!(
            models(&json!([{"id":"bad","name":"Bad","models":[{"name":"missing id"}]}])).is_err()
        );
    }

    #[test]
    fn mimir_structured_tools_preserve_file_change_hunks_and_retrieval_profiles() {
        let profile = json!({"file_change":{"files":[{"path":"src/lib.rs","additions":2,"deletions":1,"hunks":[{"lines":[{"kind":"add","content":"new","new_line":1}]}]}]}});
        let row = tool_row(
            &json!({"id":"tool-1","name":"edit_file","input_json":"{}","output":"Edited","is_error":false,"duration_ms":9,"output_profile":profile}),
            true,
            None,
            "now",
        );
        assert_eq!(row.payload["itemType"], "file_change");
        assert_eq!(row.payload["data"]["paths"], json!(["src/lib.rs"]));
        assert_eq!(
            row.payload["data"]["changes"][0]["hunks"][0]["lines"][0]["content"],
            "new"
        );
        let profile = json!({"retrieval":{"subject":"grep","shown":1,"total":2,"anchors":[{"read_path":"src/lib.rs","start_line":1}]}});
        let row = tool_row(
            &json!({"id":"tool-2","name":"search_contents_by_grep","input_json":"{\"pattern\":\"needle\"}","output_profile":profile}),
            true,
            None,
            "now",
        );
        assert_eq!(row.payload["itemType"], "dynamic_tool_call");
        assert_eq!(row.payload["data"]["query"], "needle");
        assert_eq!(row.payload["data"]["outputProfile"], profile);
    }
}
