//! What a headless `laplus-server` does when the system asks it to stop.
//!
//! **`SIGTERM` is how a server is stopped, and laplus never listened for it.**
//! `serve_until_interrupted` awaited `ctrl_c` alone, so `systemctl stop`,
//! `docker stop` and a plain `kill` all took the default disposition: the
//! process died immediately and `Server::shutdown` never ran. The connector it
//! had started is deliberately in its own process group — so that a terminal's
//! `^C` cannot reach it — which means nothing else stopped it either, and a
//! public hostname outlived the server it exposed. `docs/adr/0048` claims the
//! opposite and names the systemd case by name.
//!
//! The real binary rather than the in-process harness, because the claim is
//! about a signal the operating system delivers to a process.
#![cfg(unix)]

mod harness;

use harness::cloudflare::FakeCloudflared;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

/// A hang detector rather than a budget — see `READ_TIMEOUT` in the harness.
const SETTLES_WITHIN: Duration = Duration::from_secs(30);

/// An isolated `laplus-server`, with its data directory somewhere throwaway.
///
/// The same shape as `cli.rs`'s `IsolatedInvocation`: every variable that could
/// point the server at the developer's own state is redirected, so the test
/// cannot read or write a real laplus installation.
struct HeadlessServer {
    directory: tempfile::TempDir,
    child: std::process::Child,
}

impl HeadlessServer {
    fn data(&self) -> PathBuf {
        self.directory.path().join("data").join("laplus")
    }

    /// Start with a connector already configured and asked to run, so the
    /// server brings it up at boot without a single HTTP call. That is also the
    /// case the bug is about: a server restored by systemd after a reboot.
    fn start(fake: &FakeCloudflared, token: &Path) -> Self {
        let directory = tempfile::tempdir().expect("an isolated server environment");
        let data = directory.path().join("data").join("laplus");
        let cloudflare = data.join("cloudflare");
        std::fs::create_dir_all(&cloudflare).expect("the connector directory");
        std::fs::write(
            cloudflare.join("connector.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "httpsOrigin": "https://laplus.example.com",
                "loopbackOrigin": "http://127.0.0.1:1",
                "executablePath": fake.executable,
                "tokenFile": token,
                "desiredState": "running",
            }))
            .expect("the connector settings encode"),
        )
        .expect("the connector settings are written");

        let child = std::process::Command::new(env!("CARGO_BIN_EXE_laplus-server"))
            .args(["serve", "--port", "0"])
            .env("HOME", directory.path().join("home"))
            .env("USERPROFILE", directory.path().join("home"))
            .env("XDG_CONFIG_HOME", directory.path().join("config"))
            .env("XDG_DATA_HOME", directory.path().join("data"))
            .env_remove("LOCALAPPDATA")
            .env_remove("APPDATA")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("the laplus-server binary runs");
        Self { directory, child }
    }

    fn terminate(&self) {
        let status = std::process::Command::new("kill")
            .args(["-TERM", &self.child.id().to_string()])
            .status()
            .expect("kill runs");
        assert!(status.success(), "SIGTERM could not be delivered");
    }

    fn wait(&mut self) -> std::process::ExitStatus {
        let deadline = Instant::now() + SETTLES_WITHIN;
        loop {
            if let Some(status) = self.child.try_wait().expect("the child can be observed") {
                return status;
            }
            assert!(Instant::now() < deadline, "the server never exited after SIGTERM");
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for HeadlessServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn settles(mut until: impl FnMut() -> bool, what: &str) {
    let deadline = Instant::now() + SETTLES_WITHIN;
    while !until() {
        assert!(Instant::now() < deadline, "{what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[tokio::test]
async fn sigterm_stops_the_connector_laplus_started() {
    let directory = tempfile::tempdir().expect("a directory for the fake");
    let fake = FakeCloudflared::write_into(directory.path());
    let token = directory.path().join("connector.token");
    std::fs::write(&token, harness::cloudflare::CONNECTOR_TOKEN).expect("the token file");

    let mut server = HeadlessServer::start(&fake, &token);
    settles(
        || fake.ready_to_stop(),
        "the connector never installed its termination handler",
    );
    assert!(fake.launches() >= 1, "the connector was never started");
    // Running, and not yet stopped — otherwise the assertion below would pass
    // for a connector that had merely crashed. Count the event rather than
    // requiring it to remain the last trace line: startup's independent
    // version probe may finish after shutdown and append its own invocation.
    let graceful_stops_before = fake.graceful_stops();
    assert_eq!(graceful_stops_before, 0);

    server.terminate();
    let status = server.wait();

    // The connector was asked to stop and answered, which is what ADR-0048
    // means by shutting down gracefully with its owner. Without a SIGTERM
    // handler this line never appears: the server dies where it stands and the
    // connector, in its own process group, keeps serving the public hostname.
    settles(
        || fake.graceful_stops() > graceful_stops_before,
        "the connector outlived the server it was started by",
    );
    assert!(status.success(), "a requested shutdown is not a failure: {status:?}");
    // The data directory is this test's own; nothing here touched a real one.
    assert!(server.data().join("cloudflare").is_dir());
}
