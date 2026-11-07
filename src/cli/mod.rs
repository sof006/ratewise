//! Command-line interface (spec §7). This is the binary edge: it prints, and uses `anyhow`.

mod capture;
mod doctor;
pub mod list;

use std::io::{self, Write};
use std::path::PathBuf;

use anyhow::bail;
use clap::{Args, Parser, Subcommand};

use crate::probe::Probes;

#[derive(Debug, Parser)]
#[command(
    name = "ratewise",
    version,
    about = "View and set Linux sound-card sample rates, formats and buffer sizes"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Read a captured fixture directory instead of the live system (for development).
    #[arg(long, global = true, hide = true, env = "RATEWISE_FIXTURE")]
    pub fixture: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// List devices, their capabilities and what they are running at now.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Show one device in detail.
    Show {
        /// Stable name, `hw:CARD=…` id, or part of the description.
        device: String,
        #[arg(long)]
        json: bool,
    },
    /// Change a setting.
    Set {
        #[command(subcommand)]
        setting: SetCommand,
    },
    /// Print what a `set` would do, including file diffs, without changing anything.
    Plan {
        #[command(subcommand)]
        setting: SetCommand,
    },
    /// Restore the most recent backup.
    Revert,
    /// Remove all files Ratewise has written.
    Reset,
    /// Report the sound stack, versions, available tools and renderer.
    Doctor,
    /// Save pw-dump, /proc/asound and versions into a new directory, for bug reports and tests.
    CaptureFixture { dir: PathBuf },
}

#[derive(Debug, Subcommand)]
pub enum SetCommand {
    /// Default sample rate, e.g. 96000.
    Rate {
        rate: u32,
        #[arg(long)]
        device: Option<String>,
        #[command(flatten)]
        mode: Mode,
    },
    /// Rates PipeWire may switch between, e.g. 44100,48000,88200,96000.
    AllowedRates {
        #[arg(value_delimiter = ',', required = true)]
        rates: Vec<u32>,
        #[command(flatten)]
        mode: Mode,
    },
    /// Buffer size in frames, e.g. 256.
    Quantum {
        frames: u32,
        #[command(flatten)]
        mode: Mode,
    },
}

/// Whether a change applies to this session only or persists.
#[derive(Debug, Args)]
#[group(multiple = false)]
pub struct Mode {
    /// Apply now, for this session only.
    #[arg(long)]
    pub live: bool,
    /// Save permanently (survives reboots).
    #[arg(long)]
    pub save: bool,
}

pub fn run(cli: Cli) -> anyhow::Result<()> {
    let probes = match &cli.fixture {
        Some(dir) => Probes::fixture(dir),
        None => Probes::system(),
    };
    let not_yet = |what: &str| -> anyhow::Result<()> {
        bail!("{what} isn't available yet. For now, try `ratewise list` or `ratewise doctor`.")
    };
    match cli.command {
        Some(Command::List { json }) => list::run(&probes, json),
        Some(Command::Doctor) => doctor::run(&probes),
        Some(Command::CaptureFixture { dir }) => {
            if cli.fixture.is_some() {
                bail!("capture-fixture always reads the live system. Run it without --fixture.");
            }
            capture::run(&dir)
        }
        None => not_yet("The graphical interface"),
        Some(Command::Show { .. }) => not_yet("`ratewise show`"),
        Some(Command::Set { .. }) => not_yet("`ratewise set`"),
        Some(Command::Plan { .. }) => not_yet("`ratewise plan`"),
        Some(Command::Revert) => not_yet("`ratewise revert`"),
        Some(Command::Reset) => not_yet("`ratewise reset`"),
    }
}

/// Writes to stdout. A closed pipe (`ratewise list | head`) is not an error.
fn emit(text: &str) -> anyhow::Result<()> {
    let mut out = io::stdout().lock();
    match out.write_all(text.as_bytes()).and_then(|()| out.flush()) {
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        r => Ok(r?),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn live_and_save_conflict() {
        let r = Cli::try_parse_from(["ratewise", "set", "quantum", "256", "--live", "--save"]);
        assert!(r.is_err());
    }

    #[test]
    fn allowed_rates_split_on_commas() {
        let cli =
            Cli::try_parse_from(["ratewise", "set", "allowed-rates", "44100,48000", "--save"])
                .unwrap();
        match cli.command {
            Some(Command::Set {
                setting: SetCommand::AllowedRates { rates, mode },
            }) => {
                assert_eq!(rates, vec![44_100, 48_000]);
                assert!(mode.save);
            }
            other => panic!("unexpected parse: {other:?}"),
        }
    }
}
