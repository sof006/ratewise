use anyhow::bail;
use clap::Parser;
use ratewise::cli::{Cli, Command};
use tracing_subscriber::EnvFilter;

fn main() -> anyhow::Result<()> {
    // `RATEWISE_LOG=debug` enables verbose output; logs go to stderr so `--json` stays clean.
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_env("RATEWISE_LOG").unwrap_or_else(|_| EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    tracing::debug!(?cli, "parsed command line");

    let name = match cli.command {
        None => "the GUI",
        Some(Command::List { .. }) => "`ratewise list`",
        Some(Command::Show { .. }) => "`ratewise show`",
        Some(Command::Set { .. }) => "`ratewise set`",
        Some(Command::Plan { .. }) => "`ratewise plan`",
        Some(Command::Revert) => "`ratewise revert`",
        Some(Command::Reset) => "`ratewise reset`",
        Some(Command::Doctor) => "`ratewise doctor`",
        Some(Command::CaptureFixture { .. }) => "`ratewise capture-fixture`",
    };
    bail!("{name} isn't implemented yet. Run `ratewise --help` to see what's planned.")
}
