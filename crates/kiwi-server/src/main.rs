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
    match kiwi_server::run(kiwi_server::Args::parse()).await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("{e}");
            std::process::ExitCode::FAILURE
        }
    }
}
