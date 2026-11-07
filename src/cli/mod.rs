//! Command-line interface (spec §7). Commands are wired up milestone by milestone.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "ratewise",
    version,
    about = "View and set Linux sound-card sample rates, formats and buffer sizes"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
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
    /// Dump pw-dump, /proc/asound and versions into a directory for bug reports and tests.
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
