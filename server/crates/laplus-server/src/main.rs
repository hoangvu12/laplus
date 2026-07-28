//! Run the socket endpoint on loopback, with no window and no UI.
//!
//! The desktop application is `laplus-shell`, which starts this same server
//! in-process and points a webview at it. This binary is what is left when the
//! window is taken away: a socket endpoint the real UI can be pointed at from a
//! development server, which is how every ticket before 23 was driven and still
//! the quickest way to see a change without a webview in the loop.

use std::process::ExitCode;

use laplus_server::launch;
use laplus_server::ui::Assets;
use laplus_server::Server;

#[tokio::main]
async fn main() -> ExitCode {
    let port = match launch::requested_port() {
        Ok(port) => port,
        Err(message) => {
            eprintln!("laplus: {message}");
            return ExitCode::FAILURE;
        }
    };

    // No assets: this binary answers calls, it does not serve pages. The bundle
    // belongs to the shell — see `laplus_server::ui`.
    let server = match Server::bind(port, Assets::none()).await {
        Ok(server) => server,
        Err(failure) => {
            eprintln!("laplus: {failure}");
            return ExitCode::FAILURE;
        }
    };

    println!("laplus: listening on {}", server.ws_url());
    server.serve_until_interrupted().await;
    ExitCode::SUCCESS
}
