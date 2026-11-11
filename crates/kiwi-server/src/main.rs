use clap::Parser;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> std::process::ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            // rcodesign's library logs every file it signs as a warning.
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,apple_codesign=error")),
        )
        .init();
    let mut args = kiwi_server::Args::parse();
    let result = match args.command.take() {
        Some(kiwi_server::config::Command::Builder(command)) => {
            kiwi_server::mac_builder::client::main(command).await
        }
        None => kiwi_server::run(args).await,
    };
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("{e}");
            std::process::ExitCode::FAILURE
        }
    }
}
