//! The `codex app-server` transport for provider probes and conversations.
//!
//! Responses on this wire omit `jsonrpc` and may arrive in any order, while
//! requests sent by app-server use an independent id space. Classification is
//! therefore by shape first and client responses are correlated only through
//! the ids this client has in `pending`.
//!
//! Codex labels agent messages `commentary` before tool use and `final_answer`
//! afterwards. Both are published as transcript messages: they are agent-authored
//! prose addressed to the developer, while the work log is reserved for what the
//! agent did. Keeping the item ids separate preserves Codex's phase boundary
//! without inventing an activity kind that the other driver does not produce.
//!
//! ## A subagent is a thread
//!
//! Codex delegates by starting another thread and narrating it on the same wire.
//! So a child's prose, commands and turn boundaries arrive as the ordinary
//! `item/*` and `turn/*` notifications, distinguished only by their `threadId` —
//! and everything about this driver's subagent support follows from routing on
//! that one field ([`ConversationState::fold_notification`]):
//!
//! - **A child's events never touch the parent conversation.** They become
//!   [`crate::subagents`] updates on that child's own stream. A child's
//!   `turn/completed` in particular must not settle the root's turn, and the
//!   root's idle-terminal fallback must not see a child going idle.
//! - **The operation and the agent stay two rows.** A `spawnAgent` that
//!   completed has finished spawning; the agent it started has not finished
//!   anything. [`collaboration_call_row`] draws the first and
//!   [`collaboration_agent_row`] the second, and only the second carries the
//!   `childId` that opens a work stream.
//! - **Identity is the child's thread id and hierarchy is its canonical path.**
//!   Both are Codex's own, and neither is inferred — see [`Children`].

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader as AsyncBufReader};
use tokio::process::{Child as AsyncChild, ChildStdin as AsyncChildStdin, Command as AsyncCommand};
use tokio::sync::mpsc as async_mpsc;

use crate::approval::ApprovalRequest;
use crate::codex_protocol::{
    self as protocol, Access, Capabilities, ChildEvent, ChildNotification, CollaborationAgent,
    CollaborationCall, CommandExecution, ConversationFold, ConversationState, Incoming, Request,
    SubagentActivity,
};
use crate::config::{CodexSettings, ProviderAuth, ProviderModel};
use crate::config_store::ProviderProcessLifetime;
use crate::protocol::Drift;
use crate::session::{
    Decided, Driver, Driving, Finished, Opened, Pushed, Reaped, Reply, Settles, Start,
};
use crate::settling::SessionStatus;
use crate::threads::{Activity, Change};

fn turn_input(text: &str, attachments: &[crate::threads::PromptAttachment]) -> Result<Vec<protocol::TurnInput>, String> {
    let mut input = vec![protocol::TurnInput::Text { text: text.to_string() }];
    for attachment in attachments {
        input.push(protocol::TurnInput::Image { url: crate::attachments::data_url(attachment)? });
    }
    Ok(input)
}

pub(crate) async fn generate_title(instance: &crate::provider::CodexInstance, directory: &str, model: Option<&str>, prompt: String, attachments: &[crate::threads::PromptAttachment], timeout: Duration) -> Result<Value, String> {
    let (binary, _) = crate::provider::resolve_codex(&instance.settings.binary_path, &crate::process::Search::from_environment()).startable_codex()?;
    let mut server = AppServer::start(&binary, &instance.settings, Path::new(directory)).await.map_err(|error| error.to_string())?;
    let generated = async {
        protocol::decode_initialize(server.request(Request::Initialize).await?)?;
        server.write(&protocol::initialized()).await?;
        let access = Access::for_runtime_mode("full-access")?;
        let thread = server.request(Request::ThreadStart { cwd: directory.to_string(), model: model.map(str::to_string), access }).await?;
        let thread_id = protocol::decode_thread_start(thread)?;
        let input = turn_input(&prompt, attachments)?;
        server.send_request(Request::TurnStart { thread_id, input, model: model.map(str::to_string), access: Some(access) }).await?;
        let mut answer = None;
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let line = tokio::time::timeout_at(deadline, server.next_line()).await.map_err(|_| "Codex title generation timed out.".to_string())?.ok_or_else(|| "Codex stopped during title generation.".to_string())?;
            let Ok(value) = serde_json::from_str::<Value>(&line) else { continue };
            if value["method"] == "item/completed" && value["params"]["item"]["type"] == "agentMessage" {
                answer = value["params"]["item"]["text"].as_str().map(str::to_string);
            }
            if value["method"] == "turn/completed" {
                let terminal_answer = value["params"]["turn"]["items"].as_array().and_then(|items| items.iter().rev().find(|item| item["type"] == "agentMessage")).and_then(|item| item["text"].as_str()).map(str::to_string);
                let raw = terminal_answer.or(answer).ok_or_else(|| "Codex title generation returned no message.".to_string())?;
                return serde_json::from_str(&raw).map_err(|_| format!("Codex returned malformed structured title text: {raw}"));
            }
        }
    }.await;
    server.stop().await;
    generated
}

const RESPONSE_TIMEOUT: Duration = Duration::from_secs(10);
const STARTUP_RESPONSE_TIMEOUT: Duration = Duration::from_secs(30);
const CANCELLATION_POLL: Duration = Duration::from_millis(25);
const OUTPUT_QUEUE: usize = 256;
const EXIT_GRACE: Duration = Duration::from_secs(2);
const EXIT_POLL: Duration = Duration::from_millis(20);

// Invoke configured PowerShell scripts without a cmd.exe shim holding duplicate
// protocol handles between laplus and the app-server.
fn command_for(binary: &Path) -> Command {
    #[cfg(windows)]
    if binary
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("ps1"))
    {
        let mut command = Command::new("powershell.exe");
        command
            .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
            .arg(binary);
        return command;
    }
    Command::new(binary)
}

fn async_command_for(binary: &Path) -> AsyncCommand {
    #[cfg(windows)]
    if binary
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("ps1"))
    {
        let mut command = AsyncCommand::new("powershell.exe");
        command
            .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
            .arg(binary);
        return command;
    }
    AsyncCommand::new(binary)
}

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

/// One long-lived `codex app-server` behind one conversation.
pub(crate) struct Codex {
    app_server: AppServer,
    folding: ConversationState,
    /// The delegated children this conversation has seen, and the hierarchy
    /// their canonical paths prove. See [`Children`].
    children: Children,
    thread_id: String,
    active_turn_id: Option<String>,
    turn_id_unavailable: bool,
    model: Option<String>,
    access: Access,
    explicit_turn_config: bool,
    capabilities: Capabilities,
}

fn resume_thread(start: &Start) -> Result<Option<String>, String> {
    let Some(cursor) = &start.resume_cursor else {
        return Ok(None);
    };
    if let Some(thread_id) = cursor.value.as_str().filter(|id| !id.is_empty()) {
        return Ok(Some(thread_id.to_string()));
    }
    if cursor.value.as_object().map(serde_json::Map::len) != Some(2) {
        return Err("The stored Codex continuation is incompatible with this build.".to_string());
    }
    let version = cursor.value.get("version").and_then(serde_json::Value::as_u64);
    let thread_id = cursor.value.get("threadId").and_then(serde_json::Value::as_str);
    match (version, thread_id) {
        (Some(1), Some(thread_id)) if !thread_id.is_empty() => Ok(Some(thread_id.to_string())),
        (Some(version), _) if version > 1 => Err(format!(
            "Codex continuation version {version} is newer than this build supports."
        )),
        _ => Err("The stored Codex continuation is incompatible with this build.".to_string()),
    }
}

fn resume_cursor(start: &Start, thread_id: &str) -> crate::provider::ResumeCursor {
    crate::provider::ResumeCursor {
        provider: start.provider.clone(),
        value: serde_json::json!({"version": 1, "threadId": thread_id}),
    }
}

impl Driver for Codex {
    async fn open(start: &Start) -> Result<Opened<Codex>, String> {
        let resume = resume_thread(start)?;
        let settings = start.driver.codex()?;
        let capabilities = Capabilities::current();
        let (binary, _) = crate::provider::resolve_codex(
            &settings.binary_path,
            &crate::process::Search::from_environment(),
        )
        .startable_codex()?;
        let mut app_server = AppServer::start(
            &binary,
            settings,
            Path::new(&start.workspace_root),
        )
        .await
        .map_err(|error| {
            format!(
                "The Codex binary {} could not be started in {}: {error}",
                binary.display(),
                start.workspace_root
            )
        })?;

        let access = match Access::for_runtime_mode(&start.runtime_mode) {
            Ok(access) => access,
            Err(error) => {
                app_server.stop().await;
                return Err(error);
            }
        };
        let opened = async {
            let initialized = app_server.request(Request::Initialize).await?;
            protocol::decode_initialize(initialized)?;
            app_server.write(&protocol::initialized()).await?;
            match &resume {
                Some(resume) => {
                    match app_server
                        .request(Request::ThreadResume {
                            thread_id: resume.clone(),
                            access,
                        })
                        .await
                        .and_then(protocol::decode_thread_start)
                    {
                        Ok(thread_id) => Ok((thread_id, None)),
                        Err(error) => {
                            let thread = app_server
                                .request(Request::ThreadStart {
                                    cwd: start.workspace_root.clone(),
                                    model: start.model.clone(),
                                    access,
                                })
                                .await?;
                            let thread_id = protocol::decode_thread_start(thread)?;
                            Ok((thread_id, Some(resume_failed(resume, &error))))
                        }
                    }
                }
                None => {
                    let thread = app_server
                        .request(Request::ThreadStart {
                            cwd: start.workspace_root.clone(),
                            model: start.model.clone(),
                            access,
                        })
                        .await?;
                    protocol::decode_thread_start(thread).map(|thread_id| (thread_id, None))
                }
            }
        }
        .await;
        let (thread_id, resume_failure) = match opened {
            Ok(opened) => opened,
            Err(error) => {
                app_server.stop().await;
                return Err(error);
            }
        };
        let changes = resume_failure
            .map(|why| Change::Activity(Activity::failed("session.resume-failed", &why)))
            .into_iter()
            .collect();

        Ok(Opened {
            driver: Codex {
                app_server,
                folding: ConversationState::for_thread(thread_id.clone()),
                children: Children::default(),
                thread_id: thread_id.clone(),
                active_turn_id: None,
                turn_id_unavailable: false,
                model: start.model.clone(),
                access,
                // `thread/resume` has no model field. Restate the complete
                // configuration on its first turn as well as after a retune.
                explicit_turn_config: resume.is_some(),
                capabilities,
            },
            decided: Decided {
                changes,
                provider_resume_cursor: Some(resume_cursor(start, &thread_id)),
                ..Decided::default()
            },
        })
    }

    async fn next(&mut self, driving: &mut Driving) -> Option<Decided> {
        let line = self.app_server.next_line().await?;
        let observed = self.app_server.observe(&line);
        let folded = match observed {
            Observed::Notification => self.folding.fold_line(&line),
            Observed::Response {
                method,
                result: Ok(result),
                ..
            } if method == "turn/start" => {
                driving.pushed.clear();
                let folded = self.folding.fold_turn_start_response(result);
                self.turn_id_unavailable = !matches!(folded, ConversationFold::TurnStarted { .. });
                folded
            }
            Observed::Response {
                method,
                result: Err(error),
                ..
            } if method == "turn/start" => {
                driving.pushed.clear();
                self.active_turn_id = None;
                self.turn_id_unavailable = false;
                return Some(settle(
                    driving,
                    Ending::Failed(error),
                    None,
                    self.folding.drift(),
                ));
            }
            Observed::Response {
                method,
                result: Ok(_),
                turn_id: Some(interrupted_turn),
            } if method == "turn/interrupt"
                && self.active_turn_id.as_deref() == Some(interrupted_turn.as_str()) =>
            {
                self.active_turn_id = None;
                return Some(settle_interrupted(driving, self.folding.drift()));
            }
            Observed::Response {
                method,
                result: Err(error),
                turn_id: Some(interrupted_turn),
            } if method == "turn/interrupt"
                && self.active_turn_id.as_deref() == Some(interrupted_turn.as_str()) =>
            {
                let Some(active) = driving.turn.as_mut() else {
                    return Some(Decided::default());
                };
                if !active.was_stopped() {
                    return Some(Decided::default());
                }
                active.carries_on();
                return Some(Decided {
                    changes: vec![Change::Activity(Activity::failed(
                        "turn.interrupt-failed",
                        &format!("Codex would not stop the turn: {error}"),
                    ))],
                    ..Decided::default()
                });
            }
            Observed::Malformed => self.folding.fold_line(&line),
            Observed::Request => self.folding.fold_line(&line),
            Observed::Response { .. } => ConversationFold::Nothing,
        };

        let idle = matches!(
            &folded,
            ConversationFold::ThreadStatus { status } if status == "idle"
        );
        let stale_completion = match (&folded, self.active_turn_id.as_deref()) {
            (ConversationFold::TurnCompleted(completion), Some(active_turn_id)) => {
                active_turn_id != completion.turn_id
            }
            (ConversationFold::TurnCompleted(_), None) => !self.turn_id_unavailable,
            _ => false,
        };
        if stale_completion {
            return Some(Decided::default());
        }
        match &folded {
            ConversationFold::TurnStarted { turn_id } => {
                self.active_turn_id = Some(turn_id.clone());
                self.turn_id_unavailable = false;
            }
            ConversationFold::TurnCompleted(_) => {
                self.active_turn_id = None;
                self.turn_id_unavailable = false;
            }
            _ => {}
        }
        let drift = self.folding.drift();
        let decided = decide(folded, driving, drift, &mut self.children);
        // Empty capabilities promise the completion that carries the outcome,
        // so idle cannot win that race. Experimental API omits completion; the
        // handshake policy flips this fallback with it.
        if idle
            && self.capabilities.idle_is_terminal()
            && driving.turn.is_some()
            && decided.settles.is_none()
        {
            self.active_turn_id = None;
            return Some(settle(driving, Ending::Completed, None, drift));
        }
        Some(decided)
    }

    async fn send(&mut self, prompt: &crate::threads::Prompt) -> std::io::Result<()> {
        let input = turn_input(&prompt.text, &prompt.attachments).map_err(std::io::Error::other)?;
        self.turn_id_unavailable = false;
        let (model, access) = match self.explicit_turn_config {
            true => (self.model.clone(), Some(self.access)),
            false => (None, None),
        };
        self.app_server
            .send_request(Request::TurnStart {
                thread_id: self.thread_id.clone(),
                input,
                model,
                access,
            })
            .await
            .map(|_| ())
            .map_err(std::io::Error::other)
    }

    async fn interrupt(&mut self, _request_id: &str) -> std::io::Result<()> {
        let turn_id = self
            .active_turn_id
            .clone()
            .ok_or_else(|| std::io::Error::other("Codex has not started the turn yet"))?;
        self.app_server
            .send_request(Request::TurnInterrupt {
                thread_id: self.thread_id.clone(),
                turn_id,
            })
            .await
            .map(|_| ())
            .map_err(std::io::Error::other)
    }

    async fn answer(&mut self, asked: &ApprovalRequest, reply: Reply<'_>) -> std::io::Result<()> {
        let Reply::Decided(decision) = reply else {
            return Err(std::io::Error::other("Codex did not ask an approval question"));
        };
        let response_id = asked
            .provider_request_id
            .as_ref()
            .ok_or_else(|| std::io::Error::other("Codex approval lost its JSON-RPC request id"))?;
        if !asked
            .available_decisions
            .as_ref()
            .is_some_and(|offered| offered.contains(&decision))
        {
            return Err(std::io::Error::other(format!(
                "Codex did not offer the '{}' approval decision",
                decision.as_str()
            )));
        }
        self.app_server
            .write(&protocol::approval_response(response_id, decision.as_str()))
            .await
            .map_err(std::io::Error::other)
    }

    async fn measure(&mut self, _request_id: &str) -> std::io::Result<()> {
        Ok(())
    }

    async fn retune(&mut self, _request_id: &str, asked: &Pushed) -> std::io::Result<()> {
        match asked {
            Pushed::Model { asked, .. } => {
                self.model = Some(asked.clone());
            }
            Pushed::Mode { asked, .. } => {
                self.access = Access::for_runtime_mode(asked).map_err(std::io::Error::other)?;
            }
        }
        self.explicit_turn_config = true;
        Ok(())
    }

    fn close_input(&mut self) {
        self.app_server.close_input();
    }

    async fn stop(self, driving: &mut Driving, asked_to_stop: bool) -> Reaped {
        let complaint = self.app_server.stop().await;
        let death = (driving.turn.is_some() && !asked_to_stop).then(|| match complaint {
            Some(complaint) => format!(
                "Codex stopped before the turn finished. The agent said: {complaint}"
            ),
            None => "Codex stopped before the turn finished.".to_string(),
        });
        Reaped {
            refused: None,
            death,
        }
    }
}

fn resume_failed(thread_id: &str, error: &str) -> String {
    format!(
        "Codex could not resume thread {thread_id}, so it started a fresh thread. The previous \
         context is no longer available to the agent. The resume failed with: {error}"
    )
}

fn decide(
    folded: ConversationFold,
    driving: &mut Driving,
    drift: Drift,
    children: &mut Children,
) -> Decided {
    let mut decided = Decided::default();
    match folded {
        ConversationFold::Nothing
        | ConversationFold::ThreadStarted { .. }
        | ConversationFold::TurnStarted { .. } => {}
        ConversationFold::TitleUpdated { title } => {
            decided.changes.push(Change::MetaUpdated(crate::threads::MetaUpdate {
                title: Some(title),
                title_regeneration: Some(None),
                regenerate_title: false,
                previous_title: None,
                model_selection: None,
                branch: None,
                worktree_path: None,
            }));
        }
        ConversationFold::ThreadStatus { .. } => {}
        ConversationFold::ReasoningStarted { .. } => {
            let turn_id = driving.turn.as_ref().map(|turn| turn.turn_id.clone());
            decided
                .changes
                .push(Change::Activity(crate::worklog::thinking_started(turn_id)));
        }
        ConversationFold::ReasoningDelta { .. } => {}
        ConversationFold::ReasoningCompleted { text, .. } => {
            let turn_id = driving.turn.as_ref().map(|turn| turn.turn_id.clone());
            if let Some(thinking) = crate::worklog::thinking(&text, turn_id) {
                decided.changes.push(Change::Activity(thinking));
            }
        }
        // Commentary and final answers deliberately share the transcript path;
        // the module-level policy explains why the phase is not an activity kind.
        ConversationFold::AssistantDelta { text, .. } => {
            let Some(active) = driving.turn.as_mut() else {
                return decided;
            };
            let message_id = active
                .assistant_message_id
                .get_or_insert_with(crate::threads::fresh_message_id)
                .clone();
            decided.changes.push(Change::AssistantDelta {
                message_id,
                turn_id: active.turn_id.clone(),
                text,
            });
        }
        ConversationFold::AssistantCompleted { text, .. } => {
            let Some(active) = driving.turn.as_mut() else {
                return decided;
            };
            let message_id = active
                .assistant_message_id
                .take()
                .unwrap_or_else(crate::threads::fresh_message_id);
            decided.changes.push(Change::AssistantMessage {
                message_id,
                turn_id: active.turn_id.clone(),
                text,
            });
        }
        ConversationFold::CommandStarted(command) => {
            let turn_id = driving.turn.as_ref().map(|turn| turn.turn_id.clone());
            decided
                .changes
                .push(Change::Activity(command_call(&command).invoked(turn_id)));
        }
        ConversationFold::CommandCompleted(command) => {
            let turn_id = driving.turn.as_ref().map(|turn| turn.turn_id.clone());
            decided.changes.push(Change::Activity(
                command_call(&command).command_returned(
                    crate::worklog::CommandReturned {
                        output: command.aggregated_output.as_deref().unwrap_or_default(),
                        status: &command.status,
                        exit_code: command.exit_code,
                        duration_ms: command.duration_ms,
                    },
                    turn_id,
                )));
        }
        ConversationFold::CollaborationStarted(call) => {
            let turn_id = driving.turn.as_ref().map(|turn| turn.turn_id.clone());
            decided
                .changes
                .push(Change::Activity(collaboration_call_row(
                    &call, false, turn_id,
                )));
            children.operated(&call, false, &mut decided);
        }
        ConversationFold::CollaborationCompleted(call) => {
            let turn_id = driving.turn.as_ref().map(|turn| turn.turn_id.clone());
            decided
                .changes
                .push(Change::Activity(collaboration_call_row(
                    &call,
                    true,
                    turn_id.clone(),
                )));
            children.operated(&call, true, &mut decided);
            // **Every agent this call names is registered before any row is
            // drawn**, and the two passes are not a style choice.
            // [`Children::parent_of`] resolves a path against the paths it
            // already holds, so a `wait` that listed a descendant ahead of its
            // spawner would draw the descendant a root row — published, and
            // never retractable — purely because of the order Codex happened to
            // report its fleet in.
            for agent in &call.agents {
                children.reported(agent, &mut decided);
            }
            for agent in &call.agents {
                // A descendant of another child is launched from inside that
                // child's stream, so it gets no row here — see
                // [`Children::nested`]. The `wait` that reports on the whole
                // fleet is where an agent Codex has proven belongs to another
                // agent is most likely to be seen from the root for the first
                // time.
                if children.nested(&agent.thread_id) {
                    continue;
                }
                decided
                    .changes
                    .push(Change::Activity(collaboration_agent_row(
                        agent,
                        children.latest_of(&agent.thread_id),
                        turn_id.clone(),
                    )));
            }
        }
        ConversationFold::SubagentActivity(activity) => {
            let turn_id = driving.turn.as_ref().map(|turn| turn.turn_id.clone());
            children.acted(&activity, &mut decided);
            if !children.nested(&activity.agent_thread_id) {
                let latest = children.latest_of(&activity.agent_thread_id);
                decided
                    .changes
                    .push(Change::Activity(subagent_activity_row(
                        &activity, latest, turn_id,
                    )));
            }
        }
        // A descendant, announced by the child that launched it. Its identity,
        // its canonical path and the parentage that path proves are recorded;
        // its launcher belongs inside the spawning child's stream rather than in
        // the root transcript, which is ticket 06's placement to make.
        ConversationFold::NestedSubagentActivity(activity) => {
            children.acted(&activity, &mut decided);
        }
        ConversationFold::SubagentObserved(agent) => {
            let turn_id = driving.turn.as_ref().map(|turn| turn.turn_id.clone());
            children.reported(&agent, &mut decided);
            if !children.nested(&agent.thread_id) {
                let latest = children
                    .latest_of(&agent.thread_id)
                    .or(agent.path.as_deref());
                decided
                    .changes
                    .push(Change::Activity(collaboration_agent_row(
                        &agent, latest, turn_id,
                    )));
            }
        }
        // A child's own prose and work. It updates that child's stream and
        // nothing else: the parent transcript keeps one compact row per child,
        // and a descendant's turn boundary is not the root's.
        ConversationFold::SubagentWorked(work) => {
            children.worked(&work, &mut decided);
        }
        ConversationFold::ApprovalRequested(request) => {
            let turn_id = driving.turn.as_ref().map(|turn| turn.turn_id.clone());
            // Command executions have their own item lifecycle. The other two
            // approval methods do not yet have a rendered lifecycle, so publish
            // their call here before publishing the panel that asks about it.
            if request.request_kind != "command" {
                decided
                    .changes
                    .push(Change::Activity(request.call().invoked(turn_id.clone())));
            }
            let asked = request.permission();
            decided
                .changes
                .push(Change::Activity(crate::worklog::requested(&asked, turn_id)));
            driving.outstanding.insert(asked.request_id.clone(), asked);
        }
        ConversationFold::TokenUsage(reading) => {
            if let Some(usage) = driving.usage_to_report(Some(reading)) {
                let turn_id = driving.turn.as_ref().map(|turn| turn.turn_id.clone());
                decided
                    .changes
                    .push(Change::Activity(crate::turn::context_window_row(&usage, turn_id)));
            }
        }
        ConversationFold::TurnCompleted(completion) => {
            let ending = match (
                driving.turn.as_ref().is_some_and(|turn| turn.was_stopped()),
                completion.error,
            ) {
                (true, _) => Ending::Stopped,
                (false, Some(error)) => Ending::Failed(error),
                (false, None) => Ending::Completed,
            };
            return settle(driving, ending, completion.duration_ms, drift);
        }
    }
    decided
}

fn command_call(command: &CommandExecution) -> crate::worklog::Call {
    crate::worklog::Call {
        id: command.id.clone(),
        name: "Command".to_string(),
        input: serde_json::json!({
            "command": command.command,
            "cwd": command.cwd,
            "processId": command.process_id,
        }),
    }
}

fn collaboration_call_row(
    call: &CollaborationCall,
    ended: bool,
    turn_id: Option<String>,
) -> Activity {
    let (verb, past) = operation_words(&call.tool);
    // The same decision the child's own entry records, rendered as the client's
    // `toolLifecycleStatus` literal. Deciding it twice is how the row and the
    // entry would come to disagree about one operation.
    let status = operation_progress(call, ended).as_str();
    let title = if ended { past } else { verb };
    let mut payload = serde_json::json!({
        "itemType": "collab_agent_tool_call",
        "status": status,
        "title": title,
        "data": {
            "toolCallId": call.id,
            "operation": call.tool,
            "senderThreadId": call.sender_thread_id,
            "receiverThreadIds": call.receiver_thread_ids,
            "protocol": call.raw,
        },
    });
    if let Some(prompt) = call.prompt.as_deref() {
        payload["detail"] = Value::String(worklog_preview(prompt));
    }
    Activity::tool(
        if ended {
            "tool.completed"
        } else {
            "tool.updated"
        },
        title,
        payload,
        turn_id,
    )
}

fn collaboration_agent_row(
    agent: &CollaborationAgent,
    detail: Option<&str>,
    turn_id: Option<String>,
) -> Activity {
    let status = match agent.status.as_str() {
        "pendingInit" | "running" => "inProgress",
        "completed" => "completed",
        "errored" | "notFound" => "failed",
        "interrupted" | "shutdown" => "stopped",
        _ => "failed",
    };
    // Named where Codex says the name, and identified where it does not. The
    // other two drivers title this row with the agent that ran — "Subagent
    // Explore", "Subagent explore" — and a developer reading a work log should
    // not have to know which agent they are talking to in order to read it.
    // Codex's own name for a subagent is the last segment of its `agentPath`;
    // the thread id is what is left when there is no path, and it is an
    // identifier rather than a name, so it is used only as a fallback.
    let title = match agent.path.as_deref().and_then(agent_name) {
        Some(name) => format!("Subagent {name}"),
        None => format!("Subagent {}", short_agent_id(&agent.thread_id)),
    };
    // **A terminal row describes what came back, and nothing else.** Once an
    // agent has reported, whatever it was last doing is stale, and a row that
    // went on showing it would present the thing said on the way to an answer as
    // the answer — the spec's "terminal state replaces stale activity". A
    // completion with nothing to read says nothing rather than falling back,
    // because there is no stale line it would be honest to show instead.
    //
    // While it runs, the most useful line wins: what the agent itself reported,
    // then the latest meaningful thing its own stream saw it do, and its
    // canonical path last — identity, which is what is left when there is no
    // activity yet.
    let ended = conclusion(&agent.status, agent.message.as_deref());
    let shown_detail = match &ended {
        Some(outcome) => outcome.text.clone(),
        None => agent
            .message
            .clone()
            .or_else(|| detail.map(str::to_string)),
    };
    let mut payload = serde_json::json!({
        "itemType": "collab_agent_tool_call",
        "status": status,
        "title": title,
        "data": {
            "toolCallId": format!("agent:{}", agent.thread_id),
            "agentThreadId": agent.thread_id,
            "agentStatus": agent.status,
            "message": agent.message,
            // Codex's canonical path, kept verbatim on the row that names the
            // agent: it is the ancestry Codex proves, and the identity its own
            // picker prefers over a UUID.
            "agentPath": agent.path,
            // The stream reference this row launches. A child's Codex thread id
            // *is* its child id — durable, unique, and the same id every later
            // event about it carries — so the row and
            // `orchestration.subscribeSubagent` cannot disagree about who was
            // clicked. Only this row carries it: the operation row beside it
            // ([`collaboration_call_row`]) deliberately does not, because a
            // spawn or a wait is not the child.
            "childId": agent.thread_id,
        },
    });
    if let Some(detail) = shown_detail {
        payload["detail"] = Value::String(worklog_preview(&detail));
    }
    Activity::tool(
        if status == "inProgress" {
            "tool.updated"
        } else {
            "tool.completed"
        },
        &title,
        payload,
        turn_id,
    )
}

fn subagent_activity_row(
    activity: &SubagentActivity,
    latest: Option<&str>,
    turn_id: Option<String>,
) -> Activity {
    let status = if activity.kind == "interrupted" {
        "interrupted"
    } else {
        "running"
    };
    let agent = CollaborationAgent {
        thread_id: activity.agent_thread_id.clone(),
        status: status.to_string(),
        message: None,
        path: Some(activity.agent_path.clone()),
    };
    let mut row = collaboration_agent_row(&agent, latest.or(Some(&activity.agent_path)), turn_id);
    row.payload["data"]["activity"] = activity.raw.clone();
    row
}

/// Every Codex child this conversation has seen, and what its protocol proved
/// about it.
///
/// **A child's identity is its Codex thread id.** Codex runs a subagent as a
/// thread of its own, so the id is durable, unique for the life of the
/// conversation, and carried by every later event about that agent — the
/// activity that starts it, the collaboration call that waits on it, and the
/// child's own items. That is what makes it the child id the work stream is
/// keyed by and the row's launcher points at.
///
/// **The canonical path is what proves hierarchy.** `/root/reviewer/helper` says
/// `helper` was launched by `reviewer`, and Codex's own picker prefers that path
/// over a UUID for the same reason. This resolves the parent *path* back to the
/// thread id of the agent that holds it, and produces nothing at all when no
/// known agent holds it: a hierarchy laplus cannot prove is one it must not
/// draw. `/root/reviewer` names an agent the conversation itself launched, whose
/// parent is the conversation rather than another child, and that is `None` too.
#[derive(Debug, Default)]
struct Children {
    by_thread: HashMap<String, Subagent>,
}

#[derive(Debug, Default)]
struct Subagent {
    /// The canonical agent path, once a `subAgentActivity` has named it.
    path: Option<String>,
    /// The latest meaningful thing this child did, for the compact row.
    ///
    /// One line, from the child's own stream rather than from the parent's
    /// description of it: what the row has room for is what the developer would
    /// most want to have waited for. A turn boundary is not activity and does
    /// not move it, which is the spec's rule about heartbeats and partial
    /// states applied to the one protocol event that is purely structural.
    latest: Option<String>,
}

impl Children {
    /// This child, and everything already known about it, as the update every
    /// caller below starts from.
    ///
    /// **How long a reported child stays closed is not this adapter's question
    /// any more.** It used to be: a `concluded` flag and the set of entry keys a
    /// closed child would still accept lived here, which made the rule
    /// in-memory — a restart lost it — while the stream's own conclusion is
    /// restored from disk. `crate::subagents::Streams::record` owns it now, for
    /// every provider and across a restart, so this simply forwards what Codex
    /// said and lets the stream decide what it may still be told.
    fn about(&mut self, thread_id: &str, path: Option<&str>) -> crate::subagents::Update {
        let child = self.by_thread.entry(thread_id.to_string()).or_default();
        if let Some(path) = path.map(str::trim).filter(|path| !path.is_empty()) {
            child.path = Some(path.to_string());
        }
        let path = child.path.clone();
        let mut update = crate::subagents::Update::for_child(thread_id)
            .named(path.as_deref().and_then(agent_name).map(str::to_string));
        update.parent_child_id = path.as_deref().and_then(|path| self.parent_of(path));
        update
    }

    /// Does a *child* hold this agent's parent path?
    ///
    /// The question the root transcript asks before drawing a compact row: a
    /// descendant Codex proves belongs to another child is shown inside that
    /// child's stream and nowhere else, so the developer sees one worker with
    /// one visible parent. Unproven parentage answers `false` and keeps the
    /// truthful root behaviour.
    fn nested(&self, thread_id: &str) -> bool {
        self.by_thread
            .get(thread_id)
            .and_then(|child| child.path.as_deref())
            .and_then(|path| self.parent_of(path))
            .is_some()
    }

    /// The thread id of the agent whose canonical path is this path's parent.
    ///
    /// **Exactly one, or none.** Two live agents holding one canonical path
    /// would make the parent ambiguous, and picking either — the lowest id, the
    /// first the map happened to yield — would present a coin toss as proof.
    /// The honesty rule is the same one that governs an unknown ancestor:
    /// laplus draws the relationship the provider proves, or none.
    fn parent_of(&self, path: &str) -> Option<String> {
        let segments = path_segments(path);
        // The first segment is the conversation's own root. A path of root plus
        // one name is a child of the conversation, and it has no parent *child*.
        if segments.len() < 3 {
            return None;
        }
        let ancestor = &segments[..segments.len() - 1];
        let mut holders = self.by_thread.iter().filter(|(_, child)| {
            child
                .path
                .as_deref()
                .is_some_and(|known| path_segments(known) == ancestor)
        });
        let (thread_id, _) = holders.next()?;
        match holders.next() {
            Some(_) => None,
            None => Some(thread_id.clone()),
        }
    }

    /// The latest meaningful thing this child was seen to do.
    fn latest_of(&self, thread_id: &str) -> Option<&str> {
        self.by_thread.get(thread_id)?.latest.as_deref()
    }

    /// Remember what the compact row should say while this child runs.
    ///
    /// Only the child's **own** events move this. A spawn, a wait or an input
    /// sent to it are the parent acting on the child, and a row that answered
    /// "what is this subagent doing?" with "its parent waited for it" would be
    /// describing the wrong agent. They are entries in the child's history all
    /// the same — they belong to it — but they are not its activity.
    fn did(&mut self, thread_id: &str, said: &str) {
        let said = said.trim();
        if said.is_empty() {
            return;
        }
        self.by_thread
            .entry(thread_id.to_string())
            .or_default()
            .latest = Some(said.to_string());
    }

    /// A collaboration operation, in the stream of every child it concerns.
    ///
    /// **The operation is not the child.** A `spawnAgent` that completed has
    /// finished spawning; the agent it started is only beginning. So this records
    /// what the parent did as one piece of work in the child's history and
    /// touches neither the child's state nor its outcome — the only things that
    /// end a child are the child's own turn and an agent state that says so.
    ///
    /// One entry per receiver, in each receiver's own stream. A `wait` over three
    /// agents is three independent histories, and they must not fold into each
    /// other.
    fn operated(&mut self, call: &CollaborationCall, ended: bool, decided: &mut Decided) {
        let (verb, past) = operation_words(&call.tool);
        let work = crate::subagents::Work {
            title: if ended { past } else { verb }.to_string(),
            status: operation_progress(call, ended),
            detail: call.prompt.clone(),
            command: None,
            paths: Vec::new(),
            query: None,
        };
        // The agents it names in either field. A completed call can report a
        // state for an agent its `receiverThreadIds` never listed.
        let mut receivers = call.receiver_thread_ids.clone();
        for agent in &call.agents {
            if !receivers.contains(&agent.thread_id) {
                receivers.push(agent.thread_id.clone());
            }
        }
        let key = format!("collab:{}", call.id);
        for receiver in receivers {
            let update = self.about(&receiver, None);
            // What the parent asked *this* child for, and only on the call that
            // asked it. A later `wait` carries no prompt, and an assignment
            // absent from the protocol stays absent.
            let update = match (call.tool.as_str(), call.prompt.as_deref()) {
                ("spawnAgent", Some(prompt)) if !prompt.trim().is_empty() => {
                    update.assigned(Some(prompt.to_string()))
                }
                _ => update,
            };
            decided.child_streams.push(update.with(crate::subagents::NewEntry::worked(
                Some(key.clone()),
                crate::subagents::EntryKind::Tool,
                &work,
            )));
        }
    }

    /// A `subAgentActivity`: the agent it names, the path it proves, and the
    /// interaction itself as one entry of that agent's history.
    fn acted(&mut self, activity: &SubagentActivity, decided: &mut Decided) {
        let key = format!("activity:{}", activity.id);
        let update = self.about(&activity.agent_thread_id, Some(&activity.agent_path));
        let title = match activity.kind.as_str() {
            "started" => "Subagent started",
            "interacted" => "Input sent to the subagent",
            "interrupted" => "Subagent interrupted",
            _ => "Subagent activity",
        };
        let update = update.with(crate::subagents::NewEntry::worked(
            Some(key.clone()),
            crate::subagents::EntryKind::Tool,
            &crate::subagents::Work {
                title: title.to_string(),
                status: crate::subagents::Progress::Completed,
                detail: Some(activity.agent_path.clone()),
                command: None,
                paths: Vec::new(),
                query: None,
            },
        ));
        match conclusion(&activity.kind, None) {
            Some(outcome) => decided.child_streams.push(update.concluded(outcome)),
            None => decided
                .child_streams
                .push(update.in_state(crate::subagents::State::Working)),
        }
    }

    /// An agent state Codex reported — from a collaboration call's map, or from
    /// the child's own completed turn.
    fn reported(&mut self, agent: &CollaborationAgent, decided: &mut Decided) {
        let update = self.about(&agent.thread_id, agent.path.as_deref());
        match conclusion(&agent.status, agent.message.as_deref()) {
            Some(outcome) => decided.child_streams.push(update.concluded(outcome)),
            None => decided
                .child_streams
                .push(update.in_state(agent_state(&agent.status))),
        }
    }

    /// Something a child's own thread said or did.
    fn worked(&mut self, work: &ChildNotification, decided: &mut Decided) {
        let key = match &work.event {
            ChildEvent::Working => None,
            ChildEvent::Said { item_id, .. } => Some(format!("item:{item_id}")),
            ChildEvent::Ran(command) => Some(format!("item:{}", command.id)),
        };
        let update = self
            .about(&work.thread_id, work.path.as_deref())
            .in_state(crate::subagents::State::Working);
        match &work.event {
            // A turn boundary is structure rather than progress, and the spec
            // asks for the latest *meaningful* activity.
            ChildEvent::Working => {}
            ChildEvent::Said { text, .. } => self.did(&work.thread_id, text),
            ChildEvent::Ran(command) => self.did(&work.thread_id, &command.command),
        }
        decided.child_streams.push(match &work.event {
            ChildEvent::Working => update,
            ChildEvent::Said { text, .. } => {
                update.with(crate::subagents::NewEntry::said(key.clone(), text))
            }
            ChildEvent::Ran(command) => update.with(crate::subagents::NewEntry::worked(
                key.clone(),
                crate::subagents::EntryKind::Command,
                &command_work(command),
            )),
        });
    }
}

fn path_segments(path: &str) -> Vec<&str> {
    path.split('/')
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .collect()
}

/// How a collaboration operation is going. One decision, read by the operation's
/// own row and by the entry it leaves in each child's history.
fn operation_progress(call: &CollaborationCall, ended: bool) -> crate::subagents::Progress {
    match (ended, call.status.as_str()) {
        (false, _) => crate::subagents::Progress::InProgress,
        (true, "failed") => crate::subagents::Progress::Failed,
        (true, _) => crate::subagents::Progress::Completed,
    }
}

/// What laplus calls each of Codex's five collaboration operations, running and
/// finished.
fn operation_words(tool: &str) -> (&'static str, &'static str) {
    match tool {
        "spawnAgent" => ("Starting subagent", "Spawned subagent"),
        "sendInput" => ("Sending input to subagent", "Sent input to subagent"),
        "resumeAgent" => ("Resuming subagent", "Resumed subagent"),
        "wait" => ("Waiting for subagents", "Waited for subagents"),
        "closeAgent" => ("Closing subagent", "Closed subagent"),
        _ => ("Running subagent operation", "Subagent operation finished"),
    }
}

/// Where a child is, for the states that are not an ending.
fn agent_state(status: &str) -> crate::subagents::State {
    match status {
        "pendingInit" => crate::subagents::State::Pending,
        _ => crate::subagents::State::Working,
    }
}

/// How a child ended, for the Codex statuses that end one.
///
/// **Codex distinguishes more endings than the shared vocabulary has room for.**
/// `errored` and `notFound` are both failures, and `interrupted` and `shutdown`
/// are both interruptions, so each pair lands on one [`crate::subagents::OutcomeKind`].
/// The distinction is not thrown away with it: where Codex gave no message of its
/// own, the outcome says which of its states this was, so a child's history can
/// still tell "it broke" from "its thread was gone" and "you stopped it" from
/// "it was shut down". Codex's own word also stays verbatim on the compact row,
/// as `data.agentStatus`.
fn conclusion(status: &str, message: Option<&str>) -> Option<crate::subagents::Outcome> {
    let said = message
        .map(str::trim)
        .filter(|message| !message.is_empty())
        .map(str::to_string);
    let or = |sentence: &str| Some(said.clone().unwrap_or_else(|| sentence.to_string()));
    Some(match status {
        // A completion with nothing to read is an *empty* outcome rather than an
        // invented sentence, which is the one case a stand-in would destroy.
        "completed" => crate::subagents::Outcome::completed(said),
        "errored" => crate::subagents::Outcome::failed(or("Codex reported this subagent as errored.")),
        "notFound" => {
            crate::subagents::Outcome::failed(or("Codex could not find this subagent's thread."))
        }
        "interrupted" => crate::subagents::Outcome::interrupted(or(
            "Codex reported this subagent as interrupted.",
        )),
        "shutdown" => {
            crate::subagents::Outcome::interrupted(or("Codex shut this subagent down."))
        }
        _ => return None,
    })
}

/// A command a child ran, in the vocabulary the main agent's work rows speak.
fn command_work(command: &CommandExecution) -> crate::subagents::Work {
    // A non-zero exit is a failure whatever the item says, which is the half of
    // `worklog::Call::command_returned`'s rule that transfers. The other half —
    // "anything but completed failed" — does not: that rule is applied to a
    // *finished* command, and this folds the running item as well as the
    // finished one, where `inProgress` means running rather than failed.
    let failed = command.status == "failed"
        || command.exit_code.is_some_and(|code| code != 0);
    let status = match (command.status.as_str(), failed) {
        (_, true) => crate::subagents::Progress::Failed,
        ("completed", false) => crate::subagents::Progress::Completed,
        _ => crate::subagents::Progress::InProgress,
    };
    crate::subagents::Work {
        title: "Command".to_string(),
        status,
        // Kept as the process wrote it. A command that printed only whitespace
        // said nothing, which is absence rather than a blank line to draw.
        detail: command
            .aggregated_output
            .as_deref()
            .filter(|output| !output.trim().is_empty())
            .map(crate::subagents::bounded),
        command: Some(command.command.clone()),
        paths: Vec::new(),
        query: None,
    }
}

/// The agent's own name, out of the path Codex identifies it by.
///
/// `"/root/compute_sum"` is `compute_sum` — the capture in
/// `fixtures/codex-app-server/09-subagent-spawn.jsonl` is where that shape comes
/// from. Trailing separators are tolerated rather than trusted, and a path with
/// no segment in it at all is `None` so the caller can fall back to the id.
fn agent_name(path: &str) -> Option<&str> {
    path.rsplit('/')
        .find(|segment| !segment.trim().is_empty())
        .map(str::trim)
}

fn short_agent_id(thread_id: &str) -> &str {
    let end = thread_id
        .char_indices()
        .nth(8)
        .map(|(index, _)| index)
        .unwrap_or(thread_id.len());
    &thread_id[..end]
}

fn worklog_preview(value: &str) -> String {
    const LIMIT: usize = 180;
    if value.chars().count() <= LIMIT {
        return value.to_string();
    }
    let kept: String = value.chars().take(LIMIT - 3).collect();
    format!("{kept}...")
}

enum Ending {
    Completed,
    Failed(String),
    Stopped,
}

impl Ending {
    fn failed(&self) -> bool {
        matches!(self, Ending::Failed(_))
    }

    fn stopped(&self) -> bool {
        matches!(self, Ending::Stopped)
    }

    fn summary(&self, duration_ms: Option<u64>) -> String {
        match self {
            Ending::Failed(error) => format!("Turn failed. Codex said: {error}"),
            Ending::Stopped => "Turn stopped by the developer.".to_string(),
            Ending::Completed => match duration_ms {
                Some(duration) => {
                    format!("Turn completed in {:.1}s.", duration as f64 / 1_000.0)
                }
                None => "Turn completed.".to_string(),
            },
        }
    }

    fn session_status(&self) -> SessionStatus {
        match self {
            Ending::Completed => SessionStatus::Ready,
            Ending::Failed(_) => SessionStatus::Error,
            Ending::Stopped => SessionStatus::Interrupted,
        }
    }

    fn checkpoint_status(&self) -> Option<&'static str> {
        match self {
            Ending::Completed => Some("ready"),
            Ending::Failed(_) => Some("error"),
            Ending::Stopped => None,
        }
    }
}

fn settle(
    driving: &mut Driving,
    ending: Ending,
    duration_ms: Option<u64>,
    total_drift: Drift,
) -> Decided {
    let Some(finished) = driving.turn.take() else {
        return Decided::default();
    };
    let turn_id = finished.turn_id;
    let failed = ending.failed();
    if let Some(status) = ending.checkpoint_status() {
        driving.finished = Some(Finished {
            turn_id: turn_id.clone(),
            status,
        });
    }

    let drift = driving.drift_to_report(total_drift);
    let mut summary = ending.summary(duration_ms);
    if let Some(drifted) = drift_clause(drift) {
        summary.push_str(&format!(" · {drifted}"));
    }
    let mut activity = Activity::info(
        "turn.completed",
        &summary,
        serde_json::json!({
            "durationMs": duration_ms,
            "totalCostUsd": Value::Null,
            "isError": failed,
            "interrupted": ending.stopped(),
            "unknownEvents": total_drift.unknown_events,
            "parseErrors": total_drift.parse_errors,
        }),
        Some(turn_id.clone()),
    );
    if failed {
        activity.tone = "error";
    }

    Decided {
        changes: vec![Change::Activity(activity)],
        settles: Some(Settles {
            turn_id: Some(turn_id),
            status: ending.session_status(),
            last_error: failed.then_some(summary),
        }),
        ..Decided::default()
    }
}

fn settle_interrupted(driving: &mut Driving, drift: Drift) -> Decided {
    let Some(active) = driving.turn.as_mut() else {
        return Decided::default();
    };
    if !active.was_stopped() {
        return Decided::default();
    }
    let closing = active
        .assistant_message_id
        .take()
        .map(|message_id| Change::AssistantMessage {
            message_id,
            turn_id: active.turn_id.clone(),
            // Codex sends no authoritative message after an interrupt. Empty
            // text closes the stream while preserving its accumulated deltas.
            text: String::new(),
        });
    let mut decided = settle(driving, Ending::Stopped, None, drift);
    if let Some(closing) = closing {
        decided.changes.insert(0, closing);
    }
    decided
}

fn drift_clause(drift: Drift) -> Option<String> {
    if drift.is_clean() {
        return None;
    }
    let mut said = Vec::new();
    if drift.unknown_events > 0 {
        let events = if drift.unknown_events == 1 { "event" } else { "events" };
        said.push(format!("{} unrecognised {events}", drift.unknown_events));
    }
    if drift.parse_errors > 0 {
        let lines = if drift.parse_errors == 1 { "line" } else { "lines" };
        said.push(format!("{} unreadable {lines}", drift.parse_errors));
    }
    Some(said.join(" and "))
}

struct AppServer {
    child: AsyncChild,
    stdin: Option<AsyncChildStdin>,
    output: async_mpsc::Receiver<String>,
    deferred: VecDeque<String>,
    pending: HashMap<u64, Pending>,
    next_id: u64,
    complaint: Arc<Mutex<Option<String>>>,
    stderr: Option<tokio::task::JoinHandle<()>>,
}

impl AppServer {
    async fn start(
        binary: &Path,
        settings: &CodexSettings,
        cwd: &Path,
    ) -> std::io::Result<AppServer> {
        let launch_args = shell_words::split(&settings.launch_args).map_err(|error| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, error.to_string())
        })?;
        let mut command = async_command_for(binary);
        command
            .arg("app-server")
            .args(launch_args)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if !settings.home_path.trim().is_empty() {
            command.env(
                "CODEX_HOME",
                crate::projects::expand_home(settings.home_path.trim()),
            );
        }
        crate::process::without_a_console(command.as_std_mut());
        let mut child = command.spawn()?;
        let stdin = child.stdin.take().ok_or_else(missing_async_pipe)?;
        let stdout = child.stdout.take().ok_or_else(missing_async_pipe)?;
        let child_stderr = child.stderr.take().ok_or_else(missing_async_pipe)?;

        let (lines, output) = async_mpsc::channel(OUTPUT_QUEUE);
        tokio::spawn(async move {
            let mut reader = AsyncBufReader::new(stdout).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                if lines.send(line).await.is_err() {
                    return;
                }
            }
        });

        let complaint = Arc::new(Mutex::new(None));
        let latest = Arc::clone(&complaint);
        let stderr = tokio::spawn(async move {
            let mut reader = AsyncBufReader::new(child_stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                eprintln!("laplus: codex: {line}");
                if !line.trim().is_empty() {
                    *latest
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                        Some(line.trim().to_string());
                }
            }
        });

        Ok(AppServer {
            child,
            stdin: Some(stdin),
            output,
            deferred: VecDeque::new(),
            pending: HashMap::new(),
            next_id: 1,
            complaint,
            stderr: Some(stderr),
        })
    }

    async fn request(&mut self, request: Request) -> Result<Value, String> {
        let method = request.method();
        let timeout = if matches!(&request, Request::Initialize) {
            STARTUP_RESPONSE_TIMEOUT
        } else {
            RESPONSE_TIMEOUT
        };
        let id = self.send_request(request).await?;
        loop {
            let line = tokio::time::timeout(timeout, self.output.recv())
                .await
                .map_err(|_| format!("Codex stopped answering {method}"))?
                .ok_or_else(|| format!("Codex stopped before answering {method}"))?;
            match protocol::decode_incoming(&line) {
                Ok(Incoming::Request { id, method, .. }) => {
                    self.write(&protocol::unsupported_request(&id, &method)).await?;
                    self.deferred.push_back(line);
                }
                Ok(Incoming::Notification) | Err(_) => self.deferred.push_back(line),
                Ok(Incoming::Response { id: response_id, result }) => {
                    if self.pending.remove(&response_id).is_none() {
                        self.deferred.push_back(line);
                        continue;
                    }
                    if response_id == id {
                        return result;
                    }
                }
            }
        }
    }

    async fn send_request(&mut self, request: Request) -> Result<u64, String> {
        let id = self.next_id;
        self.next_id += 1;
        let pending = Pending {
            method: request.method().to_string(),
            turn_id: match &request {
                Request::TurnInterrupt { turn_id, .. } => Some(turn_id.clone()),
                _ => None,
            },
        };
        self.write(&request.message(id)).await?;
        self.pending.insert(id, pending);
        Ok(id)
    }

    async fn write(&mut self, message: &Value) -> Result<(), String> {
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| "Codex stdin is closed".to_string())?;
        let mut line = message.to_string();
        line.push('\n');
        stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|error| format!("Codex request could not be written: {error}"))?;
        stdin
            .flush()
            .await
            .map_err(|error| format!("Codex request could not be written: {error}"))
    }

    async fn next_line(&mut self) -> Option<String> {
        if let Some(line) = self.deferred.pop_front() {
            return Some(line);
        }
        self.output.recv().await
    }

    fn observe(&mut self, line: &str) -> Observed {
        match protocol::decode_incoming(line) {
            Ok(Incoming::Notification) => Observed::Notification,
            Ok(Incoming::Request { .. }) => Observed::Request,
            Ok(Incoming::Response { id, result }) => match self.pending.remove(&id) {
                Some(pending) => Observed::Response {
                    method: pending.method,
                    turn_id: pending.turn_id,
                    result,
                },
                None => Observed::Response {
                    method: String::new(),
                    turn_id: None,
                    result,
                },
            },
            Err(_) => Observed::Malformed,
        }
    }

    fn close_input(&mut self) {
        drop(self.stdin.take());
    }

    async fn stop(mut self) -> Option<String> {
        drop(self.stdin.take());
        let deadline = tokio::time::Instant::now() + EXIT_GRACE;
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => return self.last_words().await,
                Err(_) => break,
                Ok(None) if tokio::time::Instant::now() >= deadline => break,
                Ok(None) => tokio::time::sleep(EXIT_POLL).await,
            }
        }

        #[cfg(windows)]
        if let Some(pid) = self.child.id() {
            let mut command = AsyncCommand::new("taskkill.exe");
            command
                .args(["/PID", &pid.to_string(), "/T", "/F"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            crate::process::without_a_console(command.as_std_mut());
            let _ = command.status().await;
        }
        let _ = self.child.kill().await;
        let _ = self.child.wait().await;
        self.last_words().await
    }

    async fn last_words(&mut self) -> Option<String> {
        if let Some(stderr) = self.stderr.take() {
            let _ = tokio::time::timeout(EXIT_GRACE, stderr).await;
        }
        self.complaint
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

enum Observed {
    Notification,
    Request,
    Response {
        method: String,
        turn_id: Option<String>,
        result: Result<Value, String>,
    },
    Malformed,
}

struct Pending {
    method: String,
    turn_id: Option<String>,
}

fn missing_async_pipe() -> std::io::Error {
    std::io::Error::other("Codex was started without one of its stdio pipes")
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
        let mut command = command_for(binary);
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
                Incoming::Request { id, method, .. } => {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_completed_spawn_and_its_running_agent_are_separate_rows() {
        let call = CollaborationCall {
            id: "call-1".to_string(),
            tool: "spawnAgent".to_string(),
            status: "completed".to_string(),
            sender_thread_id: "parent-thread".to_string(),
            receiver_thread_ids: vec!["child-thread".to_string()],
            prompt: Some("Review the decoder.".to_string()),
            agents: Vec::new(),
            raw: serde_json::json!({"type": "collabAgentToolCall"}),
        };
        let operation = collaboration_call_row(&call, true, Some("turn-1".to_string()));
        let agent = collaboration_agent_row(
            &CollaborationAgent {
                thread_id: "child-thread".to_string(),
                status: "running".to_string(),
                message: None,
                path: None,
            },
            None,
            Some("turn-1".to_string()),
        );

        assert_eq!(operation.kind, "tool.completed");
        assert_eq!(operation.payload["status"], "completed");
        assert_eq!(operation.payload["data"]["toolCallId"], "call-1");
        assert_eq!(agent.kind, "tool.updated");
        assert_eq!(agent.payload["status"], "inProgress");
        assert_eq!(agent.payload["data"]["toolCallId"], "agent:child-thread");
    }

    /// A subagent row says which agent ran, the way it does under the other two
    /// drivers — `Subagent reviewer` rather than `Subagent 019fc927`. The name is
    /// the last segment of the `agentPath` Codex identifies it by, and the
    /// truncated thread id is what is left when there is no path: an identifier,
    /// and only a fallback.
    #[test]
    fn a_subagent_row_is_named_after_the_agent_that_ran() {
        let named = collaboration_agent_row(
            &CollaborationAgent {
                thread_id: "019fc9277b067c43af761ab6f2126876".to_string(),
                status: "running".to_string(),
                message: None,
                path: Some("/root/compute_sum".to_string()),
            },
            None,
            None,
        );
        assert_eq!(named.payload["title"], "Subagent compute_sum");

        let anonymous = collaboration_agent_row(
            &CollaborationAgent {
                thread_id: "019fc9277b067c43af761ab6f2126876".to_string(),
                status: "running".to_string(),
                message: None,
                path: None,
            },
            None,
            None,
        );
        assert_eq!(anonymous.payload["title"], "Subagent 019fc927");

        // A path that names nothing falls back rather than titling the row
        // "Subagent ".
        let empty = collaboration_agent_row(
            &CollaborationAgent {
                thread_id: "019fc9277b067c43af761ab6f2126876".to_string(),
                status: "running".to_string(),
                message: None,
                path: Some("/".to_string()),
            },
            None,
            None,
        );
        assert_eq!(empty.payload["title"], "Subagent 019fc927");
    }

    /// Codex's canonical path is the only hierarchy evidence there is, and it is
    /// read strictly: a path whose parent is an agent laplus knows names that
    /// agent, a path directly under the root has no parent *child*, and a path
    /// whose parent nothing holds produces nothing at all. The last is the
    /// honesty rule — an unproven relationship is not drawn.
    #[test]
    fn parentage_comes_from_the_path_and_only_where_it_is_proven() {
        let mut children = Children::default();
        let mut decided = Decided::default();
        for (thread_id, path) in [
            ("thread-reviewer", "/root/reviewer"),
            ("thread-helper", "/root/reviewer/helper"),
            ("thread-orphan", "/root/absent/orphan"),
        ] {
            children.acted(
                &SubagentActivity {
                    id: format!("activity-{thread_id}"),
                    kind: "started".to_string(),
                    agent_thread_id: thread_id.to_string(),
                    agent_path: path.to_string(),
                    raw: Value::Null,
                },
                &mut decided,
            );
        }
        let parent_of = |child: &str| {
            decided
                .child_streams
                .iter()
                .find(|update| update.child_id == child)
                .and_then(|update| update.parent_child_id.clone())
        };
        assert_eq!(parent_of("thread-reviewer"), None, "a child of the conversation");
        assert_eq!(parent_of("thread-helper"), Some("thread-reviewer".to_string()));
        assert_eq!(
            parent_of("thread-orphan"),
            None,
            "no agent holds /root/absent, so laplus must not invent one"
        );

        // And an ancestor two agents both claim proves nothing about either of
        // them, so the relationship is dropped rather than settled by a tie-break
        // the protocol never made.
        let mut ambiguous = Children::default();
        let mut drawn = Decided::default();
        for (thread_id, path) in [
            ("thread-one", "/root/reviewer"),
            ("thread-two", "/root/reviewer"),
            ("thread-nested", "/root/reviewer/helper"),
        ] {
            ambiguous.acted(
                &SubagentActivity {
                    id: format!("activity-{thread_id}"),
                    kind: "started".to_string(),
                    agent_thread_id: thread_id.to_string(),
                    agent_path: path.to_string(),
                    raw: Value::Null,
                },
                &mut drawn,
            );
        }
        assert_eq!(
            drawn
                .child_streams
                .iter()
                .find(|update| update.child_id == "thread-nested")
                .and_then(|update| update.parent_child_id.clone()),
            None,
            "two agents held the ancestor path; neither is proven to be the parent"
        );
    }

    /// The compact row is the other half of an honest ending: once a child has
    /// reported, the row says what came back rather than what it was doing.
    #[test]
    fn a_terminal_row_replaces_activity_with_what_came_back() {
        let row = |status: &str, message: Option<&str>, latest: Option<&str>| {
            collaboration_agent_row(
                &CollaborationAgent {
                    thread_id: "child-thread".to_string(),
                    status: status.to_string(),
                    message: message.map(str::to_string),
                    path: Some("/root/reviewer".to_string()),
                },
                latest,
                None,
            )
            .payload["detail"]
                .clone()
        };
        assert_eq!(row("running", None, Some("ls src")), "ls src");
        assert_eq!(
            row("completed", Some("No defects found."), Some("ls src")),
            "No defects found."
        );
        assert_eq!(
            row("shutdown", None, Some("ls src")),
            "Codex shut this subagent down.",
            "a stopped child must not go on showing the command it was running"
        );
        assert_eq!(
            row("completed", None, Some("ls src")),
            Value::Null,
            "a completion with nothing to read says nothing rather than something stale"
        );
    }

    /// A completed operation is not a completed child. The spawn finished; the
    /// agent it started has not, and the stream it opened says so.
    #[test]
    fn a_completed_spawn_leaves_its_child_open() {
        let mut children = Children::default();
        let mut decided = Decided::default();
        children.operated(
            &CollaborationCall {
                id: "spawn-1".to_string(),
                tool: "spawnAgent".to_string(),
                status: "completed".to_string(),
                sender_thread_id: "parent-thread".to_string(),
                receiver_thread_ids: vec!["child-thread".to_string()],
                prompt: Some("Review the decoder.".to_string()),
                agents: Vec::new(),
                raw: Value::Null,
            },
            true,
            &mut decided,
        );
        let update = decided.child_streams.first().expect("the child's update");
        assert_eq!(update.child_id, "child-thread");
        assert_eq!(update.assignment.as_deref(), Some("Review the decoder."));
        assert_eq!(update.state, None, "the spawn said nothing about the child");
        assert_eq!(update.outcome, None);
        assert_eq!(update.entries.len(), 1);
        assert_eq!(update.entries[0].payload["title"], "Spawned subagent");
        assert_eq!(update.entries[0].payload["status"], "completed");
    }

    /// Five Codex endings, four shared kinds. The pairs that share a kind keep
    /// their difference in the text rather than losing it silently.
    #[test]
    fn every_codex_ending_keeps_what_made_it_different() {
        let ended = |status: &str, message: Option<&str>| {
            let outcome = conclusion(status, message).expect("an ending");
            (outcome.kind.as_str(), outcome.text.unwrap_or_default())
        };
        assert_eq!(
            ended("completed", Some("No defects found.")),
            ("completed", "No defects found.".to_string())
        );
        assert_eq!(
            ended("completed", None),
            ("empty", String::new()),
            "a silent completion is empty rather than a sentence laplus wrote"
        );
        assert_eq!(
            ended("errored", None),
            ("failed", "Codex reported this subagent as errored.".to_string())
        );
        assert_eq!(
            ended("notFound", None),
            ("failed", "Codex could not find this subagent's thread.".to_string())
        );
        assert_eq!(
            ended("interrupted", None),
            ("interrupted", "Codex reported this subagent as interrupted.".to_string())
        );
        assert_eq!(
            ended("shutdown", None),
            ("interrupted", "Codex shut this subagent down.".to_string())
        );
        assert_eq!(ended("errored", Some("The model call failed.")).1, "The model call failed.");
        assert_eq!(conclusion("running", None), None);
        assert_eq!(conclusion("pendingInit", None), None);
    }

    #[test]
    fn observed_terminal_agent_states_use_the_same_stable_row_key() {
        let running = collaboration_agent_row(
            &CollaborationAgent {
                thread_id: "child-thread".to_string(),
                status: "running".to_string(),
                message: None,
                path: Some("/root/reviewer".to_string()),
            },
            Some("/root/reviewer"),
            None,
        );
        let completed = collaboration_agent_row(
            &CollaborationAgent {
                thread_id: "child-thread".to_string(),
                status: "completed".to_string(),
                message: Some("No defects found.".to_string()),
                path: Some("/root/reviewer".to_string()),
            },
            None,
            None,
        );

        assert_eq!(running.kind, "tool.updated");
        assert_eq!(completed.kind, "tool.completed");
        assert_eq!(completed.payload["status"], "completed");
        assert_eq!(completed.payload["detail"], "No defects found.");
        assert_eq!(
            running.payload["data"]["toolCallId"],
            completed.payload["data"]["toolCallId"]
        );
    }
}
