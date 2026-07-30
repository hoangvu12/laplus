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
//!
//! **Tickets 04 and 03 finished the job of making it usable from one.** `--network`
//! leaves loopback without a Settings panel to turn the switch in, and what is
//! printed at startup names the address other machines reach this one at rather
//! than the loopback address that is useless on the phone it was printed for.
//! Neither decision is made here: [`laplus_server::launch`] parses, and
//! [`laplus_server::startup`] settles what to say. This is the wiring between
//! them, and `server/docs/running-headless.md` is the page for an operator.

use std::process::ExitCode;

use laplus_server::launch::{self, Invoked};
use laplus_server::service;
use laplus_server::startup::{self, Announcement, Line, Reachable};
use laplus_server::ui::Assets;
use laplus_server::{endpoints, Server};

#[tokio::main]
async fn main() -> ExitCode {
    let requested = match launch::invoked() {
        Ok(Invoked::Serve(requested)) => requested,
        // `service` does its work and exits; it never gets as far as binding a
        // port. The server it installs is a different process entirely — see
        // `laplus_server::service`.
        Ok(Invoked::Service {
            verb,
            requested,
            arguments,
        }) => return manage_service(verb, requested, arguments),
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

    let server = match Server::bind(
        requested.port,
        assets,
        requested.network.map(|network| network.exposure),
    )
    .await
    {
        Ok(server) => server,
        Err(failure) => {
            eprintln!("laplus: {failure}");
            return ExitCode::FAILURE;
        }
    };

    println!("laplus: listening on {}", server.ws_url());

    // Read back out of the running server rather than from `requested`, so what
    // is announced is the posture the listener actually has: the flag is an
    // override and `remote-access.json` is the answer when there is none, and
    // only the configuration the server was built with knows which happened.
    let access = server.remote_access();

    // Ticket 03. `advertised_host` is a routing-table lookup guarded on
    // exposure, so this is `None` on a loopback-bound server *and* on a box with
    // no route off itself — two states the announcement then tells apart.
    //
    // `--advertise-host` wins where it was given, and is the answer for the box
    // where the routing table's is unobtainable rather than merely absent: a
    // cloud instance holds only its private address, and a tunnel's hostname is
    // not on the machine at all. `crate::launch::advertise_host_in` is the
    // reasoning. It is taken whatever the exposure, because the tunnel case is
    // loopback-bound and works anyway.
    let lan = requested
        .advertise_host
        .clone()
        .or_else(|| endpoints::advertised_host(&access))
        .map(|host| Reachable {
            paired: server.pairing_url_for(&host),
            plain: server.url_for(&host),
        });

    // Printed because this binary has no window to open it in, and since
    // ticket 73 a browser pointed here needs a credential like anything else.
    // Without these lines the quickest way to see a change would have become
    // the one that lands on a pairing screen with nothing to type into it.
    //
    // The reference server prints the same URLs for the same reason —
    // `issueStartupPairingUrl` (`EnvironmentAuth.ts:911-921`) and
    // `resolveHeadlessConnectionString` (`startupAccess.ts`).
    for line in startup::announce(&Announcement {
        exposure: access.exposure(),
        network: requested.network,
        stored: access.is_stored(),
        local: Reachable {
            paired: server.window_url(),
            plain: server.http_url(),
        },
        lan,
        advertised_by_operator: requested.advertise_host.is_some(),
        credential: server.boot_credential().map(str::to_string),
    }) {
        match line {
            Line::Said(text) => println!("laplus: {text}"),
            Line::Warned(text) => eprintln!("laplus: {text}"),
        }
    }

    server.serve_until_interrupted().await;
    ExitCode::SUCCESS
}

/// `laplus-server service <verb>`: write, inspect or remove the systemd user
/// unit that starts this server at boot.
///
/// **The bundle is staged before the plan is made, and that order matters.** The
/// unit has to name a `--ui` directory that will still be there after npm empties
/// its cache, so the path written into `ExecStart` is the copy's rather than the
/// one this process was started with. `service::stage` decides whether there is
/// anything to copy.
fn manage_service(
    verb: service::Verb,
    requested: launch::Requested,
    arguments: Vec<String>,
) -> ExitCode {
    // The bundle the *launcher* passed, kept out of the recorded flags: `npx
    // laplus` appends `--ui <its own cache>` on every run, and a unit carrying
    // that path is the exact failure staging exists to prevent.
    let arguments: Vec<String> = without_ui(&arguments);

    let outcome = (|| -> Result<(), String> {
        match verb {
            // Predicted, not staged: asking where the binary *would* go rather
            // than putting it there, so that `service status` reads the machine
            // and changes nothing on it.
            service::Verb::Status => {
                let plan = service::plan(arguments, None).ok().map(|plan| {
                    let (binary, ui) = service::destination(
                        &plan.binary,
                        requested.ui.as_deref(),
                        &service::data_directory(),
                    );
                    service::Plan { binary, ui, ..plan }
                });
                println!("{}", service::status(plan.as_ref())?);
                Ok(())
            }
            service::Verb::Uninstall => {
                match service::uninstall()? {
                    service::Outcome::Removed => println!("laplus: removed the service."),
                    _ => println!("laplus: no service was installed."),
                }
                Ok(())
            }
            service::Verb::Install => {
                let binary = std::env::current_exe()
                    .map_err(|failure| format!("cannot find this binary on disk: {failure}"))?;
                let data = service::data_directory();
                let (binary, ui) = service::stage(&binary, requested.ui.as_deref(), &data)?;
                let plan = service::Plan {
                    binary,
                    ui,
                    ..service::plan(arguments, None)?
                };
                let (outcome, warnings) = service::install(&plan)?;
                match outcome {
                    service::Outcome::Unchanged(_) => {
                        println!("laplus: the service is already what this binary would install.");
                    }
                    service::Outcome::Updated(plan) => {
                        println!("laplus: updated the service.\nlaplus: log {}", plan.log_path.display());
                    }
                    _ => {
                        println!("laplus: installed the service.\nlaplus: log {}", plan.log_path.display());
                        // The credential is printed at startup and there is no
                        // terminal to print it to any more, so say where it went.
                        println!(
                            "laplus: the pairing URL is in that log — `tail -f {}`",
                            plan.log_path.display()
                        );
                    }
                }
                for warning in warnings {
                    eprintln!("laplus: {warning}");
                }
                Ok(())
            }
        }
    })();

    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(failure) => {
            eprintln!("laplus: {failure}");
            ExitCode::FAILURE
        }
    }
}

/// The recorded flags without `--ui` and its value, in either spelling.
fn without_ui(arguments: &[String]) -> Vec<String> {
    let mut kept = Vec::new();
    let mut skipping = false;
    for argument in arguments {
        if skipping {
            skipping = false;
            continue;
        }
        if argument == "--ui" {
            skipping = true;
            continue;
        }
        if argument.starts_with("--ui=") {
            continue;
        }
        kept.push(argument.clone());
    }
    kept
}
