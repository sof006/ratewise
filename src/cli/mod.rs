//! Command-line interface (spec §7). This is the binary edge: it prints, and uses `anyhow`.

mod capture;
pub mod doctor;
pub mod list;
pub mod set;

use std::io::{self, Write};
use std::path::PathBuf;

use anyhow::bail;
use clap::{Args, Parser, Subcommand};

use crate::app::{Env, Session};
use crate::core::model::SampleFormat;
use crate::core::settings::{Change, LdacQuality, Mode, Value};
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

    /// Write into this folder instead of your home, and record commands instead of running
    /// them (for development and trying changes safely).
    #[arg(long, global = true, hide = true, env = "RATEWISE_SANDBOX")]
    pub sandbox: Option<PathBuf>,

    /// Open the window, render one frame to a PNG, check it, and exit (for launch tests).
    #[arg(long)]
    pub self_test: bool,

    /// Render the window to this PNG and exit (for development). `RATEWISE_PAGE` picks
    /// the page: `device:<id>`, `global`, `doctor` or `review`.
    #[arg(long, hide = true, value_name = "PNG")]
    pub screenshot: Option<PathBuf>,
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
        /// Stable name, `hw:CARD=…` id, or words from the description.
        device: String,
        #[arg(long)]
        json: bool,
    },
    /// Change a setting. Prints what it did; `ratewise revert` undoes it.
    Set {
        #[command(subcommand)]
        setting: SetCommand,
    },
    /// Print what a `set` would do, including file diffs, without changing anything.
    Plan {
        #[command(subcommand)]
        setting: SetCommand,
    },
    /// Undo the most recent change, from its backup. Repeat to go further back.
    Revert {
        #[command(flatten)]
        opts: UndoOpts,
    },
    /// Remove every file Ratewise has written, and release forced live settings.
    Reset {
        #[command(flatten)]
        opts: UndoOpts,
    },
    /// Report the sound stack, versions, available tools and renderer.
    Doctor,
    /// Save pw-dump, /proc/asound and versions into a new directory, for bug reports and tests.
    CaptureFixture { dir: PathBuf },
}

#[derive(Debug, Args)]
pub struct UndoOpts {
    /// Restart audio services afterwards so the change takes effect (sound stops for a moment).
    #[arg(long)]
    pub restart: bool,
    /// Print the plan without changing anything.
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, Subcommand)]
pub enum SetCommand {
    /// Default sample rate (96000 or 96k), or one device's rate with --device.
    Rate {
        #[arg(value_parser = parse_rate)]
        value: Value<u32>,
        #[command(flatten)]
        opts: SetOpts,
    },
    /// Rates PipeWire may switch between to match what's playing, e.g. 44100,48000,96000.
    AllowedRates {
        #[arg(value_parser = parse_rates)]
        value: Value<Vec<u32>>,
        #[command(flatten)]
        opts: SetOpts,
    },
    /// Default buffer size (quantum) in frames, e.g. 256.
    Quantum {
        #[arg(value_parser = parse_frames)]
        value: Value<u32>,
        #[command(flatten)]
        opts: SetOpts,
    },
    /// Smallest quantum programs may ask for.
    MinQuantum {
        #[arg(value_parser = parse_frames)]
        value: Value<u32>,
        #[command(flatten)]
        opts: SetOpts,
    },
    /// Largest quantum programs may ask for.
    MaxQuantum {
        #[arg(value_parser = parse_frames)]
        value: Value<u32>,
        #[command(flatten)]
        opts: SetOpts,
    },
    /// Force one rate right now, for testing (live only). `off` releases it.
    ForceRate {
        #[arg(value_parser = parse_force_rate)]
        value: u32,
        #[command(flatten)]
        opts: SetOpts,
    },
    /// Force one quantum right now, for testing (live only). `off` releases it.
    ForceQuantum {
        #[arg(value_parser = parse_force_frames)]
        value: u32,
        #[command(flatten)]
        opts: SetOpts,
    },
    /// Sample format, e.g. S32_LE or S24_3LE. Per device on PipeWire.
    Format {
        #[arg(value_parser = parse_format)]
        value: Value<SampleFormat>,
        #[command(flatten)]
        opts: SetOpts,
    },
    /// A device's period size in frames. A larger one often fixes USB DAC crackles.
    PeriodSize {
        #[arg(value_parser = parse_frames)]
        value: Value<u32>,
        #[command(flatten)]
        opts: SetOpts,
    },
    /// A device's headroom in frames (extra safety margin, e.g. 1024).
    Headroom {
        #[arg(value_parser = parse_frames_or_zero)]
        value: Value<u32>,
        #[command(flatten)]
        opts: SetOpts,
    },
    /// `off` keeps a device open when idle, which fixes pops when playback starts.
    Suspend {
        #[arg(value_parser = parse_suspend)]
        value: Value<bool>,
        #[command(flatten)]
        opts: SetOpts,
    },
    /// Resampler quality from 0 (fastest) to 14 (best). PipeWire's default is 4.
    ResampleQuality {
        #[arg(value_parser = parse_quality)]
        value: Value<u8>,
        #[command(flatten)]
        opts: SetOpts,
    },
    /// A Bluetooth device's mode or codec: `high-quality`, `headset` (with microphone),
    /// `off`, a codec such as `ldac` or `aac`, or a profile name from `ratewise show`.
    Profile {
        value: String,
        #[command(flatten)]
        opts: SetOpts,
    },
    /// `off` keeps Bluetooth headsets in high quality mode when a program opens the
    /// microphone, instead of switching them to headset mode. Global.
    HeadsetAutoswitch {
        #[arg(value_parser = parse_on_off)]
        value: Value<bool>,
        #[command(flatten)]
        opts: SetOpts,
    },
    /// A Bluetooth device's LDAC bitrate: `hq` (990 kbps), `sq` (660), `mq` (330) or
    /// `auto` (adapts to the connection). Saved only.
    LdacQuality {
        #[arg(value_parser = parse_ldac)]
        value: Value<LdacQuality>,
        #[command(flatten)]
        opts: SetOpts,
    },
}

impl SetCommand {
    pub fn into_parts(self) -> (Change, SetOpts) {
        use SetCommand as S;
        match self {
            S::Rate { value, opts } => (Change::Rate(value), opts),
            S::AllowedRates { value, opts } => (Change::AllowedRates(value), opts),
            S::Quantum { value, opts } => (Change::Quantum(value), opts),
            S::MinQuantum { value, opts } => (Change::MinQuantum(value), opts),
            S::MaxQuantum { value, opts } => (Change::MaxQuantum(value), opts),
            S::ForceRate { value, opts } => (Change::ForceRate(value), opts),
            S::ForceQuantum { value, opts } => (Change::ForceQuantum(value), opts),
            S::Format { value, opts } => (Change::Format(value), opts),
            S::PeriodSize { value, opts } => (Change::PeriodSize(value), opts),
            S::Headroom { value, opts } => (Change::Headroom(value), opts),
            S::Suspend { value, opts } => (Change::Suspend(value), opts),
            S::ResampleQuality { value, opts } => (Change::ResampleQuality(value), opts),
            S::Profile { value, opts } => (Change::BluetoothProfile(value), opts),
            S::HeadsetAutoswitch { value, opts } => (Change::HeadsetAutoswitch(value), opts),
            S::LdacQuality { value, opts } => (Change::LdacQuality(value), opts),
        }
    }
}

#[derive(Debug, Args)]
pub struct SetOpts {
    /// The device to change: stable name, `hw:CARD=…` id, or words from its description.
    #[arg(long)]
    pub device: Option<String>,
    #[command(flatten)]
    pub mode: ModeArgs,
    /// After saving, restart audio services so it takes effect (sound stops for a moment).
    #[arg(long)]
    pub restart: bool,
}

/// Whether a change applies to this session only or persists.
#[derive(Debug, Args)]
#[group(multiple = false)]
pub struct ModeArgs {
    /// Apply now, until audio services restart. The default where possible.
    #[arg(long)]
    pub live: bool,
    /// Save permanently (survives reboots).
    #[arg(long)]
    pub save: bool,
}

impl ModeArgs {
    pub fn mode(&self) -> Option<Mode> {
        match (self.live, self.save) {
            (true, _) => Some(Mode::Live),
            (_, true) => Some(Mode::Save),
            _ => None,
        }
    }
}

// ---- value parsers. `default` removes a saved setting. ----------------------------

fn is_default(s: &str) -> bool {
    matches!(s.trim().to_ascii_lowercase().as_str(), "default" | "unset")
}

fn rate_number(s: &str) -> Result<u32, String> {
    let bad = || format!("\"{s}\" isn't a sample rate. Use Hz (96000) or kHz (96k)");
    let t = s.trim().to_ascii_lowercase();
    let t = t.strip_suffix("hz").unwrap_or(&t).trim();
    if let Some(k) = t.strip_suffix('k') {
        let v: f64 = k.trim().parse().map_err(|_| bad())?;
        let hz = (v * 1000.0).round();
        if !(1.0..=f64::from(u32::MAX)).contains(&hz) {
            return Err(bad());
        }
        return Ok(hz as u32);
    }
    t.parse().map_err(|_| bad())
}

fn parse_rate(s: &str) -> Result<Value<u32>, String> {
    if is_default(s) {
        return Ok(Value::Default);
    }
    rate_number(s).map(Value::Set)
}

fn parse_rates(s: &str) -> Result<Value<Vec<u32>>, String> {
    if is_default(s) {
        return Ok(Value::Default);
    }
    s.split(',')
        .filter(|p| !p.trim().is_empty())
        .map(rate_number)
        .collect::<Result<Vec<_>, _>>()
        .map(Value::Set)
}

fn frames(s: &str, allow_zero: bool) -> Result<u32, String> {
    let n: u32 = s
        .trim()
        .parse()
        .map_err(|_| format!("\"{s}\" isn't a number of frames"))?;
    if n == 0 && !allow_zero {
        return Err("it must be at least 1 frame".into());
    }
    Ok(n)
}

fn parse_frames(s: &str) -> Result<Value<u32>, String> {
    if is_default(s) {
        return Ok(Value::Default);
    }
    frames(s, false).map(Value::Set)
}

fn parse_frames_or_zero(s: &str) -> Result<Value<u32>, String> {
    if is_default(s) {
        return Ok(Value::Default);
    }
    frames(s, true).map(Value::Set)
}

fn is_off(s: &str) -> bool {
    matches!(
        s.trim().to_ascii_lowercase().as_str(),
        "off" | "0" | "default" | "none"
    )
}

fn parse_force_rate(s: &str) -> Result<u32, String> {
    if is_off(s) { Ok(0) } else { rate_number(s) }
}

fn parse_force_frames(s: &str) -> Result<u32, String> {
    if is_off(s) { Ok(0) } else { frames(s, false) }
}

fn parse_format(s: &str) -> Result<Value<SampleFormat>, String> {
    if is_default(s) {
        return Ok(Value::Default);
    }
    let up = s.trim().to_ascii_uppercase();
    let f = match SampleFormat::from_alsa(&up) {
        SampleFormat::Other(_) => SampleFormat::from_pipewire(&up),
        f => f,
    };
    match f {
        SampleFormat::Other(_) => Err(format!(
            "\"{s}\" isn't a format Ratewise knows. Use S16_LE, S24_LE (24-bit in 32), \
             S24_3LE (packed 24-bit), S32_LE or FLOAT_LE"
        )),
        f => Ok(Value::Set(f)),
    }
}

fn parse_suspend(s: &str) -> Result<Value<bool>, String> {
    match s.trim().to_ascii_lowercase().as_str() {
        "off" | "no" | "false" | "never" => Ok(Value::Set(false)),
        "on" | "yes" | "true" => Ok(Value::Set(true)),
        _ if is_default(s) => Ok(Value::Default),
        _ => Err(format!(
            "\"{s}\": use `off` to keep the device open, or `on`"
        )),
    }
}

fn parse_on_off(s: &str) -> Result<Value<bool>, String> {
    match s.trim().to_ascii_lowercase().as_str() {
        "off" | "no" | "false" => Ok(Value::Set(false)),
        "on" | "yes" | "true" => Ok(Value::Set(true)),
        _ if is_default(s) => Ok(Value::Default),
        _ => Err(format!("\"{s}\": use `on`, `off` or `default`")),
    }
}

fn parse_ldac(s: &str) -> Result<Value<LdacQuality>, String> {
    if is_default(s) {
        return Ok(Value::Default);
    }
    LdacQuality::parse(s)
        .map(Value::Set)
        .ok_or_else(|| format!("\"{s}\": use `hq` (990 kbps), `sq` (660), `mq` (330) or `auto`"))
}

fn parse_quality(s: &str) -> Result<Value<u8>, String> {
    if is_default(s) {
        return Ok(Value::Default);
    }
    s.trim()
        .parse::<u8>()
        .ok()
        .filter(|q| *q <= 14)
        .map(Value::Set)
        .ok_or_else(|| format!("\"{s}\": the quality goes from 0 to 14"))
}

pub fn run(cli: Cli) -> anyhow::Result<()> {
    let probes = match &cli.fixture {
        Some(dir) => Probes::fixture(dir),
        None => Probes::system(),
    };
    let fixture = cli.fixture.is_some();
    let sandbox = cli.sandbox.as_deref();
    match cli.command {
        Some(Command::List { json }) => list::run(&probes, json),
        Some(Command::Doctor) => doctor::run(&probes),
        Some(Command::CaptureFixture { dir }) => {
            if fixture {
                bail!("capture-fixture always reads the live system. Run it without --fixture.");
            }
            capture::run(&dir)
        }
        Some(Command::Show { device, json }) => {
            let env = Env::new(fixture, sandbox, false).ok();
            set::show(&probes, env.as_ref().map(|e| &e.paths), &device, json)
        }
        Some(Command::Set { setting }) => {
            let (change, opts) = setting.into_parts();
            let mut session = Session::new(probes, Env::new(fixture, sandbox, true)?);
            let req = set::Request {
                change,
                device: opts.device.as_deref(),
                mode: opts.mode.mode(),
                restart: opts.restart,
                dry_run: false,
            };
            set::set(&mut session, req)
        }
        Some(Command::Plan { setting }) => {
            let (change, opts) = setting.into_parts();
            let mut session = Session::new(probes, Env::new(fixture, sandbox, false)?);
            let req = set::Request {
                change,
                device: opts.device.as_deref(),
                mode: opts.mode.mode(),
                restart: false,
                dry_run: true,
            };
            set::set(&mut session, req)
        }
        Some(Command::Revert { opts }) => {
            let env = Env::new(fixture, sandbox, !opts.dry_run)?;
            set::revert(&mut Session::new(probes, env), opts.restart, opts.dry_run)
        }
        Some(Command::Reset { opts }) => {
            let env = Env::new(fixture, sandbox, !opts.dry_run)?;
            set::reset(&mut Session::new(probes, env), opts.restart, opts.dry_run)
        }
        None => gui(cli.fixture, cli.sandbox, cli.screenshot, cli.self_test),
    }
}

#[cfg(feature = "gui")]
fn gui(
    fixture: Option<PathBuf>,
    sandbox: Option<PathBuf>,
    screenshot: Option<PathBuf>,
    self_test: bool,
) -> anyhow::Result<()> {
    crate::gui::run(crate::gui::Options {
        fixture,
        sandbox,
        screenshot,
        self_test,
    })
}

#[cfg(not(feature = "gui"))]
fn gui(_: Option<PathBuf>, _: Option<PathBuf>, _: Option<PathBuf>, _: bool) -> anyhow::Result<()> {
    bail!(
        "This build of Ratewise has no graphical interface. Try `ratewise list`, \
         `ratewise doctor` or `ratewise set --help`."
    )
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

    fn parse(args: &[&str]) -> (Change, SetOpts) {
        let mut full = vec!["ratewise", "set"];
        full.extend_from_slice(args);
        match Cli::try_parse_from(full).unwrap().command {
            Some(Command::Set { setting }) => setting.into_parts(),
            other => panic!("unexpected parse: {other:?}"),
        }
    }

    #[test]
    fn values_parse_in_friendly_spellings() {
        assert_eq!(parse(&["rate", "96k"]).0, Change::Rate(Value::Set(96_000)));
        assert_eq!(
            parse(&["rate", "44.1kHz"]).0,
            Change::Rate(Value::Set(44_100))
        );
        assert_eq!(parse(&["rate", "default"]).0, Change::Rate(Value::Default));
        assert_eq!(
            parse(&["allowed-rates", "44100,48k,", "--save"]).0,
            Change::AllowedRates(Value::Set(vec![44_100, 48_000]))
        );
        assert_eq!(parse(&["force-rate", "off"]).0, Change::ForceRate(0));
        assert_eq!(
            parse(&["format", "s24_32le", "--device", "x"]).0,
            Change::Format(Value::Set(SampleFormat::S24Le)),
            "PipeWire spelling accepted"
        );
        assert_eq!(
            parse(&["suspend", "off", "--device", "x"]).0,
            Change::Suspend(Value::Set(false))
        );
        assert_eq!(
            parse(&["headroom", "0", "--device", "x"]).0,
            Change::Headroom(Value::Set(0))
        );
        let (_, opts) = parse(&["quantum", "256", "--save", "--restart"]);
        assert_eq!(opts.mode.mode(), Some(Mode::Save));
        assert!(opts.restart);
    }

    #[test]
    fn bad_values_are_rejected_by_the_parser() {
        for args in [
            &["ratewise", "set", "rate", "fast"][..],
            &["ratewise", "set", "quantum", "0"],
            &["ratewise", "set", "format", "DSD"],
            &["ratewise", "set", "resample-quality", "15"],
            &["ratewise", "set", "suspend", "maybe"],
        ] {
            assert!(Cli::try_parse_from(args).is_err(), "{args:?}");
        }
    }
}
