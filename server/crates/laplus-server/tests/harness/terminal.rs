//! Driving a terminal from the far side of the socket.
//!
//! The other harness helpers only have to *read*. This one has to be the other
//! half of a terminal, because a terminal is not a stream — it is a
//! conversation, and the server is deliberately only the wire between the two
//! ends of it.
//!
//! Two obligations, and the first is not optional:
//!
//! - **Answer the shell's questions.** ConPTY opens by sending `ESC [ 6 n` and
//!   blocks until something says where the cursor is. In the app that is
//!   `xterm.js`; here it is [`Pane::absorb`]. A test that skipped it would not
//!   fail — it would hang on a shell that never printed a prompt, which is the
//!   least useful way for a suite to tell you something.
//! - **Acknowledge every chunk.** `Ack` is real back-pressure on this wire, so
//!   a pane that stopped acknowledging would stop receiving and every
//!   assertion after that would time out.
//!
//! What it deliberately does *not* do is interpret anything else: cursor
//! motion, colour and screen clears go into [`Pane::screen`] as the bytes they
//! arrived as. Assertions are therefore about what the shell *said*, which is
//! the thing this server is responsible for delivering, rather than about where
//! it would have landed on a grid, which is upstream's emulator's business.

#![allow(dead_code)]

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::SocketClient;

/// The escape ConPTY opens with: "where is the cursor?".
const CURSOR_QUERY: &str = "\u{1b}[6n";

/// What an emulator answers it with. The position is a claim about a grid this
/// harness does not keep; nothing the tests assert depends on it, and the shell
/// only needs *an* answer.
const CURSOR_REPORT: &str = "\u{1b}[1;1R";

/// How long a shell may take to say something before the test fails rather
/// than waits. Generous: it covers starting a process, and a first run on a
/// cold machine is much slower than the ones after it.
const PATIENCE: Duration = Duration::from_secs(30);

/// One terminal, as the pane on the other end of the socket sees it.
pub struct Pane {
    pub thread_id: String,
    pub terminal_id: String,
    /// The subscription `terminal.attach` opened.
    pub attachment: String,
    /// Everything the shell has said, in the order it said it.
    pub screen: String,
    /// Every event that was not output — `exited`, `error`, and any the server
    /// learns to send later.
    pub notices: Vec<Value>,
    /// The snapshots the stream has delivered. More than one means the server
    /// re-described the world, which is what it does when this pane falls
    /// behind.
    pub snapshots: Vec<Value>,
}

impl Pane {
    /// Attach to a terminal, opening one in `cwd` if it is not there.
    ///
    /// This is the order the reused UI can genuinely produce — its attach
    /// carries everything an open needs precisely because it may arrive first.
    pub async fn attach(
        client: &mut SocketClient,
        thread_id: &str,
        terminal_id: &str,
        payload: Value,
    ) -> Pane {
        let mut payload = payload;
        payload["threadId"] = json!(thread_id);
        payload["terminalId"] = json!(terminal_id);

        let attachment = client.subscribe("terminal.attach", payload).await;
        let mut pane = Pane {
            thread_id: thread_id.to_string(),
            terminal_id: terminal_id.to_string(),
            attachment,
            screen: String::new(),
            notices: Vec::new(),
            snapshots: Vec::new(),
        };
        pane.pump(client).await;
        pane
    }

    /// The opening payload a pane sends: a working directory and a size.
    pub fn opening(cwd: &std::path::Path, cols: u64, rows: u64) -> Value {
        json!({
            "cwd": cwd.to_string_lossy(),
            "cols": cols,
            "rows": rows,
            "env": shell_choice(),
        })
    }

    /// Type at the shell. Answers with the call's outcome so a test can assert
    /// on a refusal as well as on what came back.
    pub async fn type_in(&self, client: &mut SocketClient, keystrokes: &str) -> super::Outcome {
        client
            .call(
                "terminal.write",
                json!({
                    "threadId": self.thread_id,
                    "terminalId": self.terminal_id,
                    "data": keystrokes,
                }),
            )
            .await
    }

    /// Type a command line and press return.
    ///
    /// Deliberately does *not* wait for the output. What to wait for is the
    /// caller's business, and it is always a marker the command itself prints
    /// rather than a duration: how long a shell takes to answer is a fact about
    /// the machine, and a test that slept for it would be asserting on that.
    pub async fn run(&self, client: &mut SocketClient, command: &str) {
        self.type_in(client, &format!("{command}\r"))
            .await
            .expect_success();
    }

    pub async fn resize(&self, client: &mut SocketClient, cols: u64, rows: u64) -> super::Outcome {
        client
            .call(
                "terminal.resize",
                json!({
                    "threadId": self.thread_id,
                    "terminalId": self.terminal_id,
                    "cols": cols,
                    "rows": rows,
                }),
            )
            .await
    }

    /// Forget what this terminal has shown.
    pub async fn clear(&self, client: &mut SocketClient) -> super::Outcome {
        client.call("terminal.clear", self.named()).await
    }

    /// Put a new shell in this terminal.
    pub async fn restart(
        &self,
        client: &mut SocketClient,
        cwd: &std::path::Path,
        cols: u64,
        rows: u64,
    ) -> super::Outcome {
        let mut payload = Pane::opening(cwd, cols, rows);
        payload["threadId"] = json!(self.thread_id);
        payload["terminalId"] = json!(self.terminal_id);
        client.call("terminal.restart", payload).await
    }

    /// End this terminal and reap what was in it.
    pub async fn close(&self, client: &mut SocketClient) -> super::Outcome {
        client.call("terminal.close", self.named()).await
    }

    /// Leave the pane, as navigating somewhere else in the app does.
    ///
    /// The subscription ends and the terminal does not — that separation is the
    /// whole of what "navigating away and back" means on this wire, so this
    /// consumes the pane and reattaching means opening a new one.
    pub async fn detach(self, client: &mut SocketClient) {
        client.interrupt(&self.attachment).await;
        client.await_outcome(&self.attachment).await;
    }

    fn named(&self) -> Value {
        json!({"threadId": self.thread_id, "terminalId": self.terminal_id})
    }

    /// Read until the shell has said `wanted`, answering whatever it asks along
    /// the way.
    pub async fn wait_for(&mut self, client: &mut SocketClient, wanted: &str) {
        self.wait_for_occurrences(client, wanted, 1).await;
    }

    /// Read until the shell has said `wanted` this many times. Command lines are
    /// echoed by a pty, so the second occurrence proves the command ran.
    pub async fn wait_for_occurrences(
        &mut self,
        client: &mut SocketClient,
        wanted: &str,
        count: usize,
    ) {
        let deadline = Instant::now() + PATIENCE;
        while self.screen.matches(wanted).count() < count {
            assert!(
                Instant::now() < deadline,
                "the terminal never said {wanted:?} {count} times. What it did say:\n{}",
                self.screen
            );
            self.pump(client).await;
        }
    }

    /// Read until the terminal reports that its shell has gone.
    pub async fn wait_for_exit(&mut self, client: &mut SocketClient) -> Value {
        self.wait_for_notice(client, "exited").await
    }

    /// Read until the terminal says something of this kind — anything that is
    /// not output: `exited`, `closed`, `cleared`, `restarted`, `error`.
    pub async fn wait_for_notice(&mut self, client: &mut SocketClient, kind: &str) -> Value {
        let deadline = Instant::now() + PATIENCE;
        loop {
            if let Some(notice) = self
                .notices
                .iter()
                .find(|notice| notice["type"] == kind)
                .cloned()
            {
                return notice;
            }
            assert!(
                Instant::now() < deadline,
                "the terminal never said {kind:?}. What arrived:\n{:#?}",
                self.notices
            );
            self.pump(client).await;
        }
    }

    /// Take one chunk from the attachment and fold it in.
    pub async fn pump(&mut self, client: &mut SocketClient) {
        let values = client.next_chunk(&self.attachment).await;
        client.ack(&self.attachment).await;
        for value in values {
            self.absorb(client, value).await;
        }
    }

    /// The screen, with the escape sequences taken out — what a person reading
    /// the pane would see, which is what most assertions are about.
    pub fn text(&self) -> String {
        let mut text = String::new();
        let bytes = self.screen.as_bytes();
        let mut index = 0;
        while index < bytes.len() {
            if bytes[index] != 0x1b {
                let start = index;
                while index < bytes.len() && bytes[index] != 0x1b {
                    index += 1;
                }
                text.push_str(&self.screen[start..index]);
                continue;
            }
            index += 1;
            // Enough of a parse to skip what a shell actually emits: a CSI run
            // ending in a final byte, or an OSC string ending in BEL.
            match bytes.get(index) {
                Some(b'[') => {
                    index += 1;
                    while index < bytes.len() && !(0x40..=0x7e).contains(&bytes[index]) {
                        index += 1;
                    }
                    index += 1;
                }
                Some(b']') => {
                    while index < bytes.len() && bytes[index] != 0x07 {
                        index += 1;
                    }
                    index += 1;
                }
                _ => index += 1,
            }
        }
        text
    }

    async fn absorb(&mut self, client: &mut SocketClient, value: Value) {
        match value["type"].as_str() {
            Some("snapshot") => {
                let snapshot = value["snapshot"].clone();
                // A snapshot replaces the buffer rather than adding to it —
                // the client's own reducer does exactly this, and it is what
                // makes a re-description safe on a stream that otherwise only
                // appends.
                self.screen = snapshot["history"].as_str().unwrap_or_default().to_string();
                self.snapshots.push(snapshot);
            }
            Some("output") => {
                let data = value["data"].as_str().unwrap_or_default().to_string();
                self.screen.push_str(&data);
                if data.contains(CURSOR_QUERY) {
                    // The shell is blocked until this is written. Being the
                    // thing that answers is the whole of what makes this a
                    // terminal rather than a log reader.
                    self.type_in(client, CURSOR_REPORT).await.expect_success();
                }
            }
            // The two events that *replace* the buffer rather than adding to
            // it, folded exactly as the client's own reducer folds them
            // (`applyTerminalAttachStreamEvent`). A harness that only appended
            // would show a restarted terminal's dead shell and a cleared one's
            // cleared output, and every assertion about either would pass for
            // the wrong reason.
            Some("restarted") => {
                let snapshot = value["snapshot"].clone();
                self.screen = snapshot["history"].as_str().unwrap_or_default().to_string();
                self.snapshots.push(snapshot);
                self.notices.push(value);
            }
            Some("cleared") => {
                self.screen.clear();
                self.notices.push(value);
            }
            _ => self.notices.push(value),
        }
    }
}

/// The shell a test drives, named through the environment the client sends.
///
/// `ComSpec` and `SHELL` are the platform's own way of naming the command
/// interpreter and the server consults the session's environment for them, so
/// this is a real value travelling a real path — not a seam that exists for the
/// suite. What it buys is a shell that is the same everywhere the suite runs,
/// rather than whichever of PowerShell's several versions the machine prefers.
pub fn shell_choice() -> Value {
    match cfg!(windows) {
        true => json!({"ComSpec": windows_cmd()}),
        false => json!({"SHELL": "/bin/sh"}),
    }
}

fn windows_cmd() -> String {
    let root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".to_string());
    format!("{root}\\System32\\cmd.exe")
}

/// A command that prints `marker`, and nothing a shell would decorate.
pub fn echo(marker: &str) -> String {
    format!("echo {marker}")
}

/// A command that makes the shell report the size of the terminal it is in.
///
/// The only assertion available for "the pty was really resized" that does not
/// require an emulator: ask the program, and read its answer off the same
/// stream everything else arrives on.
pub fn report_size() -> String {
    match cfg!(windows) {
        true => "mode con".to_string(),
        false => "stty size".to_string(),
    }
}

/// The size `report_size` would have printed, as this platform prints it.
pub fn reported_size(text: &str, cols: u64, rows: u64) -> bool {
    match cfg!(windows) {
        true => {
            text.contains(&format!("Columns:        {cols}"))
                && text.contains(&format!("Lines:          {rows}"))
        }
        false => text.contains(&format!("{rows} {cols}")),
    }
}

/// A command that exits the shell.
pub fn quit() -> String {
    "exit".to_string()
}

/// Roughly how long [`slow_work`] takes and how often [`endless_child`] ticks.
/// One second on both platforms, because `ping` is the only delay `cmd.exe` has
/// and it counts in whole seconds.
pub const TICK: Duration = Duration::from_secs(1);

/// A command that waits a few seconds and *then* prints `marker`.
///
/// What "a process still running while detached continues" needs: the marker
/// cannot already be on the screen when the pane goes away, so its arrival is
/// evidence the shell kept working with nothing listening.
pub fn slow_work(marker: &str, ticks: u32) -> String {
    match cfg!(windows) {
        // `ping -n N` sends N packets a second apart, so it waits N-1 seconds.
        true => format!("ping -n {} 127.0.0.1 >nul & echo {marker}", ticks + 1),
        false => format!("sleep {ticks}; echo {marker}"),
    }
}

/// A script that runs forever in a process of its own, appending a line to
/// `log` every [`TICK`], and the command that starts it.
///
/// Two things make this the shape it is. It has to be a **child process** — a
/// batch file typed at a `cmd.exe` prompt is interpreted by that same
/// `cmd.exe`, so `cmd /c` is what makes a second process — and it has to leave
/// evidence *outside* the terminal, because the terminal is the thing being
/// closed. A file that stops growing is that evidence.
pub fn endless_child(directory: &std::path::Path, log: &str) -> String {
    let log = directory.join(log).display().to_string();
    let (name, script, interpreter) = match cfg!(windows) {
        true => (
            "endless-child.cmd",
            format!("@echo off\r\n:again\r\necho tick>>\"{log}\"\r\nping -n 2 127.0.0.1 >nul\r\ngoto again\r\n"),
            "cmd /c",
        ),
        false => (
            "endless-child.sh",
            format!("while true; do echo tick >> \"{log}\"; sleep 1; done\n"),
            "sh",
        ),
    };

    let path = directory.join(name);
    std::fs::write(&path, script).expect("writes the script");
    format!("{interpreter} \"{}\"", path.display())
}

/// How long a file has been, or zero if it is not there yet.
pub fn length(path: &std::path::Path) -> u64 {
    std::fs::metadata(path).map(|file| file.len()).unwrap_or(0)
}

/// The SGR sequence that sets the foreground to red. What "colour arrived"
/// means on the wire, and the one spelling of it both platforms emit.
pub const RED: &str = "\u{1b}[31m";

/// A command that makes the shell print `text` in red.
///
/// Both platforms have a way to put a literal escape into *output* without one
/// having to survive the trip through the terminal as *input*: `cmd.exe`'s
/// prompt syntax spells it `$E`, and `printf` spells it `\033`.
///
/// What comes back is deliberately **not** compared byte for byte against what
/// the command wrote. ConPTY is itself an emulator: it renders the program's
/// output into a screen buffer and re-emits VT to describe it, so a red `LC`
/// arrives as `ESC [ 31 m`, a cursor move, `LC`, and `ESC [ m` — the same
/// colour, a different spelling. Byte-exactness from the *program* was never
/// the server's to promise. What is, is that the colour crosses the wire as a
/// control sequence rather than as text, and that is what the caller asserts.
pub fn colour_command(text: &str) -> String {
    match cfg!(windows) {
        true => format!("prompt $E[31m{text}$E[0m$G"),
        false => format!("printf '\\033[31m{text}\\033[0m\\n'"),
    }
}
