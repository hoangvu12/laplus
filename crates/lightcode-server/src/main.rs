//! Run the socket endpoint on loopback.
//!
//! Ticket 23 wraps this in a Tauri shell that starts the same server in-process
//! and points a webview at it. Until then it is a plain binary, so the real UI
//! can be pointed at a running instance by hand.

use std::process::ExitCode;

use lightcode_server::Server;

/// Not upstream's 3773, so a reference server and lightcode can run side by
/// side while the port is still being compared against captures.
const DEFAULT_PORT: u16 = 4773;

#[tokio::main]
async fn main() -> ExitCode {
    let port = match port() {
        Ok(port) => port,
        Err(message) => {
            eprintln!("lightcode: {message}");
            return ExitCode::FAILURE;
        }
    };

    let server = match Server::bind(port).await {
        Ok(server) => server,
        Err(error) => {
            eprintln!("lightcode: cannot listen on 127.0.0.1:{port}: {error}");
            return ExitCode::FAILURE;
        }
    };

    println!("lightcode: listening on {}", server.ws_url());
    server.serve_until_interrupted().await;
    ExitCode::SUCCESS
}

/// `--port <n>`, else `LIGHTCODE_PORT`, else [`DEFAULT_PORT`]. Port 0 asks the
/// OS for a free one and is what the tests use.
fn port() -> Result<u16, String> {
    let mut args = std::env::args().skip(1);
    if let Some(argument) = args.next() {
        let value = match argument.strip_prefix("--port=") {
            Some(value) => value.to_string(),
            None if argument == "--port" => args
                .next()
                .ok_or_else(|| "--port needs a value".to_string())?,
            None => return Err(format!("unrecognised argument {argument}")),
        };
        if let Some(extra) = args.next() {
            return Err(format!("unrecognised argument {extra}"));
        }
        return value
            .parse()
            .map_err(|_| format!("{value} is not a port number"));
    }

    match std::env::var("LIGHTCODE_PORT") {
        Ok(value) => value
            .parse()
            .map_err(|_| format!("LIGHTCODE_PORT={value} is not a port number")),
        Err(_) => Ok(DEFAULT_PORT),
    }
}
