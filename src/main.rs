#[cfg(not(target_os = "macos"))]
compile_error!("Window Switcher requires macOS.");

mod cli;
mod core;
mod macos;
mod ui;

use std::process::ExitCode;

use macos::window_backend::MacWindowBackend;
use objc2::rc::autoreleasepool;
use tracing_subscriber::EnvFilter;

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn")),
        )
        .init();

    let args: Vec<_> = std::env::args().skip(1).collect();
    let command = match cli::Command::parse(&args) {
        Ok(command) => command,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(2);
        }
    };
    match autoreleasepool(|_| cli::run(command, &MacWindowBackend::default())) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|error| error.kind() == std::io::ErrorKind::BrokenPipe) =>
        {
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("window-switcher: {error}");
            ExitCode::FAILURE
        }
    }
}
