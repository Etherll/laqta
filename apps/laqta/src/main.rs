//! Composition root (ARCHITECTURE §2.5, M0 §5.5): parses the CLI, claims the single instance,
//! builds the runtime and ports, wires every entry point to `App::dispatch`, and runs GPUI.

mod blocking_pool;
mod cli;
mod logging;
mod spawner;
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

    let server = match laqta_ipc::claim() {
        Ok(laqta_ipc::Claim::Primary(server)) => server,
        Ok(laqta_ipc::Claim::Secondary(mut client)) => {
            if args.actions.is_empty() {
                println!("Laqta is already running");
                return ExitCode::SUCCESS;
            }
            for action in args.actions {
                if let Err(e) = client.send(action) {
                    eprintln!("laqta: {e}");
                    return ExitCode::from(1);
                }
            }
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("laqta: cannot claim the single-instance endpoint: {e}");
            return ExitCode::from(1);
        }
    };

    let _log_guard = logging::init();
    match wiring::run(server, args.actions) {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            tracing::error!("{msg}");
            eprintln!("laqta: {msg}");
            ExitCode::from(1)
        }
    }
}
