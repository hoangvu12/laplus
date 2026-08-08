//! Explicit maintenance of configured provider instances.
//!
//! Strategy resolution, command execution, serialization and post-command
//! observation live behind this boundary. Provider probing only advertises a
//! command; it never executes one.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use crate::config::{ProviderUpdateState, ProviderUpdateStatus};
use crate::config_store::{ConfigChange, ConfigStore};
use crate::process::{without_a_console, Search};
use crate::provider::{configured_instance, refresh_configured, Located};

pub const UPDATE: &str = "server.updateProvider";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Action {
    pub executable: String,
    pub args: Vec<String>,
    pub lock_key: String,
}

impl Action {
    pub fn display(&self) -> String {
        std::iter::once(self.executable.as_str())
            .chain(self.args.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

fn normalized(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/").to_lowercase()
}

/// Resolve the T3-compatible OpenCode update strategy from the installation
/// that the configured binary names.
pub fn opencode_action(binary_path: &str, search: &Search) -> Option<Action> {
    let resolved = match crate::provider::resolve_named(binary_path, "opencode", search) {
        Located::Binary { path, .. } => path,
        Located::NotExecutable { .. } | Located::Nothing { .. } => return None,
    };
    let real = std::fs::canonicalize(&resolved).unwrap_or(resolved);
    let path = normalized(&real);
    let action = if path.ends_with("/.opencode/bin/opencode")
        || path.ends_with("/.opencode/bin/opencode.exe")
    {
        Action {
            executable: "opencode".into(),
            args: vec!["upgrade".into()],
            lock_key: "opencode-native".into(),
        }
    } else if path.contains("/.vite-plus/bin/") {
        Action {
            executable: "vp".into(),
            args: vec!["i".into(), "-g".into(), "opencode-ai".into()],
            lock_key: "vite-plus-global".into(),
        }
    } else if path.contains("/.bun/bin/") {
        Action {
            executable: "bun".into(),
            args: vec!["i".into(), "-g".into(), "opencode-ai@latest".into()],
            lock_key: "bun-global".into(),
        }
    } else if path.contains("/.local/share/pnpm/")
        || path.contains("/library/pnpm/")
        || path.contains("/local/share/pnpm/")
        || path.contains("/appdata/local/pnpm/")
        || path.contains("/pnpm/global/")
    {
        Action {
            executable: "pnpm".into(),
            args: vec!["add".into(), "-g".into(), "opencode-ai@latest".into()],
            lock_key: "pnpm-global".into(),
        }
    } else if path.contains("/node_modules/.bin/")
        || path.contains("/lib/node_modules/")
        || path.contains("/npm/node_modules/")
    {
        npm_action()
    } else if path.contains("/opt/homebrew/cellar/")
        || path.contains("/usr/local/cellar/")
        || path.contains("/homebrew/cellar/")
        || path.contains("/opt/homebrew/caskroom/")
        || path.contains("/usr/local/caskroom/")
        || path.contains("/homebrew/caskroom/")
        || path.starts_with("/opt/homebrew/bin/")
        || path.starts_with("/usr/local/bin/")
    {
        Action {
            executable: "brew".into(),
            args: vec!["upgrade".into(), "anomalyco/tap/opencode".into()],
            lock_key: "homebrew".into(),
        }
    } else {
        return None;
    };
    Some(action)
}

fn npm_action() -> Action {
    Action {
        executable: "npm".into(),
        args: vec!["install".into(), "-g".into(), "opencode-ai@latest".into()],
        lock_key: "npm-global".into(),
    }
}

#[derive(Debug, Clone)]
pub struct CommandOutcome {
    pub exit_code: Option<i32>,
    pub output: String,
}

pub trait CommandRunner: Send + Sync {
    fn run(&self, action: &Action) -> Result<CommandOutcome, String>;
}

#[derive(Debug)]
struct ProcessRunner;

impl CommandRunner for ProcessRunner {
    fn run(&self, action: &Action) -> Result<CommandOutcome, String> {
        let mut command = Command::new(&action.executable);
        command.args(&action.args).stdin(Stdio::null());
        without_a_console(&mut command);
        let output = command
            .output()
            .map_err(|error| format!("Could not run `{}`: {error}", action.display()))?;
        let combined = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        );
        Ok(CommandOutcome {
            exit_code: output.status.code(),
            output: combined.chars().take(10_000).collect(),
        })
    }
}

#[derive(Clone)]
pub struct ProviderMaintenance {
    runner: Arc<dyn CommandRunner>,
    locks: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
}

impl std::fmt::Debug for ProviderMaintenance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ProviderMaintenance")
    }
}

impl ProviderMaintenance {
    pub fn new() -> Self {
        Self::with_runner(Arc::new(ProcessRunner))
    }
    pub fn with_runner(runner: Arc<dyn CommandRunner>) -> Self {
        Self {
            runner,
            locks: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn lock(&self, key: String) -> Arc<Mutex<()>> {
        self.locks
            .lock()
            .expect("maintenance lock registry")
            .entry(key)
            .or_default()
            .clone()
    }

    fn serialized<T>(
        &self,
        instance_id: &str,
        action: &Action,
        work: impl FnOnce(&dyn CommandRunner) -> T,
    ) -> T {
        let instance_lock = self.lock(format!("instance:{instance_id}"));
        let manager_lock = self.lock(format!("manager:{}", action.lock_key));
        let _instance = instance_lock
            .lock()
            .expect("provider maintenance instance lock");
        let _manager = manager_lock
            .lock()
            .expect("provider maintenance manager lock");
        work(self.runner.as_ref())
    }

    pub fn update_call(
        &self,
        payload: &serde_json::Value,
        config: &ConfigStore,
        roots: &[PathBuf],
    ) -> Result<serde_json::Value, serde_json::Value> {
        let provider = payload
            .get("provider")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let Some(instance_id) = payload
            .get("instanceId")
            .and_then(serde_json::Value::as_str)
            .filter(|id| !id.trim().is_empty())
        else {
            return Err(error(
                provider,
                "This call needs an instanceId; none was given.",
            ));
        };
        let current = config.current();
        let Some(instance) = configured_instance(&current.settings, instance_id) else {
            return Err(error(
                provider,
                format!("Provider instance '{instance_id}' is not configured."),
            ));
        };
        if instance.identity().driver != provider {
            return Err(error(
                provider,
                format!(
                    "Provider instance '{instance_id}' belongs to driver '{}', not '{provider}'.",
                    instance.identity().driver
                ),
            ));
        }
        let search = Search::from_environment();
        let Some(action) = instance.maintenance_action(&search) else {
            return Err(error(
                provider,
                "This provider does not support one-click updates.",
            ));
        };
        let (before, outcome, after) = self.serialized(instance_id, &action, |runner| {
            let before = config.current().providers.iter()
                .find(|p| p.instance_id == instance_id).and_then(|p| p.version.clone());
            let outcome = runner.run(&action);
            // The command's outcome never suppresses observation: failure can
            // have changed the installation, and the refresh is authoritative.
            refresh_configured(config, instance_id, &search, roots);
            let after = config.current().providers.iter()
                .find(|p| p.instance_id == instance_id).and_then(|p| p.version.clone());
            (before, outcome, after)
        });
        let (status, message, output) = match outcome {
            Ok(result) if result.exit_code == Some(0) && before == after => (
                ProviderUpdateStatus::Unchanged,
                "Update command completed; the observed version is unchanged.".to_string(),
                nullable(result.output),
            ),
            Ok(result) if result.exit_code == Some(0) => (
                ProviderUpdateStatus::Succeeded,
                "Provider update command completed.".to_string(),
                nullable(result.output),
            ),
            Ok(result) => (
                ProviderUpdateStatus::Failed,
                format!(
                    "Update command exited with code {}.",
                    result
                        .exit_code
                        .map_or_else(|| "unknown".into(), |code| code.to_string())
                ),
                nullable(result.output),
            ),
            Err(reason) => (ProviderUpdateStatus::Failed, reason, None),
        };
        let mut providers = config.current().providers.clone();
        if let Some(provider) = providers.iter_mut().find(|p| p.instance_id == instance_id) {
            provider.update_state = Some(ProviderUpdateState {
                status, started_at: None, finished_at: Some(crate::clock::now_iso()),
                message: Some(message), output, before_version: before, after_version: after,
            });
        }
        config.apply(ConfigChange::Providers(providers));
        Ok(serde_json::json!({"providers": config.current().providers}))
    }
}

fn nullable(value: String) -> Option<String> {
    (!value.trim().is_empty()).then_some(value)
}
fn error(provider: &str, reason: impl std::fmt::Display) -> serde_json::Value {
    serde_json::json!({"_tag":"ServerProviderUpdateError", "provider": provider, "reason": reason.to_string()})
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Barrier, Condvar};

    #[derive(Debug)]
    struct BlockingRunner {
        gate: (Mutex<bool>, Condvar),
        active: AtomicUsize,
        max_active: AtomicUsize,
    }

    impl CommandRunner for BlockingRunner {
        fn run(&self, _action: &Action) -> Result<CommandOutcome, String> {
            let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_active.fetch_max(active, Ordering::SeqCst);
            let mut released = self.gate.0.lock().unwrap();
            while !*released { released = self.gate.1.wait(released).unwrap(); }
            self.active.fetch_sub(1, Ordering::SeqCst);
            Ok(CommandOutcome { exit_code: Some(0), output: String::new() })
        }
    }

    fn installation(root: &Path, relative: &str) -> PathBuf {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "").unwrap();
        #[cfg(not(windows))]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        path
    }

    #[test]
    fn opencode_installations_select_the_t3_commands() {
        let root = tempfile::tempdir().unwrap();
        let search = Search::over(&[]);
        let cases = [
            (
                ".opencode/bin/opencode",
                "opencode upgrade",
                "opencode-native",
            ),
            (
                "lib/node_modules/opencode-ai/bin/opencode",
                "npm install -g opencode-ai@latest",
                "npm-global",
            ),
            (
                ".local/share/pnpm/opencode",
                "pnpm add -g opencode-ai@latest",
                "pnpm-global",
            ),
            (
                ".bun/bin/opencode",
                "bun i -g opencode-ai@latest",
                "bun-global",
            ),
            (
                ".vite-plus/bin/opencode",
                "vp i -g opencode-ai",
                "vite-plus-global",
            ),
            (
                "opt/homebrew/Cellar/opencode/1.20/bin/opencode",
                "brew upgrade anomalyco/tap/opencode",
                "homebrew",
            ),
        ];
        for (relative, command, lock) in cases {
            // Windows has no executable bit: the extension is what names a
            // program, so the fixture binary has to wear the one a real
            // installation would.
            let relative = if cfg!(windows) { format!("{relative}.exe") } else { relative.to_string() };
            let path = installation(root.path(), &relative);
            let action = opencode_action(&path.to_string_lossy(), &search)
                .unwrap_or_else(|| panic!("strategy for {}", path.display()));
            assert_eq!(action.display(), command, "{}", path.display());
            assert_eq!(action.lock_key, lock, "{}", path.display());
        }
    }

    #[test]
    fn an_unclassified_explicit_binary_is_manual_only() {
        assert_eq!(
            opencode_action("/workspace/tools/opencode", &Search::over(&[])),
            None
        );
    }

    #[test]
    fn one_instance_serializes_commands_with_different_manager_locks() {
        let runner = Arc::new(BlockingRunner {
            gate: (Mutex::new(false), Condvar::new()),
            active: AtomicUsize::new(0), max_active: AtomicUsize::new(0),
        });
        let maintenance = ProviderMaintenance::with_runner(runner.clone());
        let callers_ready = Arc::new(Barrier::new(3));
        let (attempted_tx, attempted_rx) = std::sync::mpsc::channel();
        let handles = ["manager-a", "manager-b"].map(|manager| {
            let maintenance = maintenance.clone();
            let ready = callers_ready.clone();
            let attempted = attempted_tx.clone();
            std::thread::spawn(move || {
                let action = Action { executable: "fake".into(), args: vec![], lock_key: manager.into() };
                ready.wait();
                attempted.send(()).unwrap();
                maintenance.serialized("same-instance", &action, |runner| runner.run(&action)).unwrap();
            })
        });
        drop(attempted_tx);
        callers_ready.wait();
        attempted_rx.recv().unwrap();
        attempted_rx.recv().unwrap();
        *runner.gate.0.lock().unwrap() = true;
        runner.gate.1.notify_all();
        for handle in handles { handle.join().unwrap(); }
        assert_eq!(runner.max_active.load(Ordering::SeqCst), 1);
    }
}
