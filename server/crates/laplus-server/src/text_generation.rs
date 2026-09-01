//! Provider-neutral short text generation, with OpenCode's isolated-session adapter.
//!
//! This is deliberately an internal application boundary rather than another
//! socket method. Callers ask for a destination-shaped result; provider wire,
//! temporary sessions and process ownership remain behind this module.

use std::{collections::HashMap, fmt, path::PathBuf, sync::Arc, time::Duration};

use serde_json::{json, Value};
use tokio::{sync::Mutex, time::Instant};

use crate::{
    config::ClaudeSettings,
    opencode::{OpenCodeClient, OwnedServer},
    process::Search,
    provider::{ClaudeInstance, ConfiguredInstance, OpenCodeInstance},
    threads::PromptAttachment,
};

const IDLE: Duration = Duration::from_secs(30);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdleDecision {
    Keep,
    Reap,
}

/// The lifecycle decision is explicit so tests do not make the scheduler or
/// wall-clock speed part of the contract.
pub fn idle_decision(idle_for: Duration) -> IdleDecision {
    if idle_for >= IDLE {
        IdleDecision::Reap
    } else {
        IdleDecision::Keep
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Operation {
    CommitMessage { context: String },
    PullRequest { context: String },
    BranchName { context: String },
    ThreadTitle { context: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResultText {
    CommitMessage {
        subject: String,
        body: Option<String>,
    },
    PullRequest {
        title: String,
        body: String,
    },
    BranchName(String),
    ThreadTitle(String),
}

#[derive(Debug)]
pub struct Error(String);

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
impl std::error::Error for Error {}

struct Pooled {
    owned: OwnedServer,
    client: OpenCodeClient,
    binary: PathBuf,
    directory: String,
    idle_since: Instant,
}

#[derive(Clone)]
pub struct Service {
    local: Arc<Mutex<HashMap<String, Pooled>>>,
    request_timeout: Duration,
}

impl Default for Service {
    fn default() -> Self {
        Self {
            local: Arc::default(),
            request_timeout: REQUEST_TIMEOUT,
        }
    }
}

impl Service {
    pub fn new() -> Self {
        Self::default()
    }

    /// A bounded request deadline; exposed for deterministic scripted peers.
    pub fn with_request_timeout(request_timeout: Duration) -> Self {
        Self {
            request_timeout,
            ..Self::default()
        }
    }

    pub async fn generate(
        &self,
        instance: &ConfiguredInstance,
        directory: &str,
        model: Option<&str>,
        operation: Operation,
    ) -> Result<ResultText, Error> {
        self.generate_with_attachments(instance, directory, model, operation, &[]).await
    }

    pub async fn generate_with_attachments(&self, instance: &ConfiguredInstance, directory: &str, model: Option<&str>, operation: Operation, attachments: &[PromptAttachment]) -> Result<ResultText, Error> {
        match instance {
            ConfiguredInstance::Claude(instance) => {
                self.generate_claude(instance, directory, model, operation, attachments)
                    .await
            }
            ConfiguredInstance::OpenCode(instance) => {
                self.generate_opencode(instance, directory, model, operation, attachments)
                    .await
            }
            ConfiguredInstance::Codex(instance) => {
                if !matches!(operation, Operation::ThreadTitle { .. }) {
                    return Err(Error("Codex only supports thread-title text generation in this build.".into()));
                }
                tokio::time::timeout(self.request_timeout, crate::codex::generate_title(instance, directory, model, prompt(&operation), attachments, self.request_timeout)).await
                    .map_err(|_| Error("Codex title generation timed out.".into()))?
                    .map_err(Error)
                    .and_then(|value| parse_value(operation, &value))
            }
        }
    }

    async fn generate_claude(
        &self,
        instance: &ClaudeInstance,
        directory: &str,
        model: Option<&str>,
        operation: Operation,
        attachments: &[PromptAttachment],
    ) -> Result<ResultText, Error> {
        if !matches!(operation, Operation::ThreadTitle { .. }) {
            return Err(Error(
                "Claude only supports thread-title text generation in this build.".into(),
            ));
        }
        let binary = crate::provider::resolve_named(
            &instance.settings.binary_path,
            "claude",
            &Search::from_environment(),
        )
        .startable_for("Claude CLI")
        .map_err(Error)?
        .0;
        generate_with_claude(
            &binary,
            &instance.settings,
            directory,
            model,
            operation,
            attachments,
            self.request_timeout,
        )
        .await
    }

    async fn generate_opencode(
        &self,
        instance: &OpenCodeInstance,
        directory: &str,
        model: Option<&str>,
        operation: Operation,
        attachments: &[PromptAttachment],
    ) -> Result<ResultText, Error> {
        if !instance.settings.server_url.is_empty() {
            let password = (!instance.settings.server_password.is_empty())
                .then(|| instance.settings.server_password.clone());
            let client = OpenCodeClient::new(&instance.settings.server_url, directory, password)
                .map_err(|error| Error(error.to_string()))?;
            return generate_with(&client, model, operation, attachments, self.request_timeout).await;
        }

        let binary = crate::provider::resolve_named(
            &instance.settings.binary_path,
            "opencode",
            &crate::process::Search::from_environment(),
        )
        .startable_for("OpenCode CLI")
        .map_err(Error)?
        .0;
        let mut pool = self.local.lock().await;
        let replace = pool
            .get(&instance.identity.instance_id)
            .is_some_and(|entry| entry.binary != binary || entry.directory != directory);
        if replace {
            if let Some(mut old) = pool.remove(&instance.identity.instance_id) {
                old.owned.stop().await;
            }
        }
        if !pool.contains_key(&instance.identity.instance_id) {
            let (owned, client) = OwnedServer::start(&binary, directory)
                .await
                .map_err(Error)?;
            pool.insert(
                instance.identity.instance_id.clone(),
                Pooled {
                    owned,
                    client,
                    binary,
                    directory: directory.to_string(),
                    idle_since: Instant::now(),
                },
            );
        }
        let client = pool
            .get(&instance.identity.instance_id)
            .expect("inserted")
            .client
            .clone();
        drop(pool);
        let result = generate_with(&client, model, operation, attachments, self.request_timeout).await;
        let mut pool = self.local.lock().await;
        if let Some(entry) = pool.get_mut(&instance.identity.instance_id) {
            entry.idle_since = Instant::now();
        }
        drop(pool);
        self.schedule_reap(instance.identity.instance_id.clone());
        result
    }

    fn schedule_reap(&self, instance_id: String) {
        let local = self.local.clone();
        tokio::spawn(async move {
            tokio::time::sleep(IDLE).await;
            let mut pool = local.lock().await;
            let expired = pool.get(&instance_id).is_some_and(|entry| {
                idle_decision(entry.idle_since.elapsed()) == IdleDecision::Reap
            });
            if expired {
                if let Some(mut entry) = pool.remove(&instance_id) {
                    entry.owned.stop().await;
                }
            }
        });
    }

    /// Apply the same idle decision as the reaper at a caller-controlled
    /// instant. Tests use this instead of asserting how long a timer slept.
    pub async fn reap_idle_at(&self, now: Instant) -> usize {
        let mut pool = self.local.lock().await;
        let expired = pool
            .iter()
            .filter_map(|(id, entry)| {
                (idle_decision(now.duration_since(entry.idle_since)) == IdleDecision::Reap)
                    .then_some(id.clone())
            })
            .collect::<Vec<_>>();
        let count = expired.len();
        for id in expired {
            if let Some(mut entry) = pool.remove(&id) {
                entry.owned.stop().await;
            }
        }
        count
    }
}

async fn generate_with_claude(
    binary: &std::path::Path,
    settings: &ClaudeSettings,
    directory: &str,
    model: Option<&str>,
    operation: Operation,
    attachments: &[PromptAttachment],
    timeout: Duration,
) -> Result<ResultText, Error> {
    let schema = json!({"type":"object","properties":{"title":{"type":"string"}},"required":["title"],"additionalProperties":false});
    let launch_args = shell_words::split(&settings.launch_args)
        .map_err(|error| Error(format!("Claude launch arguments are invalid: {error}")))?;
    let mut command = tokio::process::Command::new(binary);
    command
        .args(launch_args)
        .arg("-p")
        .arg("--output-format")
        .arg("json")
        .arg("--json-schema")
        .arg(schema.to_string())
        .arg("--dangerously-skip-permissions")
        .stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped())
        .current_dir(directory)
        .kill_on_drop(true);
    if !settings.home_path.trim().is_empty() {
        command.env(
            "CLAUDE_CONFIG_DIR",
            crate::projects::expand_home(settings.home_path.trim()),
        );
    }
    if let Some(model) = model {
        command.arg("--model").arg(model);
    }
    if attachments.is_empty() { command.arg(prompt(&operation)); } else { command.arg("--input-format").arg("stream-json").stdin(std::process::Stdio::piped()); }
    crate::process::without_a_console(command.as_std_mut());
    let mut child = command.spawn().map_err(|error| Error(format!("Claude text generation could not start: {error}")))?;
    // A short-lived `claude` is still a `claude`. This one is bounded by a
    // timeout and reaped by `kill_on_drop`, and neither of those survives an
    // abrupt end to this process — see `crate::process::bound_to_this_server`.
    crate::process::bound_to_this_server_async(&child);
    if !attachments.is_empty() {
        use tokio::io::AsyncWriteExt;
        let content = crate::turn::prompt_content(&prompt(&operation), attachments).await.map_err(Error)?;
        let mut input = child.stdin.take().ok_or_else(|| Error("Claude text generation has no stdin.".into()))?;
        input.write_all(format!("{}\n", crate::protocol::user_message_line(&content)).as_bytes()).await.map_err(|error| Error(format!("Claude text generation input failed: {error}")))?;
    }
    let output = tokio::time::timeout(timeout, child.wait_with_output())
        .await
        .map_err(|_| Error("Claude text generation timed out.".into()))?
        .map_err(|error| Error(format!("Claude text generation could not start: {error}")))?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(Error(if detail.is_empty() {
            format!("Claude text generation exited with {}.", output.status)
        } else {
            format!("Claude text generation failed: {detail}")
        }));
    }
    let envelope: Value = serde_json::from_slice(&output.stdout)
        .map_err(|_| Error("Claude returned malformed JSON output.".into()))?;
    let structured = envelope
        .get("structured_output")
        .ok_or_else(|| Error("Claude output has no structured_output value.".into()))?;
    parse_value(operation, structured)
}

async fn generate_with(
    client: &OpenCodeClient,
    model: Option<&str>,
    operation: Operation,
    attachments: &[PromptAttachment],
    timeout: Duration,
) -> Result<ResultText, Error> {
    let session = tokio::time::timeout(
        timeout,
        client.create_session(&json!({
            "permission": [{"permission":"*","pattern":"*","action":"deny"}]
        })),
    )
    .await
    .map_err(|_| {
        Error("OpenCode text generation timed out creating its temporary session.".into())
    })?
    .map_err(|error| Error(error.to_string()))?;
    let generated = async {
        let parts = crate::opencode::prompt_parts(&prompt(&operation), attachments);
        let mut body = json!({
            "parts": parts,
            "tools": {}
        });
        if let Some(model) = model {
            let (provider, model) = model
                .split_once('/')
                .ok_or_else(|| Error("OpenCode model must be provider/model.".into()))?;
            body["model"] = json!({"providerID": provider, "modelID": model});
        }
        let answer = tokio::time::timeout(timeout, client.prompt_sync(&session.id, &body))
            .await
            .map_err(|_| {
                Error("OpenCode text generation timed out waiting for a response.".into())
            })?
            .map_err(|error| Error(error.to_string()))?;
        parse(operation, &answer)
    }
    .await;
    let cleanup = tokio::time::timeout(timeout, client.delete_session(&session.id))
        .await
        .map_err(|_| Error("OpenCode temporary session cleanup timed out.".into()))
        .and_then(|result| result.map_err(|error| Error(error.to_string())));
    match (generated, cleanup) {
        (Ok(value), Ok(_)) => Ok(value),
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => Err(Error(format!(
            "OpenCode temporary session cleanup failed: {error}"
        ))),
    }
}

fn prompt(operation: &Operation) -> String {
    let (schema, context) = match operation {
        Operation::CommitMessage { context } => {
            (r#"{"subject":"...","body":"... or empty"}"#, context)
        }
        Operation::PullRequest { context } => (r#"{"title":"...","body":"..."}"#, context),
        Operation::BranchName { context } => (r#"{"branchName":"..."}"#, context),
        Operation::ThreadTitle { context } => (r#"{"title":"..."}"#, context),
    };
    let instruction = match operation {
        Operation::ThreadTitle { .. } => {
            "Write a concise descriptive title for this conversation. Do not answer the request."
        }
        _ => "Summarize the source for the requested destination.",
    };
    format!("{instruction} Return only one JSON object matching {schema}. Do not use tools. Source:\n{context}")
}

fn parse(operation: Operation, answer: &Value) -> Result<ResultText, Error> {
    let parts = answer
        .get("parts")
        .and_then(Value::as_array)
        .ok_or_else(|| Error("OpenCode text generation response has no parts array.".into()))?;
    if parts
        .iter()
        .any(|part| part.get("type").and_then(Value::as_str) == Some("tool"))
    {
        return Err(Error(
            "OpenCode attempted a tool during deny-all text generation.".into(),
        ));
    }
    let raw = parts
        .iter()
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .collect::<String>();
    let value: Value = serde_json::from_str(raw.trim())
        .map_err(|_| Error("OpenCode returned malformed structured text.".into()))?;
    parse_value(operation, &value)
}

fn parse_value(operation: Operation, value: &Value) -> Result<ResultText, Error> {
    match operation {
        Operation::CommitMessage { .. } => {
            let subject = line(field(&value, "subject")?, 72)?;
            let body = field(&value, "body")
                .ok()
                .map(clean_block)
                .filter(|body| !body.is_empty());
            Ok(ResultText::CommitMessage { subject, body })
        }
        Operation::PullRequest { .. } => Ok(ResultText::PullRequest {
            title: line(field(&value, "title")?, 120)?,
            body: clean_block(field(&value, "body")?),
        }),
        Operation::BranchName { .. } => Ok(ResultText::BranchName(branch(field(
            &value,
            "branchName",
        )?)?)),
        Operation::ThreadTitle { .. } => {
            Ok(ResultText::ThreadTitle(line(field(&value, "title")?, 120)?))
        }
    }
}

fn field<'a>(value: &'a Value, name: &str) -> Result<&'a str, Error> {
    value
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| Error(format!("Structured text is missing {name}.")))
}
fn clean_block(value: &str) -> String {
    value.replace("\r\n", "\n").trim().to_string()
}
fn line(value: &str, limit: usize) -> Result<String, Error> {
    let value = value.split_whitespace().collect::<Vec<_>>().join(" ");
    let value = value.trim_matches(['`', '"', '\'']).trim().to_string();
    if value.is_empty() {
        return Err(Error("OpenCode generated an empty value.".into()));
    }
    Ok(value.chars().take(limit).collect())
}
fn branch(value: &str) -> Result<String, Error> {
    let value = value
        .trim()
        .to_lowercase()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '/' | '.') {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>();
    let mut parts = Vec::new();
    for raw in value.split('/') {
        let mut part = raw.trim_matches(['-', '.']).to_string();
        while part.contains("--") {
            part = part.replace("--", "-");
        }
        while part.contains("..") {
            part = part.replace("..", ".");
        }
        if part.ends_with(".lock") {
            part.push_str("-branch");
        }
        if !part.is_empty() {
            parts.push(part.chars().take(100).collect::<String>());
        }
    }
    let value = parts.join("/");
    let value = value.chars().take(100).collect::<String>();
    let value = value.trim_matches(['-', '/', '.']).to_string();
    if value.is_empty() {
        return Err(Error("OpenCode generated an invalid branch name.".into()));
    }
    Ok(value)
}
