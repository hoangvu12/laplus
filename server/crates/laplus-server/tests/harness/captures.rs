//! Reading the ticket 01 captures.
//!
//! `fixtures/socket-wire/*.ndjson` is a recording of the reference TypeScript
//! server answering a real UI, one record per line. This turns a recording
//! back into the frames that crossed the wire, so a test can hold laplus's
//! answer next to the reference server's answer to the same call.

use std::path::{Path, PathBuf};

use serde_json::Value;

/// Which way a frame was going.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    ClientToServer,
    ServerToClient,
}

/// One recorded connection.
pub struct Capture {
    name: String,
    records: Vec<Value>,
}

impl Capture {
    /// Load by file stem, e.g. `"02-request-response"`.
    pub fn load(name: &str) -> Capture {
        let path = fixtures_dir().join(format!("{name}.ndjson"));
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()));

        let records = text
            .lines()
            .filter(|line| !line.trim().is_empty())
            .enumerate()
            .map(|(index, line)| {
                serde_json::from_str(line).unwrap_or_else(|error| {
                    panic!("{}:{}: {error}", path.display(), index + 1)
                })
            })
            .collect();

        Capture {
            name: name.to_string(),
            records,
        }
    }

    /// Every assembled WebSocket message, in order, parsed as JSON.
    pub fn frames(&self) -> impl Iterator<Item = (Direction, Value)> + '_ {
        self.records
            .iter()
            .filter(|record| record["type"] == "ws-message")
            .map(|record| {
                let direction = match record["dir"].as_str() {
                    Some("client-to-server") => Direction::ClientToServer,
                    Some("server-to-client") => Direction::ServerToClient,
                    other => panic!("unknown direction {other:?} in {}", self.name),
                };
                let text = record["text"]
                    .as_str()
                    .unwrap_or_else(|| panic!("a ws-message without text in {}", self.name));
                let frame = serde_json::from_str(text).unwrap_or_else(|error| {
                    panic!("a ws-message that is not json in {}: {error}", self.name)
                });
                (direction, frame)
            })
    }

    /// The reference server's terminal frame for the first call to `tag`.
    ///
    /// Finds the client's `Request`, takes its id, and returns the `Exit` that
    /// carries the same id — the same correlation a real client does, rather
    /// than trusting the order the recording happens to be in.
    pub fn response_to(&self, tag: &str) -> Value {
        let request_id = self.request_id_for(tag);

        self.frames()
            .find(|(direction, frame)| {
                *direction == Direction::ServerToClient
                    && frame["_tag"] == "Exit"
                    && frame["requestId"] == request_id
            })
            .map(|(_, frame)| frame)
            .unwrap_or_else(|| panic!("no exit for {tag} in {}", self.name))
    }

    /// Every `Chunk` the reference server sent for the first call to `tag`,
    /// in order, as whole frames.
    ///
    /// Correlated by `requestId` the same way [`Capture::response_to`] is,
    /// which matters more here than it does there: a subscription's chunks are
    /// interleaved with other calls' answers in `01-browser-session.ndjson`.
    pub fn chunks_to(&self, tag: &str) -> Vec<Value> {
        let request_id = self.request_id_for(tag);
        self.frames()
            .filter(|(direction, frame)| {
                *direction == Direction::ServerToClient
                    && frame["_tag"] == "Chunk"
                    && frame["requestId"] == request_id
            })
            .map(|(_, frame)| frame)
            .collect()
    }

    fn request_id_for(&self, tag: &str) -> Value {
        self.frames()
            .find(|(direction, frame)| {
                *direction == Direction::ClientToServer
                    && frame["_tag"] == "Request"
                    && frame["tag"] == tag
            })
            .map(|(_, frame)| frame["id"].clone())
            .unwrap_or_else(|| panic!("no request for {tag} in {}", self.name))
    }

    /// The first frame the server sent with this `_tag`.
    pub fn server_frame(&self, tag: &str) -> Value {
        self.frames()
            .find(|(direction, frame)| {
                *direction == Direction::ServerToClient && frame["_tag"] == tag
            })
            .map(|(_, frame)| frame)
            .unwrap_or_else(|| panic!("no {tag} frame in {}", self.name))
    }

    /// The body of an HTTP response — present only when the upgrade was
    /// refused, since a 101 has none.
    pub fn http_response_body(&self) -> Value {
        let text = self
            .records
            .iter()
            .find(|record| record["type"] == "http-response-body")
            .and_then(|record| record["text"].as_str())
            .unwrap_or_else(|| panic!("no http-response-body in {}", self.name));

        serde_json::from_str(text)
            .unwrap_or_else(|error| panic!("http body in {} is not json: {error}", self.name))
    }

    /// The status line of the HTTP response, e.g. `HTTP/1.1 401 Unauthorized`.
    pub fn http_status_line(&self) -> String {
        self.records
            .iter()
            .find(|record| record["type"] == "http-response")
            .and_then(|record| record["statusLine"].as_str())
            .unwrap_or_else(|| panic!("no http-response in {}", self.name))
            .to_string()
    }
}

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/socket-wire")
}
