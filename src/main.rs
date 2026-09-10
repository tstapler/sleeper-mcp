use clap::Parser;
use sleeper::cli::{run, Cli};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::WARN.into())
                // chromiumoxide's CDP protocol defs lag newer Chrome releases; it already
                // drops the unrecognized messages harmlessly (ignore_invalid_messages
                // defaults to true) — this just silences the resulting warning spam.
                .add_directive("chromiumoxide=error".parse().unwrap()),
        )
        .with_target(false)
        .init();

    let cli = Cli::parse();
    if let Err(e) = run(cli).await {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}
