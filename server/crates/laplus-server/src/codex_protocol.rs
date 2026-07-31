//! Pure types and folds for the Codex app-server JSON-RPC used by provider
//! probing and conversation turns.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::approval::ApprovalRequest as SharedApprovalRequest;
use crate::config::{AuthStatus, ProviderAuth, ProviderModel};
use crate::worklog::Decision;

/// The one handshake policy that controls both what laplus advertises and which
/// terminal event a conversation expects in return.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Capabilities {
    experimental_api: bool,
}

impl Capabilities {
    pub(crate) fn current() -> Capabilities {
        Capabilities {
            experimental_api: false,
        }
    }

    fn message(self) -> Value {
        match self.experimental_api {
            true => json!({"experimentalApi": true}),
            false => json!({}),
        }
    }

    pub(crate) fn idle_is_terminal(self) -> bool {
        self.experimental_api
    }

    #[cfg(test)]
    fn experimental() -> Capabilities {
        Capabilities {
            experimental_api: true,
        }
    }
}

/// The app-server state a conversation accumulates while messages are folded.
///
/// Provider probing decodes individual responses below. A conversation is a
/// stream instead: responses and notifications jointly describe one thread and
/// its turns, so this state is deliberately fresh per app-server process.
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationState {
    pub thread_id: Option<String>,
    pub turn_id: Option<String>,
    pub thread_status: Option<String>,
    pub turn_status: Option<String>,
    pub turn_error: Option<String>,
    pub assistant_messages: Vec<AssistantMessage>,
    pub reasoning_items: Vec<ReasoningItem>,
    pub command_executions: Vec<CommandExecution>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub approval_requests: Vec<ApprovalRequest>,
    pub unknown_events: usize,
    pub parse_errors: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct AssistantMessage {
    pub id: String,
    pub text: String,
    pub phase: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReasoningItem {
    pub id: String,
    pub text: String,
    pub completed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandExecution {
    pub id: String,
    pub command: String,
    pub cwd: String,
    pub process_id: Option<String>,
    pub status: String,
    pub aggregated_output: Option<String>,
    pub exit_code: Option<i64>,
    pub duration_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalRequest {
    pub request_id: String,
    pub server_request_id: Value,
    pub request_kind: String,
    pub item_id: Option<String>,
    pub tool_name: String,
    pub input: Value,
    pub available_decisions: Vec<Decision>,
}

impl ApprovalRequest {
    pub(crate) fn permission(&self) -> SharedApprovalRequest {
        SharedApprovalRequest {
            request_id: self.request_id.clone(),
            tool_name: self.tool_name.clone(),
            input: self.input.clone(),
            tool_use_id: self.item_id.clone(),
            description: None,
            suggestions: Vec::new(),
            available_decisions: Some(self.available_decisions.clone()),
            provider_request_id: Some(self.server_request_id.clone()),
        }
    }

    pub(crate) fn call(&self) -> crate::worklog::Call {
        crate::worklog::Call {
            id: self.item_id.clone().unwrap_or_else(|| self.request_id.clone()),
            name: self.tool_name.clone(),
            input: self.input.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConversationFold {
    Nothing,
    ThreadStarted { thread_id: String },
    TurnStarted { turn_id: String },
    ThreadStatus { status: String },
    ReasoningStarted { item_id: String },
    ReasoningDelta { item_id: String, text: String },
    ReasoningCompleted { item_id: String, text: String },
    AssistantDelta { item_id: String, text: String },
    AssistantCompleted { item_id: String, text: String },
    CommandStarted(CommandExecution),
    CommandCompleted(CommandExecution),
    ApprovalRequested(ApprovalRequest),
    TurnCompleted(Completion),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completion {
    pub turn_id: String,
    pub error: Option<String>,
    pub duration_ms: Option<u64>,
}

impl ConversationState {
    pub fn new() -> ConversationState {
        ConversationState::default()
    }

    pub fn fold_line(&mut self, line: &str) -> ConversationFold {
        match serde_json::from_str(line) {
            Ok(message) => self.fold_message(message),
            Err(_) => {
                self.parse_errors += 1;
                ConversationFold::Nothing
            }
        }
    }

    pub fn fold_message(&mut self, message: Value) -> ConversationFold {
        let Some(object) = message.as_object() else {
            self.parse_errors += 1;
            return ConversationFold::Nothing;
        };

        if let Some(method) = object.get("method").and_then(Value::as_str) {
            let params = object.get("params").unwrap_or(&Value::Null);
            if let Some(id) = object.get("id") {
                return self.fold_request(method, id, params);
            }
            return self.fold_notification(method, params);
        }

        let Some(result) = object.get("result") else {
            return ConversationFold::Nothing;
        };
        if let Some(thread_id) = result
            .get("thread")
            .and_then(|thread| thread.get("id"))
            .and_then(Value::as_str)
        {
            self.thread_id = Some(thread_id.to_string());
            return ConversationFold::ThreadStarted {
                thread_id: thread_id.to_string(),
            };
        }
        if let Some(turn) = result.get("turn") {
            let Some(turn_id) = turn.get("id").and_then(Value::as_str) else {
                return ConversationFold::Nothing;
            };
            self.turn_id = Some(turn_id.to_string());
            self.turn_status = turn
                .get("status")
                .and_then(Value::as_str)
                .map(str::to_string);
            self.turn_error = error_message(turn.get("error"));
            return ConversationFold::TurnStarted {
                turn_id: turn_id.to_string(),
            };
        }

        ConversationFold::Nothing
    }

    fn fold_notification(&mut self, method: &str, params: &Value) -> ConversationFold {
        match method {
            "thread/status/changed" => {
                let Some(status) = params
                    .get("status")
                    .and_then(|status| status.get("type"))
                    .and_then(Value::as_str)
                else {
                    return ConversationFold::Nothing;
                };
                self.thread_status = Some(status.to_string());
                ConversationFold::ThreadStatus {
                    status: status.to_string(),
                }
            }
            "item/started" => {
                let item = &params["item"];
                let Some(item_id) = item["id"].as_str() else {
                    return ConversationFold::Nothing;
                };
                match item["type"].as_str() {
                    Some("reasoning") => {
                        if !self.reasoning_items.iter().any(|item| item.id == item_id) {
                            self.reasoning_items.push(ReasoningItem {
                                id: item_id.to_string(),
                                text: text_in(item),
                                completed: false,
                            });
                        }
                        ConversationFold::ReasoningStarted {
                            item_id: item_id.to_string(),
                        }
                    }
                    Some("agentMessage") => {
                        self.upsert_assistant(AssistantMessage {
                            id: item_id.to_string(),
                            text: item["text"].as_str().unwrap_or_default().to_string(),
                            phase: item["phase"].as_str().map(str::to_string),
                        });
                        ConversationFold::Nothing
                    }
                    Some("commandExecution") => {
                        let Some(command) = command_execution(item) else {
                            return ConversationFold::Nothing;
                        };
                        self.upsert_command(command.clone());
                        ConversationFold::CommandStarted(command)
                    }
                    _ => ConversationFold::Nothing,
                }
            }
            "item/reasoning/textDelta" | "item/reasoning/summaryTextDelta" => {
                let Some(item_id) = params["itemId"].as_str() else {
                    return ConversationFold::Nothing;
                };
                let text = params["delta"].as_str().unwrap_or_default().to_string();
                if let Some(item) = self.reasoning_items.iter_mut().find(|item| item.id == item_id) {
                    item.text.push_str(&text);
                }
                ConversationFold::ReasoningDelta {
                    item_id: item_id.to_string(),
                    text,
                }
            }
            "item/agentMessage/delta" => {
                let item_id = params["itemId"].as_str().unwrap_or_default().to_string();
                ConversationFold::AssistantDelta {
                    item_id,
                    text: params["delta"].as_str().unwrap_or_default().to_string(),
                }
            }
            "item/completed" => {
                let item = &params["item"];
                let Some(item_id) = item["id"].as_str() else {
                    return ConversationFold::Nothing;
                };
                match item["type"].as_str() {
                    Some("agentMessage") => {
                        let message = AssistantMessage {
                            id: item_id.to_string(),
                            text: item["text"].as_str().unwrap_or_default().to_string(),
                            phase: item["phase"].as_str().map(str::to_string),
                        };
                        self.upsert_assistant(message.clone());
                        ConversationFold::AssistantCompleted {
                            item_id: item_id.to_string(),
                            text: message.text,
                        }
                    }
                    Some("reasoning") => {
                        let completed_text = text_in(item);
                        let text = match self
                            .reasoning_items
                            .iter_mut()
                            .find(|reasoning| reasoning.id == item_id)
                        {
                            Some(reasoning) => {
                                if !completed_text.is_empty() {
                                    reasoning.text = completed_text;
                                }
                                reasoning.completed = true;
                                reasoning.text.clone()
                            }
                            None => {
                                self.reasoning_items.push(ReasoningItem {
                                    id: item_id.to_string(),
                                    text: completed_text.clone(),
                                    completed: true,
                                });
                                completed_text
                            }
                        };
                        ConversationFold::ReasoningCompleted {
                            item_id: item_id.to_string(),
                            text,
                        }
                    }
                    Some("commandExecution") => {
                        let Some(command) = command_execution(item) else {
                            return ConversationFold::Nothing;
                        };
                        self.upsert_command(command.clone());
                        ConversationFold::CommandCompleted(command)
                    }
                    _ => ConversationFold::Nothing,
                }
            }
            "turn/completed" => {
                let turn = &params["turn"];
                let Some(turn_id) = turn["id"].as_str() else {
                    return ConversationFold::Nothing;
                };
                self.turn_id = Some(turn_id.to_string());
                self.turn_status = turn["status"].as_str().map(str::to_string);
                self.turn_error = error_message(turn.get("error"));
                ConversationFold::TurnCompleted(Completion {
                    turn_id: turn_id.to_string(),
                    error: self.turn_error.clone(),
                    duration_ms: turn["durationMs"].as_u64(),
                })
            }
            "thread/started"
            | "serverRequest/resolved"
            | "configWarning"
            | "remoteControl/status/changed"
            | "mcpServer/startupStatus/updated"
            | "thread/tokenUsage/updated"
            | "account/rateLimits/updated" => ConversationFold::Nothing,
            _ => {
                self.unknown_events += 1;
                ConversationFold::Nothing
            }
        }
    }

    fn fold_request(&mut self, method: &str, id: &Value, params: &Value) -> ConversationFold {
        let (request_kind, tool_name, input) = match method {
            "item/commandExecution/requestApproval" => (
                "command",
                "Command",
                json!({
                    "command": params.get("command").cloned().unwrap_or(Value::Null),
                    "cwd": params.get("cwd").cloned().unwrap_or(Value::Null),
                }),
            ),
            "item/fileRead/requestApproval" => ("file-read", "Read", params.clone()),
            "item/fileChange/requestApproval" => ("file-change", "Write", params.clone()),
            _ => {
                self.unknown_events += 1;
                return ConversationFold::Nothing;
            }
        };
        let available_decisions = params
            .get("availableDecisions")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter_map(Decision::parse)
            .collect();
        let request = ApprovalRequest {
            request_id: format!("codex:{id}"),
            server_request_id: id.clone(),
            request_kind: request_kind.to_string(),
            item_id: params.get("itemId").and_then(Value::as_str).map(str::to_string),
            tool_name: tool_name.to_string(),
            input,
            available_decisions,
        };
        self.approval_requests.push(request.clone());
        ConversationFold::ApprovalRequested(request)
    }

    fn upsert_assistant(&mut self, message: AssistantMessage) {
        match self
            .assistant_messages
            .iter_mut()
            .find(|existing| existing.id == message.id)
        {
            Some(existing) => *existing = message,
            None => self.assistant_messages.push(message),
        }
    }

    fn upsert_command(&mut self, command: CommandExecution) {
        match self
            .command_executions
            .iter_mut()
            .find(|existing| existing.id == command.id)
        {
            Some(existing) => *existing = command,
            None => self.command_executions.push(command),
        }
    }
}

fn command_execution(item: &Value) -> Option<CommandExecution> {
    Some(CommandExecution {
        id: item.get("id")?.as_str()?.to_string(),
        command: item.get("command")?.as_str()?.to_string(),
        cwd: item.get("cwd")?.as_str()?.to_string(),
        process_id: item.get("processId").and_then(Value::as_str).map(str::to_string),
        status: item.get("status")?.as_str()?.to_string(),
        aggregated_output: item
            .get("aggregatedOutput")
            .and_then(Value::as_str)
            .map(str::to_string),
        exit_code: item.get("exitCode").and_then(Value::as_i64),
        duration_ms: item.get("durationMs").and_then(Value::as_u64),
    })
}

fn error_message(error: Option<&Value>) -> Option<String> {
    let error = error.filter(|error| !error.is_null())?;
    Some(error
        .get("message")
        .and_then(Value::as_str)
        .or_else(|| error.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| "Codex reported a turn error".to_string()))
}

fn text_in(value: &Value) -> String {
    let mut text = Vec::new();
    for key in ["summary", "content"] {
        if let Some(parts) = value.get(key).and_then(Value::as_array) {
            for part in parts {
                if let Some(part) = part
                    .as_str()
                    .or_else(|| part.get("text").and_then(Value::as_str))
                {
                    if !part.trim().is_empty() {
                        text.push(part.trim());
                    }
                }
            }
        }
    }
    text.join("\n")
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Access {
    approval_policy: &'static str,
    sandbox: &'static str,
}

impl Access {
    pub(crate) fn for_runtime_mode(mode: &str) -> Result<Access, String> {
        let (approval_policy, sandbox) = match mode {
            "approval-required" => ("untrusted", "read-only"),
            "auto-accept-edits" => ("on-request", "workspace-write"),
            // Upstream delegates `auto` approvals to an OpenAI reviewer. Until
            // its review notifications are rendered, laplus keeps the user as
            // reviewer so an invisible subagent cannot make decisions for them.
            "auto" => ("on-request", "workspace-write"),
            "full-access" => ("never", "danger-full-access"),
            other => return Err(format!("Codex cannot use runtime mode '{other}'")),
        };
        Ok(Access {
            approval_policy,
            sandbox,
        })
    }

    fn params(&self) -> Value {
        json!({
            "approvalPolicy": self.approval_policy,
            "sandbox": self.sandbox,
            "approvalsReviewer": "user",
        })
    }
}

#[derive(Debug, Clone)]
pub(crate) enum Request {
    Initialize,
    Account,
    Models { cursor: Option<String> },
    Skills { cwds: Vec<String> },
    ThreadStart {
        cwd: String,
        model: Option<String>,
        access: Access,
    },
    // Ticket 09 activates resume; defining its access envelope here prevents
    // that path from inheriting a previous reviewer by omission.
    #[allow(dead_code)]
    ThreadResume { thread_id: String, access: Access },
    TurnStart { thread_id: String, text: String },
}

impl Request {
    pub(crate) fn method(&self) -> &'static str {
        match self {
            Request::Initialize => "initialize",
            Request::Account => "account/read",
            Request::Models { .. } => "model/list",
            Request::Skills { .. } => "skills/list",
            Request::ThreadStart { .. } => "thread/start",
            Request::ThreadResume { .. } => "thread/resume",
            Request::TurnStart { .. } => "turn/start",
        }
    }

    pub(crate) fn message(&self, id: u64) -> Value {
        let params = match self {
            Request::Initialize => json!({
                "clientInfo": {
                    "name": "laplus/client",
                    "title": "laplus",
                    "version": env!("CARGO_PKG_VERSION"),
                },
                "capabilities": Capabilities::current().message(),
            }),
            Request::Account => json!({}),
            Request::Models { cursor: None } => json!({}),
            Request::Models {
                cursor: Some(cursor),
            } => json!({"cursor": cursor}),
            Request::Skills { cwds } => json!({"cwds": cwds}),
            Request::ThreadStart { cwd, model, access } => {
                let mut params = access.params();
                params["cwd"] = json!(cwd);
                if let Some(model) = model {
                    params["model"] = json!(model);
                }
                params
            }
            Request::ThreadResume { thread_id, access } => {
                let mut params = access.params();
                params["threadId"] = json!(thread_id);
                params
            }
            Request::TurnStart { thread_id, text } => json!({
                "threadId": thread_id,
                "input": [{"type": "text", "text": text}],
            }),
        };
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": self.method(),
            "params": params,
        })
    }
}

pub(crate) fn initialized() -> Value {
    json!({"jsonrpc": "2.0", "method": "initialized"})
}

pub(crate) fn unsupported_request(id: &Value, method: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": -32601,
            "message": format!(
                "laplus does not handle app-server request '{method}' during a provider probe"
            ),
        }
    })
}

pub(crate) fn approval_response(id: &Value, decision: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {"decision": decision},
    })
}

pub(crate) enum Incoming {
    Response {
        id: u64,
        result: Result<Value, String>,
    },
    Request {
        id: Value,
        method: String,
    },
    Notification,
}

pub(crate) fn decode_incoming(line: &str) -> Result<Incoming, String> {
    let message: Value = serde_json::from_str(line)
        .map_err(|error| format!("Codex wrote malformed JSON-RPC: {error}"))?;
    let object = message
        .as_object()
        .ok_or_else(|| "Codex JSON-RPC message was not an object".to_string())?;
    if let Some(method) = object.get("method").and_then(Value::as_str) {
        return match object.get("id") {
            Some(id) => Ok(Incoming::Request {
                id: id.clone(),
                method: method.to_string(),
            }),
            None => Ok(Incoming::Notification),
        };
    }

    let id = object
        .get("id")
        .and_then(Value::as_u64)
        .ok_or_else(|| "Codex response did not carry a numeric id".to_string())?;
    let result = match object.get("error") {
        Some(error) => Err(error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("Codex refused the request")
            .to_string()),
        None => object
            .get("result")
            .cloned()
            .ok_or_else(|| "Codex response did not carry a result".to_string()),
    };
    Ok(Incoming::Response { id, result })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct InitializeResult {
    user_agent: String,
}

pub(crate) fn decode_initialize(response: Value) -> Result<String, String> {
    let result: InitializeResult = decode("initialize", response)?;
    version_in(&result.user_agent)
        .ok_or_else(|| "Codex initialize.userAgent did not contain a three-part version".to_string())
}

pub(crate) fn decode_thread_start(response: Value) -> Result<String, String> {
    response
        .get("thread")
        .and_then(|thread| thread.get("id"))
        .and_then(Value::as_str)
        .filter(|id| !id.trim().is_empty())
        .map(str::to_string)
        .ok_or_else(|| "Codex thread/start response did not carry a thread id".to_string())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccountResult {
    account: RequiredNullable<Account>,
    requires_openai_auth: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Account {
    #[serde(rename = "type")]
    kind: String,
    email: Option<String>,
    plan_type: Option<String>,
}

pub(crate) fn decode_account(response: Value) -> Result<ProviderAuth, String> {
    let result: AccountResult = decode("account/read", response)?;
    let Some(account) = result.account.0 else {
        return match result.requires_openai_auth {
            true => Ok(ProviderAuth {
                status: AuthStatus::Unauthenticated,
                r#type: None,
                label: None,
                email: None,
            }),
            false => Err(
                "Codex account/read returned no account without requiring authentication"
                    .to_string(),
            ),
        };
    };
    let kind = required_text("account.type", account.kind)?;
    let plan = optional_text(account.plan_type);
    Ok(ProviderAuth {
        status: AuthStatus::Authenticated,
        label: account_label(&kind, plan.as_deref()),
        email: optional_text(account.email),
        r#type: Some(kind),
    })
}

fn account_label(kind: &str, plan: Option<&str>) -> Option<String> {
    let label = match (kind, plan) {
        ("apiKey", _) => "OpenAI API Key",
        ("amazonBedrock", _) => "Amazon Bedrock",
        ("chatgpt", Some("free")) => "ChatGPT Free Subscription",
        ("chatgpt", Some("go")) => "ChatGPT Go Subscription",
        ("chatgpt", Some("plus")) => "ChatGPT Plus Subscription",
        ("chatgpt", Some("pro")) => "ChatGPT Pro 20x Subscription",
        ("chatgpt", Some("prolite")) => "ChatGPT Pro 5x Subscription",
        ("chatgpt", Some("team")) => "ChatGPT Team Subscription",
        ("chatgpt", Some("business" | "self_serve_business_usage_based")) => {
            "ChatGPT Business Subscription"
        }
        ("chatgpt", Some("enterprise" | "enterprise_cbp_usage_based" | "ent26")) => {
            "ChatGPT Enterprise Subscription"
        }
        ("chatgpt", Some("edu")) => "ChatGPT Edu Subscription",
        ("chatgpt", _) => "ChatGPT Subscription",
        _ => return None,
    };
    Some(label.to_string())
}

pub(crate) struct ModelPage {
    pub(crate) models: Vec<ProviderModel>,
    pub(crate) next_cursor: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelListResult {
    data: Vec<Model>,
    next_cursor: RequiredNullable<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Model {
    model: String,
    display_name: String,
    hidden: bool,
    is_default: bool,
    default_reasoning_effort: String,
    supported_reasoning_efforts: Vec<ReasoningEffort>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReasoningEffort {
    reasoning_effort: String,
}

pub(crate) fn decode_models(response: Value) -> Result<ModelPage, String> {
    let result: ModelListResult = decode("model/list", response)?;
    let mut models = Vec::new();
    for model in result.data.into_iter().filter(|model| !model.hidden) {
        let slug = required_text("model.model", model.model)?;
        let name = required_text("model.displayName", model.display_name)?;
        let default = required_text(
            "model.defaultReasoningEffort",
            model.default_reasoning_effort,
        )?;
        let mut options = Vec::new();
        for effort in model.supported_reasoning_efforts {
            let effort = required_text("reasoningEffort", effort.reasoning_effort)?;
            let mut option = json!({
                "id": effort,
                "label": reasoning_label(&effort),
            });
            if default == effort {
                option["isDefault"] = json!(true);
            }
            options.push(option);
        }
        if options.is_empty() {
            return Err(format!(
                "Codex model/list model '{slug}' had no supportedReasoningEfforts"
            ));
        }
        models.push(ProviderModel {
            slug,
            name,
            is_custom: false,
            is_default: model.is_default.then_some(true),
            capabilities: Some(json!({
                "optionDescriptors": [{
                    "id": "reasoningEffort",
                    "label": "Reasoning",
                    "type": "select",
                    "options": options,
                    "currentValue": default,
                }]
            })),
        });
    }
    Ok(ModelPage {
        models,
        next_cursor: result
            .next_cursor
            .0
            .and_then(|cursor| optional_text(Some(cursor))),
    })
}

pub(crate) fn append_custom_models(models: &mut Vec<ProviderModel>, custom: &[String]) {
    for slug in custom
        .iter()
        .map(|slug| slug.trim())
        .filter(|slug| !slug.is_empty())
    {
        if models.iter().any(|model| model.slug == slug) {
            continue;
        }
        models.push(ProviderModel {
            slug: slug.to_string(),
            name: slug.to_string(),
            is_custom: true,
            is_default: None,
            capabilities: None,
        });
    }
}

pub(crate) fn custom_models(custom: &[String]) -> Vec<ProviderModel> {
    let mut models = Vec::new();
    append_custom_models(&mut models, custom);
    models
}

fn reasoning_label(effort: &str) -> String {
    match effort {
        "none" => "None",
        "minimal" => "Minimal",
        "low" => "Low",
        "medium" => "Medium",
        "high" => "High",
        "xhigh" => "Extra High",
        "max" => "Max",
        "ultra" => "Ultra",
        other => other,
    }
    .to_string()
}

#[derive(Deserialize)]
struct SkillsResult {
    data: Vec<SkillsAtCwd>,
}

#[derive(Deserialize)]
struct SkillsAtCwd {
    skills: Vec<Skill>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Skill {
    name: String,
    path: String,
    enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    scope: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    short_description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    interface: Option<SkillInterface>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct SkillInterface {
    #[serde(skip_serializing_if = "Option::is_none")]
    display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    short_description: Option<String>,
}

pub(crate) fn decode_skills(response: Value) -> Result<Vec<Value>, String> {
    let result: SkillsResult = decode("skills/list", response)?;
    let mut skills = Vec::new();
    for entry in result.data {
        for skill in entry.skills {
            let name = required_text("skill.name", skill.name)?;
            let path = required_text("skill.path", skill.path)?;
            let mut rendered = json!({
                "name": name,
                "path": path,
                "enabled": skill.enabled,
            });
            if let Some(value) = optional_text(skill.description) {
                rendered["description"] = Value::String(value);
            }
            if let Some(value) = optional_text(skill.scope) {
                rendered["scope"] = Value::String(value);
            }
            if let Some(value) = optional_text(skill.short_description) {
                rendered["shortDescription"] = Value::String(value);
            }
            if let Some(interface) = skill.interface {
                if let Some(value) = optional_text(interface.display_name) {
                    rendered["displayName"] = Value::String(value);
                }
                if rendered.get("shortDescription").is_none() {
                    if let Some(value) = optional_text(interface.short_description) {
                        rendered["shortDescription"] = Value::String(value);
                    }
                }
            }
            skills.push(rendered);
        }
    }
    Ok(skills)
}

#[derive(Deserialize)]
struct RequiredNullable<T>(Option<T>);

fn decode<T: for<'de> Deserialize<'de>>(method: &str, response: Value) -> Result<T, String> {
    serde_json::from_value(response)
        .map_err(|error| format!("Codex {method} response was malformed: {error}"))
}

fn required_text(field: &str, value: String) -> Result<String, String> {
    optional_text(Some(value)).ok_or_else(|| format!("Codex {field} was empty"))
}

fn optional_text(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn version_in(user_agent: &str) -> Option<String> {
    let bytes = user_agent.as_bytes();
    for start in 0..bytes.len() {
        if !bytes[start].is_ascii_digit() {
            continue;
        }
        let tail = &user_agent[start..];
        let length = tail
            .bytes()
            .take_while(|byte| byte.is_ascii_digit() || *byte == b'.')
            .count();
        let candidate = &tail[..length];
        if candidate.split('.').count() == 3
            && candidate
                .split('.')
                .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
        {
            return Some(candidate.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::Path;

    use serde_json::{json, Value};

    use super::*;

    #[test]
    fn the_provider_fixture_pins_every_message_laplus_sends() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/codex-app-server/01-provider-probe.jsonl");
        let expected: Vec<Value> = std::fs::read_to_string(&fixture)
            .expect("reads the provider fixture")
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).expect("a fixture record"))
            .filter(|record| record["dir"] == "send")
            .map(|record| record["msg"].clone())
            .collect();

        let actual = vec![
            Request::Initialize.message(1),
            initialized(),
            Request::Account.message(2),
            Request::Models { cursor: None }.message(3),
            Request::Skills {
                cwds: vec!["<workspace>".to_string()],
            }
            .message(4),
            unsupported_request(&json!(2), "fixture/request"),
            Request::Models {
                cursor: Some("page-2".to_string()),
            }
            .message(5),
        ];

        assert_eq!(actual, expected);
    }

    #[test]
    fn the_provider_fixture_received_half_folds_to_the_expected_snapshot() {
        let directory = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/codex-app-server");
        let fixture = std::fs::read_to_string(directory.join("01-provider-probe.jsonl"))
            .expect("reads the provider fixture");
        let mut responses = HashMap::new();
        let mut notifications = 0;
        let mut server_requests = Vec::new();
        for record in fixture
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).expect("a fixture record"))
            .filter(|record| record["dir"] == "recv")
        {
            match decode_incoming(&record["msg"].to_string()).expect("a received message") {
                Incoming::Notification => notifications += 1,
                Incoming::Request { id, method, .. } => {
                    server_requests.push(json!({"id": id, "method": method}));
                }
                Incoming::Response { id, result } => {
                    responses.insert(id, result.expect("a successful fixture response"));
                }
            }
        }

        let version = decode_initialize(responses.remove(&1).expect("initialize response"))
            .expect("the version");
        let auth = decode_account(responses.remove(&2).expect("account response"))
            .expect("the account");
        let skills = decode_skills(responses.remove(&4).expect("skills response"))
            .expect("the skills");
        let first = decode_models(responses.remove(&3).expect("first model page"))
            .expect("the first models");
        assert_eq!(first.next_cursor.as_deref(), Some("page-2"));
        let last = decode_models(responses.remove(&5).expect("last model page"))
            .expect("the last models");
        assert_eq!(last.next_cursor, None);
        let mut models = first.models;
        models.extend(last.models);

        let actual = json!({
            "version": version,
            "auth": auth,
            "models": models,
            "skills": skills,
            "notificationsIgnored": notifications,
            "serverRequests": server_requests,
        });
        let expected: Value = serde_json::from_str(
            &std::fs::read_to_string(directory.join("01-provider-probe.expected.json"))
                .expect("reads the expected provider fold"),
        )
        .expect("the expected provider fold is JSON");

        assert_eq!(actual, expected);
    }

    #[test]
    fn every_runtime_mode_sets_access_explicitly_on_start_and_resume() {
        for (mode, approval_policy, sandbox) in [
            ("approval-required", "untrusted", "read-only"),
            ("auto-accept-edits", "on-request", "workspace-write"),
            ("auto", "on-request", "workspace-write"),
            ("full-access", "never", "danger-full-access"),
        ] {
            let access = Access::for_runtime_mode(mode).expect("a contract runtime mode");
            let start = Request::ThreadStart {
                cwd: "<workspace>".to_string(),
                model: None,
                access,
            }
            .message(1);
            let resume = Request::ThreadResume {
                thread_id: "codex-thread-1".to_string(),
                access,
            }
            .message(2);

            for request in [start, resume] {
                assert_eq!(request["params"]["approvalPolicy"], approval_policy);
                assert_eq!(request["params"]["sandbox"], sandbox);
                assert_eq!(request["params"]["approvalsReviewer"], "user");
            }
        }
    }

    #[test]
    fn the_turn_fixture_pins_every_message_laplus_sends() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/codex-app-server/01-plain-turn.jsonl");
        let expected: Vec<Value> = std::fs::read_to_string(&fixture)
            .expect("reads the turn fixture")
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).expect("a fixture record"))
            .filter(|record| record["dir"] == "send")
            .map(|record| record["msg"].clone())
            .collect();
        let actual = vec![
            Request::Initialize.message(1),
            initialized(),
            Request::ThreadStart {
                cwd: "<workspace>".to_string(),
                model: Some("gpt-5.4-mini".to_string()),
                // This fixture preserves an observed experimental combination,
                // rather than one of laplus's runtime-mode mappings.
                access: Access {
                    approval_policy: "never",
                    sandbox: "read-only",
                },
            }
            .message(2),
            Request::TurnStart {
                thread_id: "codex-thread-1".to_string(),
                text: "Reply with exactly one short sentence saying hello. Do not use any tools."
                    .to_string(),
            }
            .message(3),
        ];

        assert_eq!(actual, expected);
    }

    #[test]
    fn the_handshake_policy_selects_the_terminal_fallback() {
        assert!(!Capabilities::current().idle_is_terminal());
        assert!(Capabilities::experimental().idle_is_terminal());
    }

    #[test]
    fn every_approval_kind_keeps_only_contract_decisions() {
        for (method, kind, tool) in [
            ("item/commandExecution/requestApproval", "command", "Command"),
            ("item/fileRead/requestApproval", "file-read", "Read"),
            ("item/fileChange/requestApproval", "file-change", "Write"),
        ] {
            let mut state = ConversationState::new();
            let folded = state.fold_message(json!({
                "id": "request-1",
                "method": method,
                "params": {
                    "itemId": "item-1",
                    "command": "printf hi",
                    "cwd": "<workspace>",
                    "availableDecisions": [
                        "acceptForSession",
                        {"acceptWithNetworkPolicyAmendment": {"host": "example.com"}},
                        "decline"
                    ]
                }
            }));
            let ConversationFold::ApprovalRequested(request) = folded else {
                panic!("{method} did not become an approval request");
            };

            assert_eq!(request.server_request_id, json!("request-1"));
            assert_eq!(request.request_id, "codex:\"request-1\"");
            assert_eq!(request.request_kind, kind);
            assert_eq!(request.tool_name, tool);
            assert_eq!(
                request.available_decisions,
                vec![Decision::AcceptForSession, Decision::Decline]
            );
            assert_eq!(state.unknown_events, 0, "{method}");
        }
    }

    #[test]
    fn missing_required_probe_fields_are_errors_not_empty_healthy_answers() {
        assert!(decode_initialize(json!({})).is_err());
        assert!(decode_account(json!({"requiresOpenaiAuth": true})).is_err());
        assert!(decode_models(json!({"nextCursor": null})).is_err());
        assert!(decode_models(json!({"data": []})).is_err());
        assert!(decode_skills(json!({})).is_err());
        assert!(decode_skills(json!({"data": [{"cwd": "x"}]})).is_err());
    }

    #[test]
    fn an_explicit_logged_out_account_is_not_a_malformed_account() {
        let auth = decode_account(json!({
            "account": null,
            "requiresOpenaiAuth": true,
        }))
        .expect("logged out is a valid account response");

        assert_eq!(auth.status, crate::config::AuthStatus::Unauthenticated);
    }

    #[test]
    fn the_version_parse_does_not_assume_the_client_name_is_slash_free() {
        assert_eq!(
            decode_initialize(json!({
                "userAgent": "laplus/client/0.146.0 (Windows 11; x86_64) unknown"
            })),
            Ok("0.146.0".to_string())
        );
    }
}
