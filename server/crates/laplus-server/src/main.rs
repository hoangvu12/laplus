//! Run the socket endpoint with no window, and optionally serve the UI to it.
//!
//! The desktop application is `laplus-shell`, which starts this same server
//! in-process and points a webview at it. This binary is what is left when the
//! window is taken away: a socket endpoint the real UI can be pointed at from a
//! development server, which is how every ticket before 23 was driven and still
//! the quickest way to see a change without a webview in the loop.
//!
//! **Since ticket 01 of the headless-Linux effort it can also serve the page.**
//! `--ui <dir>` points it at a built `apps/web/dist`, which is what a phone
//! needs: a browser has no application to start, so the page and the API have
//! to come from the same place. Without the flag this binary behaves exactly as
//! it did — 404 at `/`, every route unchanged.

use std::process::ExitCode;

use laplus_server::launch;
use laplus_server::ui::Assets;
use laplus_server::Server;

#[tokio::main]
async fn main() -> ExitCode {
    let requested = match launch::requested() {
        Ok(requested) => requested,
        Err(message) => {
            eprintln!("laplus: {message}");
            return ExitCode::FAILURE;
        }
    };

    // No assets unless pointed at some: this binary answers calls, and serves
    // pages only when told which. The shell's bundle is compiled in instead —
    // see `laplus_server::ui`.
    //
    // **A bundle that will not load stops the server**, rather than starting one
    // that answers 404 at `/`. The failure a misspelled path would otherwise
    // produce is indistinguishable from the feature not working, and `--ui` is
    // asked for by somebody who wants pages served.
    let assets = match &requested.ui {
        None => Assets::none(),
        Some(directory) => match Assets::from_directory(directory) {
            Ok(assets) => {
                println!(
                    "laplus: serving the UI from {}{}",
                    directory.display(),
                    match assets.version() {
                        Some(version) => format!(" ({version})"),
                        None => String::new(),
                    }
                );
                assets
            }
            Err(failure) => {
                eprintln!("laplus: cannot serve {}: {failure}", directory.display());
                return ExitCode::FAILURE;
            }
        },
    };

    let server = match Server::bind(requested.port, assets).await {
        Ok(server) => server,
        Err(failure) => {
            eprintln!("laplus: {failure}");
            return ExitCode::FAILURE;
        }
    };

    println!("laplus: listening on {}", server.ws_url());

    // Printed because this binary has no window to open it in, and since
    // ticket 73 a browser pointed here needs a credential like anything else.
    // Without this line the quickest way to see a change would have become the
    // one that lands on a pairing screen with nothing to type into it.
    //
    // The reference server prints the same URL for the same reason —
    // `issueStartupPairingUrl`, `EnvironmentAuth.ts:911-921`.
    //
    // A development server serving the UI on another port is a *different
    // origin*, so it needs the tunnel allowlist rather than this; see
    // `laplus_server::remote_access`.
    match server.window_url() {
        Some(url) => println!("laplus: open {url}"),
        None => eprintln!(
            "laplus: no boot credential was minted, so a browser opened at {} will \
             ask to be paired",
            server.http_url()
        ),
    }

    server.serve_until_interrupted().await;
    ExitCode::SUCCESS
}
