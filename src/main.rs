use clap::Parser;
use ratewise::cli::{self, Cli};
use tracing_subscriber::EnvFilter;

fn main() -> anyhow::Result<()> {
    // `RATEWISE_LOG=debug` enables verbose output; logs go to stderr so `--json` stays clean.
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_env("RATEWISE_LOG")
                .unwrap_or_else(|_| EnvFilter::new("warn,arboard=error,winit=error")),
        )
        .with_writer(std::io::stderr)
        .init();

    ratewise::crash::install();
    let cli = Cli::parse();
    tracing::debug!(?cli, "parsed command line");
    cli::run(cli)
}
