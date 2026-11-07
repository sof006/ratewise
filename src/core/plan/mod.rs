//! Turns a requested change into a [`Plan`]: live commands and file writes, each
//! with a plain-English reason (spec §4). Pure: the caller reads the current
//! files and probes the system, and the backend carries the plan out.

pub mod render;
pub mod verify;

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use similar::TextDiff;

use crate::core::model::{
    Device, DeviceKind, Direction, SampleFormat, SessionManager, Snapshot, SoundServer,
    WirePlumberConfig, khz,
};
use crate::core::paths::Paths;
use crate::core::settings::{Change, Mode, SavedConfig, Value, rate_list};
pub use verify::{Check, ClockKey, ClockValue, Expect, Outcome, render_checks, verify};

/// A program to run, with its arguments.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Cmd {
    pub program: String,
    pub args: Vec<String>,
}

impl Cmd {
    pub fn new(program: &str, args: &[&str]) -> Self {
        Self {
            program: program.into(),
            args: args.iter().map(|a| a.to_string()).collect(),
        }
    }

    /// As you'd type it in a shell.
    pub fn display(&self) -> String {
        let quote = |a: &String| {
            if !a.is_empty()
                && a.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_./=:@".contains(c))
            {
                a.clone()
            } else {
                format!("'{}'", a.replace('\'', r"'\''"))
            }
        };
        std::iter::once(&self.program)
            .chain(&self.args)
            .map(quote)
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Create or replace a file. `old` is `None` for a new file.
    Write {
        path: PathBuf,
        old: Option<String>,
        new: String,
        why: String,
        /// Whether a diff helps the reader (not for Ratewise's own record).
        show_diff: bool,
    },
    Remove {
        path: PathBuf,
        old: String,
        why: String,
    },
    Run {
        cmd: Cmd,
        why: String,
        undo: Option<Cmd>,
    },
}

impl Step {
    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::Write { path, .. } | Self::Remove { path, .. } => Some(path),
            Self::Run { .. } => None,
        }
    }
}

/// Services that must restart before saved changes take effect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Restart {
    /// e.g. "PipeWire, pipewire-pulse and WirePlumber".
    pub services: String,
    /// Empty when Ratewise can't restart them itself.
    pub commands: Vec<Cmd>,
    /// What to do instead when there are no commands.
    pub manual: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanKind {
    Set(Mode),
    Reset,
    Revert,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub kind: PlanKind,
    /// One sentence, e.g. "Set the default sample rate to 96 kHz (saved)."
    pub title: String,
    pub steps: Vec<Step>,
    /// Warnings and explanations for the user.
    pub notes: Vec<String>,
    pub restart: Option<Restart>,
    /// Shown after applying, when nothing needs restarting but something else does.
    pub afterwards: Option<String>,
    /// What to check once it's applied.
    pub expect: Vec<Expect>,
}

impl Plan {
    fn new(kind: PlanKind, title: String) -> Self {
        Self {
            kind,
            title,
            steps: Vec::new(),
            notes: Vec::new(),
            restart: None,
            afterwards: None,
            expect: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }
}

/// A file as the caller found it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileState {
    Missing,
    Text(String),
    /// Ratewise never writes through links: the target may be outside the allowed folders.
    Symlink,
    Unreadable(String),
}

/// Everything a plan needs to know about the system.
pub struct PlanContext<'a> {
    pub snapshot: &'a Snapshot,
    pub paths: &'a Paths,
    pub saved: &'a SavedConfig,
    /// Current state of [`Paths::managed`] files. Missing entries count as missing files.
    pub files: &'a BTreeMap<PathBuf, FileState>,
    /// `/etc/pulse/daemon.conf`, used when a user copy has to be created.
    pub system_pulse_daemon_conf: Option<&'a str>,
    /// Names of installed tools (`pw-metadata`, `systemctl`, ...).
    pub tools: &'a [String],
    /// `$PULSE_CONFIG` is set, so PulseAudio reads its config from somewhere else.
    pub pulse_config_override: bool,
}

impl PlanContext<'_> {
    fn file(&self, p: &Path) -> &FileState {
        self.files.get(p).unwrap_or(&FileState::Missing)
    }
    fn has_tool(&self, name: &str) -> bool {
        self.tools.iter().any(|t| t == name)
    }
}

pub struct SetRequest<'a> {
    pub change: Change,
    pub device: Option<&'a Device>,
    /// `None`: live if possible.
    pub mode: Option<Mode>,
}

/// Why a plan can't be made, in plain English with what to do next.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct PlanError(pub String);

fn refuse<T>(msg: impl Into<String>) -> Result<T, PlanError> {
    Err(PlanError(msg.into()))
}

fn server_name(s: SoundServer) -> &'static str {
    match s {
        SoundServer::PipeWire => "PipeWire",
        SoundServer::PulseAudio => "PulseAudio",
        SoundServer::Jack => "JACK",
        SoundServer::Alsa => "ALSA",
        SoundServer::None => "this system",
    }
}

const MIN_RATE: u32 = 8_000;
const MAX_RATE: u32 = 768_000;
const QUANTUM_LIMIT: u32 = 8_192;

// ---- set ---------------------------------------------------------------------------

pub fn plan_set(ctx: &PlanContext, req: &SetRequest) -> Result<Plan, PlanError> {
    let server = ctx.snapshot.stack.server;
    let change = &req.change.clone().normalised();
    let device = req.device;
    match server {
        SoundServer::Jack => {
            return refuse(
                "A standalone JACK server owns audio. Ratewise shows it read-only; change its \
                 settings with your JACK tool (such as QjackCtl).",
            );
        }
        SoundServer::None => {
            return refuse(
                "No sound server or sound cards were found, so there's nothing to configure. \
                 `ratewise doctor` shows what's missing.",
            );
        }
        _ => {}
    }
    check_scope(server, change, device)?;
    let mode = pick_mode(ctx, change, device, req.mode)?;
    let notes = validate(ctx, change, device)?;
    let mut plan = match mode {
        Mode::Live => plan_live(ctx, change)?,
        Mode::Save => plan_save(ctx, change, device)?,
    };
    plan.notes.splice(0..0, notes);
    Ok(plan)
}

fn check_scope(
    server: SoundServer,
    change: &Change,
    device: Option<&Device>,
) -> Result<(), PlanError> {
    let name = change.name();
    if change.is_device_only() && device.is_none() {
        return refuse(format!(
            "`{name}` is set per device. Add --device <name>; `ratewise list` shows the names."
        ));
    }
    if change.is_global_only() && device.is_some() {
        return refuse(format!(
            "`{name}` is a global setting and can't be set for one device. Leave out --device."
        ));
    }
    let per_device = matches!(change, Change::Rate(_) | Change::Format(_));
    if server == SoundServer::PipeWire && matches!(change, Change::Format(_)) && device.is_none() {
        return refuse(
            "On PipeWire the sample format is set per device. Add --device <name>; \
             `ratewise list` shows the names.",
        );
    }
    if server != SoundServer::PipeWire && per_device && device.is_some() {
        return refuse(format!(
            "{} uses one {} for every device, so it can't be set per device. Leave out --device \
             to change it for all devices.",
            server_name(server),
            if matches!(change, Change::Rate(_)) {
                "sample rate"
            } else {
                "sample format"
            }
        ));
    }
    Ok(())
}

/// Whether `change` can be applied live, and whether it can be saved.
fn support(
    ctx: &PlanContext,
    change: &Change,
    device: Option<&Device>,
) -> (Result<(), String>, Result<(), String>) {
    let server = ctx.snapshot.stack.server;
    let live_clock = matches!(
        change,
        Change::Rate(_)
            | Change::AllowedRates(_)
            | Change::Quantum(_)
            | Change::MinQuantum(_)
            | Change::MaxQuantum(_)
            | Change::ForceRate(_)
            | Change::ForceQuantum(_)
    ) && device.is_none();
    let live = match server {
        SoundServer::PipeWire if live_clock => Ok(()),
        SoundServer::PipeWire if device.is_some() => Err(
            "per-device settings are applied by WirePlumber when it sets the device up".to_string(),
        ),
        SoundServer::PipeWire => Err("PipeWire reads it when a stream starts".to_string()),
        s => Err(format!("{} can't change it while running", server_name(s))),
    };
    let save = match (server, change) {
        (_, Change::ForceRate(_)) => Err(
            "`force-rate` is a live-only setting for testing. To keep a rate, use `rate` \
             (and `allowed-rates`) with --save."
                .to_string(),
        ),
        (_, Change::ForceQuantum(_)) => Err(
            "`force-quantum` is a live-only setting for testing. To keep a buffer size, use \
             `quantum` with --save."
                .to_string(),
        ),
        (SoundServer::PipeWire, _) => match device {
            Some(d) => pipewire_device_support(ctx, d),
            None => Ok(()),
        },
        (SoundServer::PulseAudio, _) if ctx.pulse_config_override => Err(
            "PULSE_CONFIG is set, so PulseAudio reads its settings from that file instead of \
             ~/.config/pulse. Unset it, or change the file it points to yourself."
                .to_string(),
        ),
        (
            SoundServer::PulseAudio,
            Change::Rate(_) | Change::AllowedRates(_) | Change::Format(_),
        ) => Ok(()),
        (
            SoundServer::PulseAudio,
            Change::Quantum(_) | Change::MinQuantum(_) | Change::MaxQuantum(_),
        ) => Err(
            "PulseAudio picks buffer sizes per program, so there's no quantum to set.".to_string(),
        ),
        (SoundServer::PulseAudio, Change::ResampleQuality(_)) => Err(
            "Ratewise doesn't set PulseAudio's resampler. Set `resample-method` in \
             ~/.config/pulse/daemon.conf yourself (for example soxr-hq)."
                .to_string(),
        ),
        (SoundServer::PulseAudio, _) => Err(format!(
            "PulseAudio's config files have no per-device `{}`.",
            change.name()
        )),
        (SoundServer::Alsa, Change::Rate(_) | Change::Format(_)) => Ok(()),
        (SoundServer::Alsa, Change::PeriodSize(_)) => match device {
            Some(d) => alsa_device_support(d),
            None => Ok(()),
        },
        (SoundServer::Alsa, Change::AllowedRates(_)) => {
            Err("Without a sound server nothing switches rates. Set `rate` instead.".to_string())
        }
        (SoundServer::Alsa, _) => Err(format!(
            "Without a sound server, ALSA only offers a global rate and format and a per-card \
             period size, so `{}` can't be set.",
            change.name()
        )),
        (s, _) => Err(format!(
            "{} can't be configured by Ratewise.",
            server_name(s)
        )),
    };
    (live, save)
}

fn pipewire_device_support(ctx: &PlanContext, d: &Device) -> Result<(), String> {
    if d.kind == DeviceKind::Bluetooth {
        return Err(format!(
            "{} is a Bluetooth device. Its rate and format come from the Bluetooth codec, so these \
             ALSA settings don't apply to it.",
            d.description
        ));
    }
    if d.pipewire.is_none() {
        return Err(format!(
            "PipeWire isn't using {}: its card's profile doesn't include it, so a rule for it \
             would have no effect. Pick a profile that includes it (in your sound settings), then \
             try again.",
            d.description
        ));
    }
    match &ctx.snapshot.stack.session_manager {
        Some(SessionManager::WirePlumber {
            config: Some(_), ..
        }) => Ok(()),
        Some(SessionManager::WirePlumber { config: None, .. }) | None => Err(
            "Ratewise can't tell which WirePlumber version is running, so it doesn't know which \
             config format to write. `ratewise doctor` shows what it found."
                .to_string(),
        ),
        Some(SessionManager::PipewireMediaSession) => Err(
            "pipewire-media-session is deprecated and Ratewise doesn't write rules for it. \
             Switch to WirePlumber to set per-device options."
                .to_string(),
        ),
    }
}

fn alsa_device_support(d: &Device) -> Result<(), String> {
    match d.alsa.as_ref().and_then(|a| a.card_id.as_deref()) {
        Some(id) if render::alsa_key_safe(id) => Ok(()),
        Some(id) => Err(format!(
            "The card id \"{id}\" can't be used in an ALSA config key. Set the period size in \
             ~/.asoundrc yourself."
        )),
        None => Err(format!("The ALSA card id of {} is unknown.", d.description)),
    }
}

fn pick_mode(
    ctx: &PlanContext,
    change: &Change,
    device: Option<&Device>,
    asked: Option<Mode>,
) -> Result<Mode, PlanError> {
    let (live, save) = support(ctx, change, device);
    let name = change.name();
    match asked {
        Some(Mode::Live) => match live {
            Ok(()) => Ok(Mode::Live),
            Err(why) => match save {
                Ok(()) => refuse(format!(
                    "`{name}` can't be changed live: {why}. Use --save instead; it takes effect \
                     after audio services restart."
                )),
                Err(save_why) => refuse(save_why),
            },
        },
        Some(Mode::Save) => save.map(|()| Mode::Save).map_err(PlanError),
        None => match (live, save) {
            (Ok(()), _) => Ok(Mode::Live),
            (Err(why), Ok(())) => refuse(format!(
                "`{name}` can't be changed live: {why}. Add --save to save it; it takes effect \
                 after audio services restart."
            )),
            (Err(_), Err(save_why)) => refuse(save_why),
        },
    }
}

/// Checks values. Returns warnings; errors stop the plan.
fn validate(
    ctx: &PlanContext,
    change: &Change,
    device: Option<&Device>,
) -> Result<Vec<String>, PlanError> {
    let server = ctx.snapshot.stack.server;
    let saved = &ctx.saved.global;
    let clock = ctx.snapshot.clock.as_ref().map(|c| &c.value);
    let mut notes = Vec::new();
    let valid_rate = |r: u32| -> Result<(), PlanError> {
        if (MIN_RATE..=MAX_RATE).contains(&r) {
            Ok(())
        } else {
            refuse(format!(
                "{r} Hz isn't a usable sample rate. Pick one between 8 and 768 kHz, such as \
                 44100, 48000 or 96000."
            ))
        }
    };
    let valid_quantum = |name: &str, q: u32| -> Result<(), PlanError> {
        if (1..=QUANTUM_LIMIT).contains(&q) {
            Ok(())
        } else {
            refuse(format!(
                "`{name}` must be between 1 and {QUANTUM_LIMIT} frames."
            ))
        }
    };
    let pow2_note = |q: u32| {
        (!q.is_power_of_two()).then(|| {
            format!(
                "{q} isn't a power of two. PipeWire rounds quantums down to one, so it will use {}.",
                1u32 << (31 - q.leading_zeros())
            )
        })
    };
    let hardware = || ctx.snapshot.devices.iter().filter(|d| d.alsa.is_some());

    match change {
        Change::Rate(Value::Set(r)) => {
            valid_rate(*r)?;
            match device {
                Some(d) => {
                    if let Some(rates) = &d.capabilities.rates
                        && !rates.value.contains(*r)
                    {
                        return refuse(format!(
                            "{} can't run at {} kHz. It supports {} [from {}].",
                            d.description,
                            khz(*r),
                            rates_text(&rates.value),
                            rates.source
                        ));
                    }
                }
                None => {
                    let cant: Vec<&str> = hardware()
                        .filter(|d| {
                            d.capabilities
                                .rates
                                .as_ref()
                                .is_some_and(|c| !c.value.contains(*r))
                        })
                        .map(|d| d.description.as_str())
                        .collect();
                    if !cant.is_empty() {
                        notes.push(match server {
                            SoundServer::Alsa => format!(
                                "These devices can't run at {} kHz, so programs using ALSA's dmix \
                                 on them will fail to open: {}.",
                                khz(*r),
                                cant.join(", ")
                            ),
                            s => format!(
                                "These devices can't run at {} kHz, so {} will resample for them: {}.",
                                khz(*r),
                                server_name(s),
                                cant.join(", ")
                            ),
                        });
                    }
                    if server == SoundServer::PipeWire
                        && let Some(allowed) = saved.allowed_rates.as_ref()
                        && !allowed.contains(r)
                    {
                        notes.push(format!(
                            "{} kHz isn't in your saved allowed rates ({} kHz).",
                            khz(*r),
                            rate_list(allowed)
                        ));
                    }
                    if server == SoundServer::PulseAudio
                        && let Some(allowed) = saved.allowed_rates.as_ref()
                        && !allowed.contains(r)
                        && allowed.len() == 2
                    {
                        return refuse(format!(
                            "PulseAudio switches between two rates, and your saved allowed rates \
                             are {} kHz. Pick one of them, or change allowed-rates first.",
                            rate_list(allowed)
                        ));
                    }
                }
            }
        }
        Change::AllowedRates(Value::Set(rs)) => {
            if rs.is_empty() {
                return refuse("Give at least one rate, e.g. 44100,48000.");
            }
            for &r in rs {
                valid_rate(r)?;
            }
            let mut sorted = rs.clone();
            sorted.sort_unstable();
            sorted.dedup();
            if sorted.len() > 32 {
                return refuse("PipeWire accepts up to 32 allowed rates.");
            }
            if server == SoundServer::PulseAudio {
                if sorted.len() > 2 {
                    return refuse(
                        "PulseAudio can switch between only two rates: the default and one \
                         alternate. Give one or two rates.",
                    );
                }
                if let Some(r) = saved.rate
                    && sorted.len() == 2
                    && !sorted.contains(&r)
                {
                    return refuse(format!(
                        "Your saved default rate is {} kHz, so include it: PulseAudio switches \
                         between the default and one alternate rate.",
                        khz(r)
                    ));
                }
            }
            if server == SoundServer::PipeWire {
                let default = saved.rate.or(clock.and_then(|c| c.rate));
                if let Some(d) = default
                    && !sorted.contains(&d)
                {
                    notes.push(format!(
                        "The default rate ({} kHz) isn't in the list. PipeWire still starts at it \
                         and switches when a stream asks for a listed rate.",
                        khz(d)
                    ));
                }
                notes.push(
                    "PipeWire switches rate only while the device is idle, and only to rates that \
                     device supports."
                        .into(),
                );
            }
        }
        Change::Quantum(Value::Set(q)) => {
            valid_quantum("quantum", *q)?;
            let min = saved.min_quantum.or(clock.and_then(|c| c.min_quantum));
            let max = saved.max_quantum.or(clock.and_then(|c| c.max_quantum));
            if let Some(min) = min
                && *q < min
            {
                return refuse(format!(
                    "{q} is below the minimum quantum ({min}). Lower min-quantum first."
                ));
            }
            if let Some(max) = max
                && *q > max
            {
                return refuse(format!(
                    "{q} is above the maximum quantum ({max}). Raise max-quantum first."
                ));
            }
            notes.extend(pow2_note(*q));
            if let Some(rate) = saved.rate.or(clock.and_then(|c| c.rate)) {
                notes.push(format!(
                    "{q} frames at {} kHz is {:.1} ms. Lower means less latency but more risk of \
                     crackles.",
                    khz(rate),
                    f64::from(*q) * 1000.0 / f64::from(rate)
                ));
            }
        }
        Change::MinQuantum(Value::Set(q)) => {
            valid_quantum("min-quantum", *q)?;
            if let Some(quantum) = saved.quantum
                && *q > quantum
            {
                return refuse(format!(
                    "{q} is above your saved quantum ({quantum}). Lower the quantum first."
                ));
            }
            notes.extend(pow2_note(*q));
        }
        Change::MaxQuantum(Value::Set(q)) => {
            valid_quantum("max-quantum", *q)?;
            if let Some(quantum) = saved.quantum
                && *q < quantum
            {
                return refuse(format!(
                    "{q} is below your saved quantum ({quantum}). Raise the quantum first."
                ));
            }
            notes.extend(pow2_note(*q));
        }
        Change::ForceRate(r) if *r > 0 => valid_rate(*r)?,
        Change::ForceQuantum(q) if *q > 0 => valid_quantum("force-quantum", *q)?,
        Change::Format(Value::Set(f)) => {
            let known = match server {
                SoundServer::PipeWire => f.pipewire_name().is_some(),
                SoundServer::PulseAudio => f.pulse_name().is_some(),
                _ => !matches!(f, SampleFormat::Other(_)),
            };
            if !known {
                return refuse(format!(
                    "Ratewise can't set {f} on {}. Pick one of S16_LE, S24_LE, S24_3LE, S32_LE \
                     or FLOAT_LE.",
                    server_name(server)
                ));
            }
            match device {
                Some(d) => {
                    if let Some(formats) = &d.capabilities.formats
                        && !formats.value.contains(f)
                    {
                        let list: Vec<String> =
                            formats.value.iter().map(|f| f.to_string()).collect();
                        return refuse(format!(
                            "{} doesn't support {f}. It supports {} [from {}].",
                            d.description,
                            list.join(", "),
                            formats.source
                        ));
                    }
                }
                None => {
                    let cant: Vec<&str> = hardware()
                        .filter(|d| {
                            d.capabilities
                                .formats
                                .as_ref()
                                .is_some_and(|c| !c.value.contains(f))
                        })
                        .map(|d| d.description.as_str())
                        .collect();
                    if !cant.is_empty() {
                        notes.push(format!(
                            "These devices don't support {f}{}: {}.",
                            if server == SoundServer::Alsa {
                                ", so dmix will fail to open them"
                            } else {
                                ", so it will be converted for them"
                            },
                            cant.join(", ")
                        ));
                    }
                }
            }
        }
        Change::PeriodSize(Value::Set(n)) => {
            if *n == 0 {
                return refuse("The period size must be at least 1 frame.");
            }
            if let Some(d) = device
                && let Some(range) = &d.capabilities.period_frames
                && !(range.value.min..=range.value.max).contains(n)
            {
                return refuse(format!(
                    "{} accepts a period size between {} and {} frames [from {}].",
                    d.description, range.value.min, range.value.max, range.source
                ));
            }
            if let Some(d) = device
                && d.direction == Direction::Duplex
                && server == SoundServer::Alsa
            {
                return refuse("Pick the playback or the capture side of this device.");
            }
        }
        Change::Headroom(Value::Set(n)) if *n > 1 << 20 => {
            return refuse(
                "Headroom above 1048576 frames isn't useful; typical values are 256 to 8192.",
            );
        }
        Change::ResampleQuality(Value::Set(q)) if *q > 14 => {
            return refuse(
                "The resampler quality goes from 0 (fastest) to 14 (best). PipeWire's default is 4.",
            );
        }
        _ => {}
    }
    Ok(notes)
}

fn rates_text(r: &crate::core::model::RateSet) -> String {
    match r {
        crate::core::model::RateSet::Discrete { rates } => format!("{} kHz", rate_list(rates)),
        crate::core::model::RateSet::Range { min, max } => {
            format!("{}–{} kHz", khz(*min), khz(*max))
        }
    }
}

fn clock_change(change: &Change) -> Option<(ClockKey, Option<ClockValue>)> {
    let n = |v: &Value<u32>| v.as_option().map(|&n| ClockValue::Number(n));
    Some(match change {
        Change::Rate(v) => (ClockKey::Rate, n(v)),
        Change::AllowedRates(v) => (
            ClockKey::AllowedRates,
            v.as_option().map(|rs| {
                let mut rs = rs.clone();
                rs.sort_unstable();
                rs.dedup();
                ClockValue::Rates(rs)
            }),
        ),
        Change::Quantum(v) => (ClockKey::Quantum, n(v)),
        Change::MinQuantum(v) => (ClockKey::MinQuantum, n(v)),
        Change::MaxQuantum(v) => (ClockKey::MaxQuantum, n(v)),
        Change::ForceRate(r) => (ClockKey::ForceRate, Some(ClockValue::Number(*r))),
        Change::ForceQuantum(q) => (ClockKey::ForceQuantum, Some(ClockValue::Number(*q))),
        _ => return None,
    })
}

/// `pw-metadata` setting a key, or deleting it (back to the configured default).
fn metadata_cmd(key: ClockKey, value: Option<&ClockValue>) -> Cmd {
    match value {
        Some(v) => Cmd::new(
            "pw-metadata",
            &[
                "-n",
                "settings",
                "0",
                key.metadata_key(),
                &v.metadata_text(),
            ],
        ),
        None => Cmd::new(
            "pw-metadata",
            &["-n", "settings", "-d", "0", key.metadata_key()],
        ),
    }
}

fn current_clock(ctx: &PlanContext, key: ClockKey) -> Option<ClockValue> {
    let c = &ctx.snapshot.clock.as_ref()?.value;
    let n = |v: Option<u32>| v.map(ClockValue::Number);
    match key {
        ClockKey::Rate => n(c.rate),
        ClockKey::AllowedRates => c.allowed_rates.clone().map(ClockValue::Rates),
        ClockKey::Quantum => n(c.quantum),
        ClockKey::MinQuantum => n(c.min_quantum),
        ClockKey::MaxQuantum => n(c.max_quantum),
        ClockKey::ForceRate => Some(ClockValue::Number(c.force_rate.unwrap_or(0))),
        ClockKey::ForceQuantum => Some(ClockValue::Number(c.force_quantum.unwrap_or(0))),
    }
}

fn plan_live(ctx: &PlanContext, change: &Change) -> Result<Plan, PlanError> {
    if !ctx.has_tool("pw-metadata") {
        return refuse(
            "pw-metadata isn't installed, so live changes aren't possible. Install your \
             distribution's PipeWire tools, or use --save.",
        );
    }
    let (key, value) =
        clock_change(change).ok_or_else(|| PlanError("not a live setting".into()))?;
    let mut plan = Plan::new(
        PlanKind::Set(Mode::Live),
        format!(
            "{} (live, until PipeWire restarts).",
            capitalise(&change.describe(None))
        ),
    );
    let previous = current_clock(ctx, key);
    plan.steps.push(Step::Run {
        cmd: metadata_cmd(key, value.as_ref()),
        why: format!("Changes PipeWire's {} setting now.", key.metadata_key()),
        undo: Some(metadata_cmd(key, previous.as_ref())),
    });
    if let Some(v) = value {
        plan.expect.push(Expect::Clock { key, want: v });
    }
    if !matches!(change, Change::ForceRate(_) | Change::ForceQuantum(_)) {
        plan.afterwards = Some(
            "Live changes last until PipeWire restarts. To keep it, run the same command with \
             --save."
                .into(),
        );
    }
    if matches!(key, ClockKey::Rate | ClockKey::AllowedRates) {
        plan.notes.push(
            "PipeWire applies a new default rate the next time the device goes idle and starts \
             again. To switch immediately, use `force-rate`."
                .into(),
        );
    }
    Ok(plan)
}

fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

/// Files that should exist (or not) for `saved`, on this stack.
struct Desired {
    files: Vec<(PathBuf, Option<String>, &'static str)>,
    saved: SavedConfig,
}

fn desired_files(ctx: &PlanContext, saved: &SavedConfig) -> Result<Desired, PlanError> {
    let p = ctx.paths;
    let mut saved = saved.clone();
    let mut files: Vec<(PathBuf, Option<String>, &'static str)> = Vec::new();
    match ctx.snapshot.stack.server {
        SoundServer::PipeWire => {
            files.push((
                p.pipewire_dropin(),
                render::pipewire(&saved.global),
                "PipeWire's clock settings.",
            ));
            let streams = render::stream_properties(&saved.global);
            files.push((
                p.client_dropin(),
                streams.clone(),
                "Resampler quality for PipeWire programs.",
            ));
            files.push((
                p.pipewire_pulse_dropin(),
                streams.clone(),
                "Resampler quality for PulseAudio programs on PipeWire.",
            ));
            let old_pipewire = ctx
                .snapshot
                .stack
                .pipewire_version
                .as_deref()
                .is_some_and(|v| v.starts_with("0."));
            files.push((
                p.client_rt_dropin(),
                streams.filter(|_| old_pipewire),
                "Resampler quality for ALSA-plugin programs (PipeWire 0.3).",
            ));
            let config = match &ctx.snapshot.stack.session_manager {
                Some(SessionManager::WirePlumber { config, .. }) => *config,
                _ => None,
            };
            let rules05 = render::wireplumber_05(&saved.devices);
            let rules04 = render::wireplumber_04(&saved.devices);
            let why = "WirePlumber rules for per-device settings.";
            match config {
                Some(WirePlumberConfig::SpaJson05) => {
                    files.push((p.wireplumber_05(), rules05, why));
                    files.push((
                        p.wireplumber_04(),
                        None,
                        "WirePlumber 0.4 rules, replaced by the 0.5 file.",
                    ));
                }
                Some(WirePlumberConfig::Lua04) => {
                    files.push((p.wireplumber_04(), rules04, why));
                    files.push((
                        p.wireplumber_05(),
                        None,
                        "WirePlumber 0.5 rules, replaced by the 0.4 file.",
                    ));
                }
                None if rules05.is_some() => {
                    return refuse(
                        "Ratewise can't tell which WirePlumber version is running, so it doesn't \
                         know which config format to write.",
                    );
                }
                None => {}
            }
        }
        SoundServer::PulseAudio => {
            let dropin = render::pulse(&saved.global);
            let daemon_conf = p.pulse_daemon_conf();
            match (&dropin, ctx.file(&daemon_conf)) {
                (Some(_), FileState::Missing) => {
                    let copy = render::pulse_daemon_conf_copy(ctx.system_pulse_daemon_conf);
                    saved.created_pulse_daemon_conf = Some(render::fingerprint(&copy));
                    files.push((
                        daemon_conf,
                        Some(copy),
                        "PulseAudio reads ~/.config/pulse/daemon.conf.d/ only when this file exists, \
                         so Ratewise creates it as a copy of the system file.",
                    ));
                }
                (None, FileState::Text(text))
                    if saved.created_pulse_daemon_conf.as_deref()
                        == Some(&render::fingerprint(text)) =>
                {
                    saved.created_pulse_daemon_conf = None;
                    files.push((
                        daemon_conf,
                        None,
                        "Ratewise created this copy and nobody has edited it.",
                    ));
                }
                _ => {}
            }
            files.push((
                p.pulse_dropin(),
                dropin,
                "PulseAudio's sample rate and format.",
            ));
        }
        SoundServer::Alsa => {
            let asoundrc = p.asoundrc();
            let current = match ctx.file(&asoundrc) {
                FileState::Text(t) => Some(t.as_str()),
                _ => None,
            };
            let body = render::alsa_block(&saved.global, &saved.devices);
            let next = render::splice_block(current, body.as_deref()).map_err(|e| {
                PlanError(format!(
                    "Ratewise won't edit ~/.asoundrc: {e}. Fix or delete that line, then try again."
                ))
            })?;
            files.push((
                asoundrc,
                next,
                "ALSA's dmix and dsnoop defaults, in a marked block.",
            ));
        }
        SoundServer::Jack | SoundServer::None => {}
    }
    Ok(Desired { files, saved })
}

/// Steps that turn the current files into the desired ones, plus the record.
fn file_steps(ctx: &PlanContext, desired: &Desired) -> Result<Vec<Step>, PlanError> {
    let mut steps = Vec::new();
    for (path, want, why) in &desired.files {
        if let Some(step) = file_step(ctx, path, want.as_deref(), why, true)? {
            steps.push(step);
        }
    }
    if !steps.is_empty() || desired.saved != *ctx.saved {
        let record = (!desired.saved.is_empty()
            || desired.saved.created_pulse_daemon_conf.is_some())
        .then(|| {
            serde_json::to_string_pretty(&desired.saved).expect("saved config serializes") + "\n"
        });
        if let Some(step) = file_step(
            ctx,
            &ctx.paths.saved(),
            record.as_deref(),
            "Ratewise's record of what it has saved.",
            false,
        )? {
            steps.push(step);
        }
    }
    Ok(steps)
}

fn file_step(
    ctx: &PlanContext,
    path: &Path,
    want: Option<&str>,
    why: &str,
    show_diff: bool,
) -> Result<Option<Step>, PlanError> {
    let shown = ctx.paths.display(path);
    let current = match ctx.file(path) {
        FileState::Symlink => {
            return refuse(format!(
                "{shown} is a symbolic link. Ratewise doesn't write through links, because the \
                 real file may be outside your config folders. Replace the link with a regular \
                 file, or make this change yourself."
            ));
        }
        FileState::Unreadable(e) => return refuse(format!("Couldn't read {shown}: {e}.")),
        FileState::Missing => None,
        FileState::Text(t) => Some(t.as_str()),
    };
    Ok(match (current, want) {
        (None, None) => None,
        (Some(old), Some(new)) if old == new => None,
        (Some(old), None) => Some(Step::Remove {
            path: path.to_path_buf(),
            old: old.to_string(),
            why: why.to_string(),
        }),
        (old, Some(new)) => Some(Step::Write {
            path: path.to_path_buf(),
            old: old.map(String::from),
            new: new.to_string(),
            why: why.to_string(),
            show_diff,
        }),
    })
}

fn plan_save(
    ctx: &PlanContext,
    change: &Change,
    device: Option<&Device>,
) -> Result<Plan, PlanError> {
    let next = ctx.saved.with(change, device);
    let desired = desired_files(ctx, &next)?;
    let mut plan = Plan::new(
        PlanKind::Set(Mode::Save),
        format!("{} (saved).", capitalise(&change.describe(device))),
    );
    plan.steps = file_steps(ctx, &desired)?;
    if plan.steps.is_empty() {
        plan.notes
            .push("That's already saved, so nothing needs to change.".into());
        return Ok(plan);
    }
    finish(ctx, &mut plan);
    if matches!(change, Change::ResampleQuality(_)) {
        plan.afterwards = Some(
            "Programs read the resampler quality when they start playing, so restart any that \
             are already playing to hear the change."
                .into(),
        );
    }
    if let Some(asoundrc) = desired.files.iter().find(|f| f.0 == ctx.paths.asoundrc())
        && let Some(text) = &asoundrc.1
    {
        let later =
            render::overrides_after_block(text, &["defaults.pcm.dmix", "defaults.dmix", "cards."]);
        if !later.is_empty() {
            plan.notes.push(format!(
                "~/.asoundrc sets these after Ratewise's block, so they win: {}.",
                later.join("; ")
            ));
        }
    }
    // What to check after a restart.
    if ctx.snapshot.stack.server == SoundServer::PipeWire {
        match (device, clock_change(change)) {
            (None, Some((key, Some(want)))) => plan.expect.push(Expect::Clock { key, want }),
            (Some(d), _) => match change {
                Change::Rate(Value::Set(r)) => plan.expect.push(Expect::DeviceRate {
                    id: d.id.clone(),
                    description: d.description.clone(),
                    rate: *r,
                }),
                Change::Format(Value::Set(f)) => plan.expect.push(Expect::DeviceFormat {
                    id: d.id.clone(),
                    description: d.description.clone(),
                    format: f.clone(),
                }),
                _ => {}
            },
            _ => {}
        }
    }
    Ok(plan)
}

/// Works out what must restart for the plan's file changes to apply.
fn finish(ctx: &PlanContext, plan: &mut Plan) {
    let p = ctx.paths;
    let stack = &ctx.snapshot.stack;
    let touched: Vec<&Path> = plan.steps.iter().filter_map(Step::path).collect();
    let touches = |paths: &[PathBuf]| touched.iter().any(|t| paths.iter().any(|p| p == t));
    let systemd = stack.systemd_user_session == Some(true) && ctx.has_tool("systemctl");
    let pipewire_files = [
        p.pipewire_dropin(),
        p.client_dropin(),
        p.client_rt_dropin(),
        p.pipewire_pulse_dropin(),
        p.wireplumber_05(),
        p.wireplumber_04(),
    ];
    match stack.server {
        SoundServer::PipeWire if touches(&pipewire_files) => {
            let mut units = vec!["pipewire.service"];
            let mut names = vec!["PipeWire"];
            if stack.pipewire_pulse {
                units.push("pipewire-pulse.service");
                names.push("pipewire-pulse");
            }
            match stack.session_manager {
                Some(SessionManager::PipewireMediaSession) => {
                    units.push("pipewire-media-session.service");
                    names.push("pipewire-media-session");
                }
                _ => {
                    units.push("wireplumber.service");
                    names.push("WirePlumber");
                }
            }
            let mut args = vec!["--user", "restart"];
            args.extend(units);
            plan.restart = Some(Restart {
                services: join_and(&names),
                commands: if systemd { vec![Cmd::new("systemctl", &args)] } else { vec![] },
                manual: (!systemd).then(|| {
                    "This system has no systemd user session, so log out and back in for it to take effect."
                        .to_string()
                }),
            });
        }
        SoundServer::PulseAudio if touches(&[p.pulse_dropin(), p.pulse_daemon_conf()]) => {
            plan.restart = Some(Restart {
                services: "PulseAudio".into(),
                commands: if systemd {
                    vec![Cmd::new(
                        "systemctl",
                        &["--user", "restart", "pulseaudio.service"],
                    )]
                } else {
                    vec![
                        Cmd::new("pulseaudio", &["-k"]),
                        Cmd::new("pulseaudio", &["--start"]),
                    ]
                },
                manual: None,
            });
        }
        SoundServer::Alsa if touches(&[p.asoundrc()]) => {
            plan.afterwards = Some(
                "ALSA reads ~/.asoundrc when a program opens a device, so restart programs that \
                 are playing or recording to pick it up."
                    .into(),
            );
        }
        _ => {}
    }
}

fn join_and(items: &[&str]) -> String {
    match items {
        [] => String::new(),
        [one] => one.to_string(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

// ---- reset and revert --------------------------------------------------------------

/// Removes everything Ratewise has written, and releases any forced live setting.
pub fn plan_reset(ctx: &PlanContext) -> Result<Plan, PlanError> {
    let p = ctx.paths;
    let mut plan = Plan::new(
        PlanKind::Reset,
        "Remove every setting Ratewise has saved.".into(),
    );
    let own = [
        p.pipewire_dropin(),
        p.client_dropin(),
        p.client_rt_dropin(),
        p.pipewire_pulse_dropin(),
        p.wireplumber_05(),
        p.wireplumber_04(),
        p.pulse_dropin(),
    ];
    for path in own {
        match ctx.file(&path) {
            FileState::Text(old) => plan.steps.push(Step::Remove {
                path,
                old: old.clone(),
                why: "Ratewise's own file.".into(),
            }),
            FileState::Symlink => plan.notes.push(format!(
                "Left {} alone: it's a symbolic link, so Ratewise didn't write it.",
                p.display(&path)
            )),
            _ => {}
        }
    }
    let asoundrc = p.asoundrc();
    if let FileState::Text(text) = ctx.file(&asoundrc) {
        match render::splice_block(Some(text), None) {
            Ok(next) => {
                if let Some(step) = file_step(
                    ctx,
                    &asoundrc,
                    next.as_deref(),
                    "Removes Ratewise's block; everything else stays.",
                    true,
                )? {
                    plan.steps.push(step);
                }
            }
            Err(e) => plan.notes.push(format!(
                "Left ~/.asoundrc alone: {e}. Remove Ratewise's lines from it yourself."
            )),
        }
    }
    let daemon_conf = p.pulse_daemon_conf();
    if let (Some(hash), FileState::Text(text)) =
        (&ctx.saved.created_pulse_daemon_conf, ctx.file(&daemon_conf))
    {
        if *hash == render::fingerprint(text) {
            plan.steps.push(Step::Remove {
                path: daemon_conf,
                old: text.clone(),
                why: "Ratewise created this copy and nobody has edited it.".into(),
            });
        } else {
            plan.notes.push(
                "Kept ~/.config/pulse/daemon.conf: Ratewise created it, but it has been edited since.".into(),
            );
        }
    }
    if let FileState::Text(old) = ctx.file(&p.saved()) {
        plan.steps.push(Step::Remove {
            path: p.saved(),
            old: old.clone(),
            why: "Ratewise's record of what it has saved.".into(),
        });
    }
    if ctx.snapshot.stack.server == SoundServer::PipeWire && ctx.has_tool("pw-metadata") {
        for key in [ClockKey::ForceRate, ClockKey::ForceQuantum] {
            let current = current_clock(ctx, key);
            if current
                .as_ref()
                .is_some_and(|v| *v != ClockValue::Number(0))
            {
                plan.steps.push(Step::Run {
                    cmd: metadata_cmd(key, Some(&ClockValue::Number(0))),
                    why: format!("Releases the live {} setting.", key.metadata_key()),
                    undo: Some(metadata_cmd(key, current.as_ref())),
                });
                plan.expect.push(Expect::Clock {
                    key,
                    want: ClockValue::Number(0),
                });
            }
        }
    }
    if plan.steps.is_empty() {
        plan.notes
            .push("Ratewise hasn't saved anything, so there's nothing to reset.".into());
    }
    finish(ctx, &mut plan);
    Ok(plan)
}

/// A backup as the caller loaded it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupView {
    pub id: String,
    pub description: String,
    /// Each file's contents before the change; `None` if it didn't exist.
    pub files: Vec<(PathBuf, Option<String>)>,
    /// Commands that undo the live part of the change.
    pub live_undo: Vec<Cmd>,
}

/// Puts files back as they were before the backed-up change.
pub fn plan_revert(ctx: &PlanContext, backup: &BackupView) -> Result<Plan, PlanError> {
    let mut plan = Plan::new(
        PlanKind::Revert,
        format!(
            "Undo “{}” (backup {}).",
            backup.description.trim_end_matches('.'),
            backup.id
        ),
    );
    for (path, before) in &backup.files {
        if !ctx.paths.is_allowed(path) {
            return refuse(format!(
                "Backup {} names {}, which is outside the folders Ratewise may write. It may have \
                 been edited by hand, so Ratewise won't restore it.",
                backup.id,
                path.display()
            ));
        }
        let show_diff = *path != ctx.paths.saved();
        if let Some(step) = file_step(
            ctx,
            path,
            before.as_deref(),
            "Restored from the backup.",
            show_diff,
        )? {
            plan.steps.push(step);
        }
    }
    for cmd in &backup.live_undo {
        plan.steps.push(Step::Run {
            cmd: cmd.clone(),
            why: "Restores the live setting.".into(),
            undo: None,
        });
    }
    if plan.steps.is_empty() {
        plan.notes
            .push("Everything already matches the backup.".into());
    }
    finish(ctx, &mut plan);
    Ok(plan)
}

// ---- text --------------------------------------------------------------------------

/// The plan as `ratewise plan` prints it, with file diffs.
pub fn render_plan(plan: &Plan, paths: &Paths) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "{}", plan.title);
    if !plan.steps.is_empty() {
        let _ = writeln!(s, "\nSteps");
        for (i, step) in plan.steps.iter().enumerate() {
            let n = i + 1;
            match step {
                Step::Write {
                    path,
                    old,
                    new,
                    why,
                    show_diff,
                } => {
                    let verb = if old.is_some() { "Update" } else { "Create" };
                    let _ = writeln!(s, "  {n}. {verb} {}", paths.display(path));
                    let _ = writeln!(s, "     {why}");
                    if *show_diff {
                        s.push_str(&diff(old.as_deref().unwrap_or(""), new));
                    }
                }
                Step::Remove { path, old, why } => {
                    let _ = writeln!(s, "  {n}. Remove {}", paths.display(path));
                    let _ = writeln!(s, "     {why}");
                    if *path != paths.saved() {
                        s.push_str(&diff(old, ""));
                    }
                }
                Step::Run { cmd, why, .. } => {
                    let _ = writeln!(s, "  {n}. Run: {}", cmd.display());
                    let _ = writeln!(s, "     {why}");
                }
            }
        }
    }
    if !plan.notes.is_empty() {
        let _ = writeln!(s, "\nNotes");
        for note in &plan.notes {
            let _ = writeln!(s, "  - {note}");
        }
    }
    if plan.restart.is_some() || plan.afterwards.is_some() {
        let _ = writeln!(s, "\nAfterwards");
        if let Some(r) = &plan.restart {
            s.push_str(&restart_text(r));
        }
        if let Some(a) = &plan.afterwards {
            let _ = writeln!(s, "  {a}");
        }
    }
    if !plan.steps.is_empty() && plan.kind != PlanKind::Revert {
        if plan.steps.iter().any(|s| s.path().is_some()) {
            let _ = writeln!(
                s,
                "\nRatewise backs up every file first; `ratewise revert` undoes this."
            );
        } else {
            let _ = writeln!(s, "\n`ratewise revert` undoes this.");
        }
    }
    s
}

pub fn restart_text(r: &Restart) -> String {
    let mut s = String::new();
    match &r.manual {
        Some(m) => {
            let _ = writeln!(
                s,
                "  {} must restart for this to take effect. {m}",
                r.services
            );
        }
        None => {
            let _ = writeln!(
                s,
                "  Restart {} for this to take effect. Sound stops for a moment. Add --restart to \
                 do it now, or run:",
                r.services
            );
            for c in &r.commands {
                let _ = writeln!(s, "    {}", c.display());
            }
        }
    }
    s
}

fn diff(old: &str, new: &str) -> String {
    let d = TextDiff::from_lines(old, new);
    let mut out = String::new();
    for hunk in d.unified_diff().context_radius(2).iter_hunks() {
        for line in hunk.to_string().lines() {
            let _ = writeln!(out, "       {line}");
        }
    }
    out
}
