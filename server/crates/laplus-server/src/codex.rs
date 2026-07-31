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

use serde_json::Value;

use crate::codex_protocol::{self as protocol, Incoming, Request};
use crate::config::{CodexSettings, ProviderAuth, ProviderModel};
use crate::config_store::ProviderProcessLifetime;

const RESPONSE_TIMEOUT: Duration = Duration::from_secs(10);
const CANCELLATION_POLL: Duration = Duration::from_millis(25);
pub struct Snapshot {
    pub version: Option<String>,
    pub auth: ProviderAuth,
    pub models: Vec<ProviderModel>,
    pub skills: Vec<Value>,
}

pub(crate) fn probe(
    binary: &Path,
    settings: &CodexSettings,
    roots: &[PathBuf],
    lifetime: &ProviderProcessLifetime,
) -> Result<Snapshot, String> {
    let _active_process = lifetime.begin()?;
    let cwd = roots
        .first()
        .cloned()
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));
    let mut client = Client::start(binary, settings, &cwd, lifetime.clone())?;

    let version = protocol::decode_initialize(client.request(Request::Initialize)?)?;
    client.write(&protocol::initialized())?;

    let account_id = client.send_request(Request::Account)?;
    let models_id = client.send_request(Request::Models { cursor: None })?;
    let cwds: Vec<String> = match roots.is_empty() {
        true => vec![cwd.display().to_string()],
        false => roots
            .iter()
            .map(|root| root.display().to_string())
            .collect(),
    };
    let skills_id = client.send_request(Request::Skills { cwds })?;

    let auth = protocol::decode_account(client.wait(account_id)?)?;
    let skills = protocol::decode_skills(client.wait(skills_id)?)?;
    let mut page = protocol::decode_models(client.wait(models_id)?)?;
    let mut models = Vec::new();
    loop {
        models.extend(page.models);
        let cursor = page.next_cursor;
        let Some(cursor) = cursor else { break };
        page = protocol::decode_models(
            client.request(Request::Models {
                cursor: Some(cursor),
            })?,
        )?;
    }
    protocol::append_custom_models(&mut models, &settings.custom_models);

    Ok(Snapshot {
        version: Some(version),
        auth,
        models,
        skills,
    })
}

struct Client {
    child: Child,
    stdin: Option<ChildStdin>,
    output: mpsc::Receiver<String>,
    pending: HashMap<u64, String>,
    responses: HashMap<u64, Result<Value, String>>,
    next_id: u64,
    stderr: Arc<Mutex<Option<String>>>,
    lifetime: ProviderProcessLifetime,
}

impl Client {
    fn start(
        binary: &Path,
        settings: &CodexSettings,
        cwd: &Path,
        lifetime: ProviderProcessLifetime,
    ) -> Result<Client, String> {
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
            lifetime,
        })
    }

    fn request(&mut self, request: Request) -> Result<Value, String> {
        let id = self.send_request(request)?;
        self.wait(id)
    }

    fn send_request(&mut self, request: Request) -> Result<u64, String> {
        let id = self.next_id;
        self.next_id += 1;
        self.write(&request.message(id))?;
        self.pending.insert(id, request.method().to_string());
        Ok(id)
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
            if self.lifetime.is_cancelled() {
                return Err("Codex provider probe was cancelled during server shutdown".to_string());
            }
            if let Some(response) = self.responses.remove(&wanted) {
                return response;
            }
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .unwrap_or_default();
            let wait = remaining.min(CANCELLATION_POLL);
            let line = match self.output.recv_timeout(wait) {
                Ok(line) => line,
                Err(mpsc::RecvTimeoutError::Timeout) if wait < remaining => continue,
                Err(error) => return Err(self.wait_error(wanted, error)),
            };
            match protocol::decode_incoming(&line)? {
                // A method plus an id is app-server asking us something. Its id
                // is independent from, and never looked up in, `pending`.
                Incoming::Request { id, method } => {
                    self.write(&protocol::unsupported_request(&id, &method))?;
                }
                Incoming::Notification => {}
                Incoming::Response { id, result } => {
                    if self.pending.remove(&id).is_some() {
                        self.responses.insert(id, result);
                    }
                }
            }
        }
    }

    fn wait_error(&self, wanted: u64, error: mpsc::RecvTimeoutError) -> String {
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
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        drop(self.stdin.take());
        crate::process::terminate_tree_and_wait(&mut self.child);
    }
}
