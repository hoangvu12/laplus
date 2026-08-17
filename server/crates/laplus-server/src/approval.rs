//! Provider-neutral approval requests held by the shared session loop.

use serde::Serialize;
use serde_json::Value;

use crate::worklog::Decision;

/// What an agent has stopped to ask, independent of the wire it arrived on.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ApprovalRequest {
    pub request_id: String,
    pub tool_name: String,
    pub input: Value,
    pub tool_use_id: Option<String>,
    pub description: Option<String>,
    pub suggestions: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub available_decisions: Option<Vec<Decision>>,
    /// Provider-owned correlation data carried opaquely by the shared loop.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_request_id: Option<Value>,
    /// The delegated child that is waiting, when it is not the root agent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subagent: Option<Waiting>,
}

/// Which child stopped for this request.
///
/// Carried on the request rather than derived beside it, because the same value
/// has to reach three places that must agree: the row the developer answers in
/// the main conversation, the entry recorded in that child's own stream, and the
/// route the answer is sent back on. A request the provider does not attribute
/// to a child leaves this `None`, which is the root behaviour unchanged — laplus
/// does not invent ownership.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Waiting {
    /// The child's stream reference: what `orchestration.subscribeSubagent` and
    /// the compact row are both addressed by.
    pub child_id: String,
    /// Its semantic name or type, when the provider gave one.
    pub name: Option<String>,
}
