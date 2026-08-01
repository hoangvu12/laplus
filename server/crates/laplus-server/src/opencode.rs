//! HTTP ownership and event-stream lifetime for OpenCode.

use std::fmt;

use futures_util::StreamExt;
use reqwest::{Method, StatusCode, Url};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::Value;
use tokio::{
    sync::{mpsc, watch},
    task::JoinHandle,
};

use crate::opencode_protocol::{
    Health, OpenCodeEvent, Session, SseDecodeError, SseDecoder, StructuredError,
};

#[derive(Clone)]
pub struct OpenCodeClient {
    base_url: Url,
    directory: String,
    password: Option<String>,
    http: reqwest::Client,
}

impl fmt::Debug for OpenCodeClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OpenCodeClient")
            .field("base_url", &self.base_url)
            .field("directory", &self.directory)
            .field("password", &self.password.as_ref().map(|_| "[redacted]"))
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub enum OpenCodeError {
    InvalidBaseUrl(String),
    Transport(reqwest::Error),
    Authentication {
        status: StatusCode,
        error: Option<StructuredError>,
    },
    MissingSession {
        status: StatusCode,
        error: StructuredError,
    },
    Server {
        status: StatusCode,
        error: Option<StructuredError>,
        body: String,
    },
    MalformedJson {
        source: serde_json::Error,
        body: String,
    },
    MalformedSse(SseDecodeError),
    StreamClosed,
}

impl fmt::Display for OpenCodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidBaseUrl(value) => write!(formatter, "invalid OpenCode base URL: {value}"),
            Self::Transport(error) => write!(formatter, "OpenCode transport failed: {error}"),
            Self::Authentication { status, .. } => {
                write!(formatter, "OpenCode authentication failed ({status})")
            }
            Self::MissingSession { .. } => formatter.write_str("OpenCode session does not exist"),
            Self::Server { status, .. } => write!(formatter, "OpenCode request failed ({status})"),
            Self::MalformedJson { source, .. } => {
                write!(formatter, "OpenCode returned malformed JSON: {source}")
            }
            Self::MalformedSse(error) => {
                write!(formatter, "OpenCode returned malformed SSE: {error}")
            }
            Self::StreamClosed => formatter.write_str("OpenCode event stream closed"),
        }
    }
}

impl std::error::Error for OpenCodeError {}

impl OpenCodeClient {
    pub fn new(
        base_url: &str,
        directory: impl Into<String>,
        password: Option<String>,
    ) -> Result<Self, OpenCodeError> {
        let mut base_url =
            Url::parse(base_url).map_err(|_| OpenCodeError::InvalidBaseUrl(base_url.into()))?;
        if !matches!(base_url.scheme(), "http" | "https") || base_url.host().is_none() {
            return Err(OpenCodeError::InvalidBaseUrl(base_url.to_string()));
        }
        if !base_url.path().ends_with('/') {
            base_url.set_path(&format!("{}/", base_url.path()));
        }
        Ok(Self {
            base_url,
            directory: directory.into(),
            password,
            http: reqwest::Client::new(),
        })
    }

    pub async fn health(&self) -> Result<Health, OpenCodeError> {
        self.request_json(Method::GET, "global/health", Option::<&()>::None)
            .await
    }
    pub async fn providers(&self) -> Result<Value, OpenCodeError> {
        self.request_json(Method::GET, "provider", Option::<&()>::None)
            .await
    }
    pub async fn agents(&self) -> Result<Value, OpenCodeError> {
        self.request_json(Method::GET, "agent", Option::<&()>::None)
            .await
    }
    pub async fn create_session(&self, body: &Value) -> Result<Session, OpenCodeError> {
        self.request_json(Method::POST, "session", Some(body)).await
    }
    pub async fn session(&self, id: &str) -> Result<Session, OpenCodeError> {
        self.request_json(Method::GET, &format!("session/{id}"), Option::<&()>::None)
            .await
    }
    pub async fn prompt(&self, id: &str, body: &Value) -> Result<Value, OpenCodeError> {
        self.request_json(
            Method::POST,
            &format!("session/{id}/prompt_async"),
            Some(body),
        )
        .await
    }
    pub async fn abort(&self, id: &str) -> Result<Value, OpenCodeError> {
        self.request_json(
            Method::POST,
            &format!("session/{id}/abort"),
            Option::<&()>::None,
        )
        .await
    }
    pub async fn revert(&self, id: &str, body: &Value) -> Result<Value, OpenCodeError> {
        self.request_json(Method::POST, &format!("session/{id}/revert"), Some(body))
            .await
    }
    pub async fn reply_permission(&self, id: &str, body: &Value) -> Result<Value, OpenCodeError> {
        self.request_json(Method::POST, &format!("permission/{id}/reply"), Some(body))
            .await
    }
    pub async fn reply_question(&self, id: &str, body: &Value) -> Result<Value, OpenCodeError> {
        self.request_json(Method::POST, &format!("question/{id}/reply"), Some(body))
            .await
    }
    pub async fn reject_question(&self, id: &str) -> Result<Value, OpenCodeError> {
        self.request_json(
            Method::POST,
            &format!("question/{id}/reject"),
            Option::<&()>::None,
        )
        .await
    }

    pub async fn subscribe(&self) -> Result<EventStream, OpenCodeError> {
        let response = self
            .request(Method::GET, "event", Option::<&()>::None)
            .send()
            .await
            .map_err(OpenCodeError::Transport)?;
        let response = classify_response(response).await?;
        let (events_tx, events_rx) = mpsc::channel(32);
        let (cancel_tx, mut cancel_rx) = watch::channel(false);
        let task = tokio::spawn(async move {
            let mut chunks = response.bytes_stream();
            let mut decoder = SseDecoder::default();
            loop {
                tokio::select! {
                    changed = cancel_rx.changed() => if changed.is_ok() && *cancel_rx.borrow() { break },
                    chunk = chunks.next() => match chunk {
                        Some(Ok(bytes)) => for decoded in decoder.push(&bytes) {
                            if events_tx.send(decoded.map_err(OpenCodeError::MalformedSse)).await.is_err() { return; }
                        },
                        Some(Err(error)) => { let _ = events_tx.send(Err(OpenCodeError::Transport(error))).await; return; }
                        None => { if let Some(error) = decoder.finish() { let _ = events_tx.send(Err(OpenCodeError::MalformedSse(error))).await; } return; }
                    }
                }
            }
        });
        Ok(EventStream {
            events: events_rx,
            cancel: cancel_tx,
            task: Some(task),
            unknown_count: 0,
        })
    }

    fn request<B: Serialize + ?Sized>(
        &self,
        method: Method,
        path: &str,
        body: Option<&B>,
    ) -> reqwest::RequestBuilder {
        let mut url = self.base_url.join(path).expect("validated base URL");
        url.query_pairs_mut()
            .append_pair("directory", &self.directory);
        let mut request = self.http.request(method, url);
        if let Some(password) = &self.password {
            request = request.basic_auth("opencode", Some(password));
        }
        if let Some(body) = body {
            request = request.json(body);
        }
        request
    }

    async fn request_json<T: DeserializeOwned, B: Serialize + ?Sized>(
        &self,
        method: Method,
        path: &str,
        body: Option<&B>,
    ) -> Result<T, OpenCodeError> {
        let response = self
            .request(method, path, body)
            .send()
            .await
            .map_err(OpenCodeError::Transport)?;
        let bytes = classify_response(response)
            .await?
            .bytes()
            .await
            .map_err(OpenCodeError::Transport)?;
        serde_json::from_slice(&bytes).map_err(|source| OpenCodeError::MalformedJson {
            source,
            body: String::from_utf8_lossy(&bytes).into_owned(),
        })
    }
}

async fn classify_response(
    response: reqwest::Response,
) -> Result<reqwest::Response, OpenCodeError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let bytes = response.bytes().await.map_err(OpenCodeError::Transport)?;
    let body = String::from_utf8_lossy(&bytes).into_owned();
    let error = serde_json::from_slice::<StructuredError>(&bytes).ok();
    if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
        return Err(OpenCodeError::Authentication { status, error });
    }
    if status == StatusCode::NOT_FOUND && error.as_ref().is_some_and(is_missing_session) {
        return Err(OpenCodeError::MissingSession {
            status,
            error: error.expect("checked"),
        });
    }
    Err(OpenCodeError::Server {
        status,
        error,
        body,
    })
}

fn is_missing_session(error: &StructuredError) -> bool {
    error.name.as_deref() == Some("NotFoundError")
        && error
            .data
            .as_ref()
            .and_then(|value| value.get("message"))
            .and_then(Value::as_str)
            .or(error.message.as_deref())
            .is_some_and(|message| message.to_ascii_lowercase().contains("session"))
}

pub struct EventStream {
    events: mpsc::Receiver<Result<OpenCodeEvent, OpenCodeError>>,
    cancel: watch::Sender<bool>,
    task: Option<JoinHandle<()>>,
    unknown_count: u64,
}

impl EventStream {
    pub async fn next(&mut self) -> Result<OpenCodeEvent, OpenCodeError> {
        match self.events.recv().await {
            Some(Ok(event)) => {
                if event.is_unknown() {
                    self.unknown_count += 1;
                }
                Ok(event)
            }
            Some(Err(error)) => Err(error),
            None => Err(OpenCodeError::StreamClosed),
        }
    }
    pub fn unknown_count(&self) -> u64 {
        self.unknown_count
    }
    pub async fn cancel(mut self) {
        let _ = self.cancel.send(true);
        if let Some(task) = self.task.take() {
            task.abort();
            let _ = task.await;
        }
    }
}

impl Drop for EventStream {
    fn drop(&mut self) {
        let _ = self.cancel.send(true);
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}
