//! Pure wire vocabulary and SSE decoder for OpenCode's narrow protocol subset.

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct StructuredError {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub data: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Health {
    pub healthy: bool,
    pub version: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Session {
    pub id: String,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct EventEnvelope {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub properties: Value,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum OpenCodeEvent {
    Known(EventEnvelope),
    Unknown(EventEnvelope),
}

impl OpenCodeEvent {
    pub fn envelope(&self) -> &EventEnvelope {
        match self {
            Self::Known(event) | Self::Unknown(event) => event,
        }
    }

    pub fn is_unknown(&self) -> bool {
        matches!(self, Self::Unknown(_))
    }
}

const KNOWN_EVENTS: &[&str] = &[
    "server.connected",
    "session.created",
    "session.updated",
    "session.deleted",
    "session.status",
    "session.idle",
    "session.error",
    "message.updated",
    "message.part.updated",
    "message.part.delta",
    "message.part.removed",
    "permission.asked",
    "permission.updated",
    "permission.replied",
    "question.asked",
    "question.replied",
    "question.rejected",
];

#[derive(Debug, Clone, PartialEq)]
pub enum SseDecodeError {
    InvalidUtf8,
    MalformedField(String),
    MalformedJson(String),
    TruncatedRecord,
}

impl fmt::Display for SseDecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

#[derive(Default)]
pub struct SseDecoder {
    pending: Vec<u8>,
    data: Vec<String>,
    saw_record_field: bool,
}

impl SseDecoder {
    pub fn push(&mut self, chunk: &[u8]) -> Vec<Result<OpenCodeEvent, SseDecodeError>> {
        self.pending.extend_from_slice(chunk);
        let mut answers = Vec::new();
        while let Some(newline) = self.pending.iter().position(|byte| *byte == b'\n') {
            let mut line = self.pending.drain(..=newline).collect::<Vec<_>>();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            let line = match String::from_utf8(line) {
                Ok(line) => line,
                Err(_) => {
                    answers.push(Err(SseDecodeError::InvalidUtf8));
                    continue;
                }
            };
            if line.is_empty() {
                if self.saw_record_field {
                    if !self.data.is_empty() {
                        answers.push(decode_event(&self.data.join("\n")));
                    }
                    self.data.clear();
                    self.saw_record_field = false;
                }
            } else if line.starts_with(':') {
                // Comment-only records are OpenCode's heartbeat traffic.
            } else if let Some(value) = line.strip_prefix("data:") {
                self.saw_record_field = true;
                self.data
                    .push(value.strip_prefix(' ').unwrap_or(value).to_owned());
            } else if line.starts_with("event:")
                || line.starts_with("id:")
                || line.starts_with("retry:")
            {
                self.saw_record_field = true;
            } else {
                answers.push(Err(SseDecodeError::MalformedField(line)));
            }
        }
        answers
    }

    pub fn finish(&self) -> Option<SseDecodeError> {
        (!self.pending.is_empty() || self.saw_record_field || !self.data.is_empty())
            .then_some(SseDecodeError::TruncatedRecord)
    }
}

fn decode_event(data: &str) -> Result<OpenCodeEvent, SseDecodeError> {
    let event: EventEnvelope =
        serde_json::from_str(data).map_err(|_| SseDecodeError::MalformedJson(data.into()))?;
    if KNOWN_EVENTS.contains(&event.kind.as_str()) {
        Ok(OpenCodeEvent::Known(event))
    } else {
        Ok(OpenCodeEvent::Unknown(event))
    }
}
