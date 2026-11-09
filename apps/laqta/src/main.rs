//! Composition root (ARCHITECTURE §2.5): parses the CLI, claims the single instance,
//! builds the runtime and ports, wires every entry point to `App::dispatch`, and runs GPUI.

mod blocking_pool;
mod cli;
mod cloud;
mod fonts;
mod imports;
mod local_models;
mod logging;
mod media;
mod models;
mod notifier;
mod ocr;
mod requests;
mod spawner;
mod stubs;
mod updates;
mod uploads;
mod wiring;

use std::process::ExitCode;

fn main() -> ExitCode {
    let args = match cli::parse(std::env::args().skip(1)) {
        Ok(cli::Command::Version) => {
            println!("laqta {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        Ok(cli::Command::Run(args)) => args,
        Err(msg) => {
            eprintln!("laqta: {msg}\n{}", cli::USAGE);
            return ExitCode::from(2);
        }
    };

    if args.startup_delay_secs > 0 {
        std::thread::sleep(std::time::Duration::from_secs(args.startup_delay_secs as u64));
    }
    let claim = if args.multi { None } else { Some(laqta_ipc::claim()) };
    let server = match claim {
        None => None,
        Some(Ok(laqta_ipc::Claim::Primary(server))) => Some(server),
        Some(Ok(laqta_ipc::Claim::Secondary(mut client))) => {
            if args.requests.is_empty() {
                println!("Laqta is already running");
                return ExitCode::SUCCESS;
            }
            for request in &args.requests {
                if let Err(e) = client.send_request(request) {
                    eprintln!("laqta: {e}");
                    return ExitCode::from(1);
                }
            }
            return ExitCode::SUCCESS;
        }
        Some(Err(e)) => {
            eprintln!("laqta: cannot claim the single-instance endpoint: {e}");
            return ExitCode::from(1);
        }
    };

    let _log_guard = logging::init();
    match wiring::run(server, args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            tracing::error!("{msg}");
            eprintln!("laqta: {msg}");
            ExitCode::from(1)
        }
    }
}
