//! The provider-probe subset of the `codex app-server` JSON-RPC transport.
//!
//! Responses on this wire omit `jsonrpc` and may arrive in any order, while
//! requests sent by app-server use an independent id space. Classification is
//! therefore by shape first and client responses are correlated only through
//! the ids this client has in `pending`.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::config::{AuthStatus, CodexSettings, ProviderAuth, ProviderModel};

const RESPONSE_TIMEOUT: Duration = Duration::from_secs(10);
const PREFERRED_MODELS: &[&str] = &["gpt-5.6-sol", "gpt-5.6-terra"];

pub struct Snapshot {
    pub version: Option<String>,
    pub auth: ProviderAuth,
    pub models: Vec<ProviderModel>,
    pub skills: Vec<Value>,
}

pub fn probe(
    binary: &Path,
    settings: &CodexSettings,
    roots: &[PathBuf],
) -> Result<Snapshot, String> {
    let cwd = roots
        .first()
        .cloned()
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));
    let mut client = Client::start(binary, settings, &cwd)?;

    let initialize = client.request(
        "initialize",
        json!({
            "clientInfo": {
                "name": "laplus/client",
                "title": "laplus",
                "version": env!("CARGO_PKG_VERSION"),
            },
            "capabilities": {},
        }),
    )?;
    client.notify("initialized", None)?;

    let account_id = client.send_request("account/read", json!({}))?;
    let models_id = client.send_request("model/list", json!({}))?;
    let cwds: Vec<String> = match roots.is_empty() {
        true => vec![cwd.display().to_string()],
        false => roots
            .iter()
            .map(|root| root.display().to_string())
            .collect(),
    };
    let skills_id = client.send_request("skills/list", json!({"cwds": cwds}))?;

    let account = client.wait(account_id)?;
    let skills = client.wait(skills_id)?;
    let mut page = client.wait(models_id)?;
    let mut models = Vec::new();
    loop {
        models.extend(parse_models(&page));
        let cursor = page
            .get("nextCursor")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|cursor| !cursor.is_empty())
            .map(str::to_string);
        let Some(cursor) = cursor else { break };
        page = client.request("model/list", json!({"cursor": cursor}))?;
    }
    append_custom_models(&mut models, &settings.custom_models);
    prefer_default(&mut models);

    Ok(Snapshot {
        version: initialize
            .get("userAgent")
            .and_then(Value::as_str)
            .and_then(version_in),
        auth: parse_account(&account),
        models,
        skills: parse_skills(&skills),
    })
}

fn parse_account(response: &Value) -> ProviderAuth {
    let Some(account) = response.get("account").filter(|account| !account.is_null()) else {
        return ProviderAuth {
            status: match response.get("requiresOpenaiAuth").and_then(Value::as_bool) {
                Some(true) => AuthStatus::Unauthenticated,
                _ => AuthStatus::Unknown,
            },
            r#type: None,
            label: None,
            email: None,
        };
    };
    let kind = text(account.get("type"));
    let plan = text(account.get("planType"));
    ProviderAuth {
        status: AuthStatus::Authenticated,
        label: account_label(kind.as_deref(), plan.as_deref()),
        email: text(account.get("email")),
        r#type: kind,
    }
}

fn account_label(kind: Option<&str>, plan: Option<&str>) -> Option<String> {
    let label = match (kind, plan) {
        (Some("apiKey"), _) => "OpenAI API Key",
        (Some("amazonBedrock"), _) => "Amazon Bedrock",
        (Some("chatgpt"), Some("free")) => "ChatGPT Free Subscription",
        (Some("chatgpt"), Some("go")) => "ChatGPT Go Subscription",
        (Some("chatgpt"), Some("plus")) => "ChatGPT Plus Subscription",
        (Some("chatgpt"), Some("pro")) => "ChatGPT Pro 20x Subscription",
        (Some("chatgpt"), Some("prolite")) => "ChatGPT Pro 5x Subscription",
        (Some("chatgpt"), Some("team")) => "ChatGPT Team Subscription",
        (Some("chatgpt"), Some("business" | "self_serve_business_usage_based")) => {
            "ChatGPT Business Subscription"
        }
        (Some("chatgpt"), Some("enterprise" | "enterprise_cbp_usage_based" | "ent26")) => {
            "ChatGPT Enterprise Subscription"
        }
        (Some("chatgpt"), Some("edu")) => "ChatGPT Edu Subscription",
        (Some("chatgpt"), _) => "ChatGPT Subscription",
        _ => return None,
    };
    Some(label.to_string())
}

fn parse_models(response: &Value) -> Vec<ProviderModel> {
    response
        .get("data")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter(|model| model.get("hidden").and_then(Value::as_bool) != Some(true))
        .filter_map(|model| {
            let slug = text(model.get("model"))?;
            let name = text(model.get("displayName")).unwrap_or_else(|| slug.clone());
            Some(ProviderModel {
                slug,
                name,
                is_custom: false,
                is_default: model
                    .get("isDefault")
                    .and_then(Value::as_bool)
                    .filter(|is_default| *is_default),
                capabilities: Some(reasoning_capabilities(model)),
            })
        })
        .collect()
}

fn reasoning_capabilities(model: &Value) -> Value {
    let default = text(model.get("defaultReasoningEffort"));
    let options: Vec<Value> = model
        .get("supportedReasoningEfforts")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter_map(|effort| text(effort.get("reasoningEffort")))
        .map(|effort| {
            let mut option = json!({
                "id": effort,
                "label": reasoning_label(&effort),
            });
            if default.as_deref() == Some(effort.as_str()) {
                option["isDefault"] = json!(true);
            }
            option
        })
        .collect();
    if options.is_empty() {
        return json!({"optionDescriptors": []});
    }
    let mut descriptor = json!({
        "id": "reasoningEffort",
        "label": "Reasoning",
        "type": "select",
        "options": options,
    });
    if let Some(default) = default {
        descriptor["currentValue"] = Value::String(default);
    }
    json!({"optionDescriptors": [descriptor]})
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

fn append_custom_models(models: &mut Vec<ProviderModel>, custom: &[String]) {
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

fn prefer_default(models: &mut [ProviderModel]) {
    let preferred = PREFERRED_MODELS.iter().find(|slug| {
        models
            .iter()
            .any(|model| !model.is_custom && model.slug == **slug)
    });
    let Some(preferred) = preferred else { return };
    for model in models {
        model.is_default = (model.slug == *preferred).then_some(true);
    }
}

fn parse_skills(response: &Value) -> Vec<Value> {
    response
        .get("data")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .flat_map(|entry| {
            entry
                .get("skills")
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or_default()
        })
        .filter_map(|skill| {
            let name = text(skill.get("name"))?;
            let path = text(skill.get("path"))?;
            let mut rendered = json!({
                "name": name,
                "path": path,
                "enabled": skill.get("enabled").and_then(Value::as_bool).unwrap_or(true),
            });
            for field in ["description", "scope", "shortDescription"] {
                if let Some(value) = text(skill.get(field)) {
                    rendered[field] = Value::String(value);
                }
            }
            if let Some(interface) = skill.get("interface") {
                if let Some(value) = text(interface.get("displayName")) {
                    rendered["displayName"] = Value::String(value);
                }
                if rendered.get("shortDescription").is_none() {
                    if let Some(value) = text(interface.get("shortDescription")) {
                        rendered["shortDescription"] = Value::String(value);
                    }
                }
            }
            Some(rendered)
        })
        .collect()
}

fn text(value: Option<&Value>) -> Option<String> {
    let value = value?.as_str()?.trim();
    (!value.is_empty()).then(|| value.to_string())
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
                .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
        {
            return Some(candidate.to_string());
        }
    }
    None
}

struct Client {
    child: Child,
    stdin: Option<ChildStdin>,
    output: mpsc::Receiver<String>,
    pending: HashMap<u64, String>,
    responses: HashMap<u64, Result<Value, String>>,
    next_id: u64,
    stderr: Arc<Mutex<Option<String>>>,
}

impl Client {
    fn start(binary: &Path, settings: &CodexSettings, cwd: &Path) -> Result<Client, String> {
        let launch_args = shell_words::split(&settings.launch_args)
            .map_err(|error| format!("Codex launch arguments could not be read: {error}"))?;
        let mut command = Command::new(binary);
        command
            .arg("app-server")
            .args(launch_args)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if !settings.home_path.trim().is_empty() {
            command.env(
                "CODEX_HOME",
                crate::projects::expand_home(settings.home_path.trim()),
            );
        }
        crate::process::without_a_console(&mut command);
        let mut child = command
            .spawn()
            .map_err(|error| format!("{} could not be started: {error}", binary.display()))?;
        let pipes = (child.stdin.take(), child.stdout.take(), child.stderr.take());
        let (Some(stdin), Some(stdout), Some(child_stderr)) = pipes else {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Codex was started without one of its stdio pipes".to_string());
        };

        let (lines, output) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if lines.send(line).is_err() {
                    break;
                }
            }
        });
        let stderr = Arc::new(Mutex::new(None));
        let latest = Arc::clone(&stderr);
        std::thread::spawn(move || {
            for line in BufReader::new(child_stderr).lines().map_while(Result::ok) {
                if !line.trim().is_empty() {
                    // Severity words are Codex's logging vocabulary, not process
                    // state. Only a failed request makes stderr diagnostic.
                    eprintln!("laplus: codex: {line}");
                    *latest
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                        Some(line.trim().to_string());
                }
            }
        });

        Ok(Client {
            child,
            stdin: Some(stdin),
            output,
            pending: HashMap::new(),
            responses: HashMap::new(),
            next_id: 1,
            stderr,
        })
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.send_request(method, params)?;
        self.wait(id)
    }

    fn send_request(&mut self, method: &str, params: Value) -> Result<u64, String> {
        let id = self.next_id;
        self.next_id += 1;
        self.write(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))?;
        self.pending.insert(id, method.to_string());
        Ok(id)
    }

    fn notify(&mut self, method: &str, params: Option<Value>) -> Result<(), String> {
        let mut message = json!({"jsonrpc": "2.0", "method": method});
        if let Some(params) = params {
            message["params"] = params;
        }
        self.write(&message)
    }

    fn write(&mut self, message: &Value) -> Result<(), String> {
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| "Codex stdin is closed".to_string())?;
        writeln!(stdin, "{message}")
            .and_then(|()| stdin.flush())
            .map_err(|error| format!("Codex request could not be written: {error}"))
    }

    fn wait(&mut self, wanted: u64) -> Result<Value, String> {
        let deadline = Instant::now() + RESPONSE_TIMEOUT;
        loop {
            if let Some(response) = self.responses.remove(&wanted) {
                return response;
            }
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .unwrap_or_default();
            let line = self.output.recv_timeout(remaining).map_err(|error| {
                let request = self
                    .pending
                    .get(&wanted)
                    .map(String::as_str)
                    .unwrap_or("unknown request");
                let last = self
                    .stderr
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .clone();
                match last {
                    Some(last) => format!(
                        "Codex stopped answering {request} ({error}); stderr ended with: {last}"
                    ),
                    None => format!("Codex stopped answering {request} ({error})"),
                }
            })?;
            let Ok(message) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            // A method plus an id is app-server asking us something. Its id is
            // not looked up in `pending`, even when the number is identical.
            if let Some(method) = message.get("method").and_then(Value::as_str) {
                if let Some(id) = message.get("id") {
                    self.write(&json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": {
                            "code": -32601,
                            "message": format!(
                                "laplus does not handle app-server request '{method}' during a provider probe"
                            ),
                        }
                    }))?;
                }
                continue;
            }
            let Some(id) = message.get("id").and_then(Value::as_u64) else {
                continue;
            };
            if self.pending.remove(&id).is_none() {
                continue;
            }
            let response = match message.get("error") {
                Some(error) => Err(error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("Codex refused the request")
                    .to_string()),
                None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
            };
            self.responses.insert(id, response);
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        drop(self.stdin.take());
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_version_parse_does_not_assume_the_client_name_is_slash_free() {
        assert_eq!(
            version_in("laplus/client/0.146.0 (Windows 11; x86_64) unknown"),
            Some("0.146.0".to_string())
        );
    }
}
