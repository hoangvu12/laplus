//! Run the socket endpoint on loopback, with no window and no UI.
//!
//! The desktop application is `lightcode-shell`, which starts this same server
//! in-process and points a webview at it. This binary is what is left when the
//! window is taken away: a socket endpoint the real UI can be pointed at from a
//! development server, which is how every ticket before 23 was driven and still
//! the quickest way to see a change without a webview in the loop.

use std::process::ExitCode;

use lightcode_server::launch;
use lightcode_server::ui::Assets;
use lightcode_server::Server;

#[tokio::main]
async fn main() -> ExitCode {
    let port = match launch::requested_port() {
        Ok(port) => port,
        Err(message) => {
            eprintln!("lightcode: {message}");
            return ExitCode::FAILURE;
        }
    };

    // No assets: this binary answers calls, it does not serve pages. The bundle
    // belongs to the shell — see `lightcode_server::ui`.
    let server = match Server::bind(port, Assets::none()).await {
        Ok(server) => server,
        Err(failure) => {
            eprintln!("lightcode: {failure}");
            return ExitCode::FAILURE;
        }
    };

    println!("lightcode: listening on {}", server.ws_url());
    server.serve_until_interrupted().await;
    ExitCode::SUCCESS
}
