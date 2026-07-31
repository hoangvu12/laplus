//! Pure types and folds for the provider-probe subset of Codex JSON-RPC.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::config::{AuthStatus, ProviderAuth, ProviderModel};

#[derive(Debug, Clone)]
pub(crate) enum Request {
    Initialize,
    Account,
    Models { cursor: Option<String> },
    Skills { cwds: Vec<String> },
}

impl Request {
    pub(crate) fn method(&self) -> &'static str {
        match self {
            Request::Initialize => "initialize",
            Request::Account => "account/read",
            Request::Models { .. } => "model/list",
            Request::Skills { .. } => "skills/list",
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
                "capabilities": {},
            }),
            Request::Account => json!({}),
            Request::Models { cursor: None } => json!({}),
            Request::Models {
                cursor: Some(cursor),
            } => json!({"cursor": cursor}),
            Request::Skills { cwds } => json!({"cwds": cwds}),
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
                Incoming::Request { id, method } => {
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
