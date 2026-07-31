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
}
