//! Conversation-scoped Model Context Protocol sessions.

use std::{
    collections::HashMap,
    fmt,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex, RwLock},
};

use serde_json::{json, Value};

const PROTOCOL_VERSION: &str = "2025-06-18";

/// The provider-facing seam. Provider adapters can acquire connection material
/// and own its lifetime, but cannot choose or route individual host tools.
pub trait Platform: fmt::Debug + Send + Sync {
    fn open_session(&self, thread_id: &str) -> Result<Session, OpenError>;
    fn set_origin(&self, origin: String);
    fn authorizes(&self, id: &str, authorization: &str) -> bool;
    fn live_sessions(&self) -> usize;
    fn dispatch<'a>(
        &'a self,
        id: &'a str,
        message: Value,
    ) -> Pin<Box<dyn Future<Output = Value> + Send + 'a>>;
}

#[derive(Debug, Clone, Copy)]
pub struct OpenError;

impl fmt::Display for OpenError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Laplus could not open the conversation's MCP session")
    }
}

impl std::error::Error for OpenError {}

#[derive(Clone)]
pub struct Host(Arc<Inner>);

struct Inner {
    origin: RwLock<Option<String>>,
    sessions: Mutex<HashMap<String, SessionRecord>>,
    toolkits: Vec<Arc<dyn Toolkit>>,
}

struct SessionRecord {
    grant: crate::auth::McpVerifier,
    thread_id: String,
}

/// Internal host capability adapter. Providers never see this seam; the MCP
/// host selects toolkits and supplies the trusted conversation identity.
pub trait Toolkit: fmt::Debug + Send + Sync {
    fn tools(&self) -> Vec<Value>;
    fn call<'a>(
        &'a self,
        thread_id: &'a str,
        name: &'a str,
        arguments: Value,
    ) -> Pin<Box<dyn Future<Output = Result<Value, String>> + Send + 'a>>;
}

impl fmt::Debug for Host {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("McpHost").finish_non_exhaustive()
    }
}

impl Host {
    pub fn new() -> Self {
        Self(Arc::new(Inner {
            origin: RwLock::new(None),
            sessions: Mutex::new(HashMap::new()),
            toolkits: Vec::new(),
        }))
    }

    pub fn with_toolkits(toolkits: Vec<Arc<dyn Toolkit>>) -> Self {
        Self(Arc::new(Inner {
            origin: RwLock::new(None),
            sessions: Mutex::new(HashMap::new()),
            toolkits,
        }))
    }

    fn set_host_origin(&self, origin: impl Into<String>) {
        *self.0.origin.write().expect("MCP origin lock") = Some(origin.into());
    }

    pub fn open(&self, thread_id: &str) -> Result<Session, OpenError> {
        self.open_session(thread_id)
    }

    pub fn authorizes(&self, endpoint: &str, authorization: &str) -> bool {
        let id = endpoint
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or_default();
        <Self as Platform>::authorizes(self, id, authorization)
    }

    async fn dispatch_message(&self, id: &str, message: Value) -> Value {
        let request_id = message.get("id").cloned().unwrap_or(Value::Null);
        if message.get("method").and_then(Value::as_str) != Some("tools/call") {
            return dispatch_with_tools(
                message,
                self.0
                    .toolkits
                    .iter()
                    .flat_map(|toolkit| toolkit.tools())
                    .collect(),
            );
        }
        let Some(name) = message.pointer("/params/name").and_then(Value::as_str) else {
            return rpc_error(request_id, -32602, "Invalid tool name");
        };
        let arguments = message
            .pointer("/params/arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let thread_id = self
            .0
            .sessions
            .lock()
            .expect("MCP sessions lock")
            .get(id)
            .map(|record| record.thread_id.clone());
        let Some(thread_id) = thread_id else {
            return rpc_error(request_id, -32602, "Unknown tool");
        };
        for toolkit in &self.0.toolkits {
            if toolkit
                .tools()
                .iter()
                .any(|tool| tool.get("name").and_then(Value::as_str) == Some(name))
            {
                return match toolkit.call(&thread_id, name, arguments).await {
                    Ok(content) => {
                        json!({"jsonrpc":"2.0","id":request_id,"result":{"content":content,"isError":false}})
                    }
                    Err(message) => {
                        json!({"jsonrpc":"2.0","id":request_id,"result":{"content":[{"type":"text","text":message}],"isError":true}})
                    }
                };
            }
        }
        rpc_error(request_id, -32602, "Unknown tool")
    }
}

impl Default for Host {
    fn default() -> Self {
        Self::new()
    }
}

impl Platform for Host {
    fn open_session(&self, thread_id: &str) -> Result<Session, OpenError> {
        let origin = self
            .0
            .origin
            .read()
            .expect("MCP origin lock")
            .clone()
            .ok_or(OpenError)?;
        let id = random_hex()?;
        let (grant, verifier) = crate::auth::mint_mcp_grant().map_err(|_| OpenError)?;
        self.0.sessions.lock().expect("MCP sessions lock").insert(
            id.clone(),
            SessionRecord {
                grant: verifier,
                thread_id: thread_id.to_string(),
            },
        );
        Ok(Session {
            endpoint: format!("{origin}/mcp/{id}"),
            authorization: grant,
            revoke: Some(Box::new({
                let owner = Arc::clone(&self.0);
                move || {
                    owner
                        .sessions
                        .lock()
                        .expect("MCP sessions lock")
                        .remove(&id);
                }
            })),
        })
    }

    fn set_origin(&self, origin: String) {
        self.set_host_origin(origin);
    }

    fn authorizes(&self, id: &str, authorization: &str) -> bool {
        self.0
            .sessions
            .lock()
            .expect("MCP sessions lock")
            .get(id)
            .is_some_and(|record| record.grant.verifies(authorization))
    }

    fn live_sessions(&self) -> usize {
        self.0.sessions.lock().expect("MCP sessions lock").len()
    }

    fn dispatch<'a>(
        &'a self,
        id: &'a str,
        message: Value,
    ) -> Pin<Box<dyn Future<Output = Value> + Send + 'a>> {
        Box::pin(self.dispatch_message(id, message))
    }
}

pub struct Session {
    endpoint: String,
    authorization: crate::auth::McpGrant,
    revoke: Option<Box<dyn FnOnce() + Send + Sync>>,
}

impl Session {
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }
    pub fn authorization(&self) -> &str {
        self.authorization.expose()
    }

    #[doc(hidden)]
    pub fn for_adapter(
        endpoint: String,
        authorization: String,
        revoke: impl FnOnce() + Send + Sync + 'static,
    ) -> Self {
        Self {
            endpoint,
            authorization: crate::auth::McpGrant::for_adapter(authorization),
            revoke: Some(Box::new(revoke)),
        }
    }
}

impl fmt::Debug for Session {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("McpSession")
            .field("endpoint", &self.endpoint)
            .field("authorization", &self.authorization)
            .finish()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Some(revoke) = self.revoke.take() {
            revoke();
        }
    }
}

fn random_hex() -> Result<String, OpenError> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| OpenError)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// Dispatch the stateless MCP 2025-06-18 subset owned by the platform.
pub fn dispatch(message: Value) -> Value {
    dispatch_with_tools(message, Vec::new())
}

fn dispatch_with_tools(message: Value, tools: Vec<Value>) -> Value {
    let id = message.get("id").cloned().unwrap_or(Value::Null);
    let method = message
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let result = match method {
        "initialize" => Some(json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities":{"tools":{}},
            "serverInfo":{"name":"laplus","version":env!("CARGO_PKG_VERSION")}
        })),
        "tools/list" => Some(json!({"tools":tools})),
        "tools/call" => {
            return json!({"jsonrpc":"2.0","id":id,"error":{
                "code":-32602,"message":"Unknown tool"
            }})
        }
        "notifications/initialized" => return Value::Null,
        _ => None,
    };
    match result {
        Some(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
        None => {
            json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Method not found"}})
        }
    }
}

fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_session_grant_is_scoped_redacted_and_revoked_with_its_handle() {
        let host = Host::new();
        host.set_origin("http://127.0.0.1:4773".to_string());
        let session = host.open("thread-1").expect("session opens");
        let endpoint = session.endpoint().to_string();
        let authorization = session.authorization().to_string();

        assert!(host.authorizes(&endpoint, &authorization));
        assert!(!host.authorizes(&endpoint, "Bearer wrong"));
        let debug = format!("{session:?}");
        assert!(!debug.contains(&authorization));
        assert!(debug.contains("[redacted]"));

        drop(session);
        assert!(!host.authorizes(&endpoint, &authorization));
    }

    #[test]
    fn protocol_initializes_then_lists_the_empty_toolkit_registry() {
        let initialized = dispatch(json!({
            "jsonrpc":"2.0","id":1,"method":"initialize","params":{
                "protocolVersion":"2025-11-25","capabilities":{},
                "clientInfo":{"name":"test","version":"1"}
            }
        }));
        assert_eq!(
            initialized["result"]["protocolVersion"],
            json!("2025-06-18")
        );
        assert_eq!(initialized["result"]["capabilities"], json!({"tools":{}}));

        assert_eq!(
            dispatch(json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}})),
            json!({"jsonrpc":"2.0","id":2,"result":{"tools":[]}})
        );
        assert_eq!(
            dispatch(
                json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"missing","arguments":{}}})
            )["error"]["code"],
            json!(-32602)
        );
    }
}
