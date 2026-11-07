//! Device, capability and state model. Pure data, no I/O.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Bumped whenever the JSON shape of [`Snapshot`] changes incompatibly.
pub const SCHEMA_VERSION: u32 = 1;

/// Standard sample rates (spec §3.2), used for the capability strip and for expanding ranges.
pub const STANDARD_RATES: [u32; 15] = [
    8000, 11025, 16000, 22050, 32000, 44100, 48000, 88200, 96000, 176400, 192000, 352800, 384000,
    705600, 768000,
];

/// Where a probed fact came from, so the UI can show provenance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Source {
    PipeWire,
    Kernel,
    Procfs,
}

impl Source {
    /// Short label for display ("from /proc/asound").
    pub fn label(self) -> &'static str {
        match self {
            Source::PipeWire => "PipeWire",
            Source::Kernel => "kernel",
            Source::Procfs => "/proc/asound",
        }
    }
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// A value together with the source it was read from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sourced<T> {
    pub value: T,
    pub source: Source,
}

impl<T> Sourced<T> {
    pub fn new(value: T, source: Source) -> Self {
        Self { value, source }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Playback,
    Capture,
    Duplex,
}

impl Direction {
    pub fn short(self) -> &'static str {
        match self {
            Direction::Playback => "playback",
            Direction::Capture => "capture",
            Direction::Duplex => "duplex",
        }
    }
}

/// What kind of hardware this is, mainly for the sidebar icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeviceKind {
    Usb,
    #[serde(rename = "builtin")]
    BuiltIn,
    Hdmi,
    Bluetooth,
    Unknown,
}

impl DeviceKind {
    pub fn label(self) -> &'static str {
        match self {
            DeviceKind::Usb => "USB",
            DeviceKind::BuiltIn => "built-in",
            DeviceKind::Hdmi => "HDMI",
            DeviceKind::Bluetooth => "Bluetooth",
            DeviceKind::Unknown => "other",
        }
    }
}

/// A PCM sample format, normalised to ALSA naming (`S24_LE`, `S24_3LE`, ...).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub enum SampleFormat {
    U8,
    S8,
    S16Le,
    S16Be,
    /// 24-bit samples in 32-bit containers (PipeWire calls this `S24_32LE`).
    S24Le,
    S24Be,
    /// Packed 24-bit samples in 3 bytes (PipeWire calls this `S24LE`).
    S24_3Le,
    S24_3Be,
    S32Le,
    S32Be,
    FloatLe,
    FloatBe,
    Float64Le,
    Float64Be,
    /// Anything else, kept verbatim (e.g. `IEC958_SUBFRAME_LE`, `DSD_U32_BE`).
    Other(String),
}

impl SampleFormat {
    /// Parse an ALSA format name as printed by the kernel (`S32_LE`).
    pub fn from_alsa(name: &str) -> Self {
        let name = name.trim();
        match name.to_ascii_uppercase().as_str() {
            "U8" => Self::U8,
            "S8" => Self::S8,
            "S16_LE" => Self::S16Le,
            "S16_BE" => Self::S16Be,
            "S24_LE" => Self::S24Le,
            "S24_BE" => Self::S24Be,
            "S24_3LE" => Self::S24_3Le,
            "S24_3BE" => Self::S24_3Be,
            "S32_LE" => Self::S32Le,
            "S32_BE" => Self::S32Be,
            "FLOAT_LE" => Self::FloatLe,
            "FLOAT_BE" => Self::FloatBe,
            "FLOAT64_LE" => Self::Float64Le,
            "FLOAT64_BE" => Self::Float64Be,
            _ => Self::Other(name.to_string()),
        }
    }

    /// Parse a PipeWire/SPA format name (`S24_32LE`, `F32P`, ...).
    ///
    /// Planar variants map to the same sample format, and names without an
    /// endianness suffix are native-endian, which is little-endian on every
    /// target Ratewise supports.
    pub fn from_pipewire(name: &str) -> Self {
        let name = name.trim();
        let base = name
            .strip_suffix('P')
            .filter(|b| !b.is_empty())
            .unwrap_or(name);
        match base {
            "U8" => Self::U8,
            "S8" => Self::S8,
            "S16LE" | "S16" => Self::S16Le,
            "S16BE" => Self::S16Be,
            "S24_32LE" | "S24_32" => Self::S24Le,
            "S24_32BE" => Self::S24Be,
            "S24LE" | "S24" => Self::S24_3Le,
            "S24BE" => Self::S24_3Be,
            "S32LE" | "S32" => Self::S32Le,
            "S32BE" => Self::S32Be,
            "F32LE" | "F32" => Self::FloatLe,
            "F32BE" => Self::FloatBe,
            "F64LE" | "F64" => Self::Float64Le,
            "F64BE" => Self::Float64Be,
            _ => Self::Other(name.to_string()),
        }
    }

    /// The ALSA name, e.g. `S24_3LE`.
    pub fn alsa_name(&self) -> &str {
        match self {
            Self::U8 => "U8",
            Self::S8 => "S8",
            Self::S16Le => "S16_LE",
            Self::S16Be => "S16_BE",
            Self::S24Le => "S24_LE",
            Self::S24Be => "S24_BE",
            Self::S24_3Le => "S24_3LE",
            Self::S24_3Be => "S24_3BE",
            Self::S32Le => "S32_LE",
            Self::S32Be => "S32_BE",
            Self::FloatLe => "FLOAT_LE",
            Self::FloatBe => "FLOAT_BE",
            Self::Float64Le => "FLOAT64_LE",
            Self::Float64Be => "FLOAT64_BE",
            Self::Other(s) => s,
        }
    }

    /// Bit depth of the samples (not the container), if known.
    pub fn bits(&self) -> Option<u32> {
        match self {
            Self::U8 | Self::S8 => Some(8),
            Self::S16Le | Self::S16Be => Some(16),
            Self::S24Le | Self::S24Be | Self::S24_3Le | Self::S24_3Be => Some(24),
            Self::S32Le | Self::S32Be | Self::FloatLe | Self::FloatBe => Some(32),
            Self::Float64Le | Self::Float64Be => Some(64),
            Self::Other(_) => None,
        }
    }

    pub fn is_float(&self) -> bool {
        matches!(
            self,
            Self::FloatLe | Self::FloatBe | Self::Float64Le | Self::Float64Be
        )
    }

    /// "24-bit", "32-bit float", or the raw name when unknown.
    pub fn depth_label(&self) -> String {
        match (self.bits(), self.is_float()) {
            (Some(b), false) => format!("{b}-bit"),
            (Some(b), true) => format!("{b}-bit float"),
            (None, _) => self.alsa_name().to_string(),
        }
    }
}

impl fmt::Display for SampleFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.alsa_name())
    }
}

impl From<String> for SampleFormat {
    fn from(s: String) -> Self {
        Self::from_alsa(&s)
    }
}

impl From<SampleFormat> for String {
    fn from(f: SampleFormat) -> Self {
        f.alsa_name().to_string()
    }
}

/// Sample rates a device accepts: either an explicit list or a continuous range.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum RateSet {
    Discrete { rates: Vec<u32> },
    Range { min: u32, max: u32 },
}

impl RateSet {
    pub fn discrete(rates: impl IntoIterator<Item = u32>) -> Self {
        let mut rates: Vec<u32> = rates.into_iter().filter(|&r| r > 0).collect();
        rates.sort_unstable();
        rates.dedup();
        Self::Discrete { rates }
    }

    pub fn range(a: u32, b: u32) -> Self {
        Self::Range {
            min: a.min(b),
            max: a.max(b),
        }
    }

    pub fn contains(&self, rate: u32) -> bool {
        match self {
            Self::Discrete { rates } => rates.binary_search(&rate).is_ok(),
            Self::Range { min, max } => (*min..=*max).contains(&rate),
        }
    }

    /// The standard rates (spec §3.2) this set accepts.
    pub fn standard_rates(&self) -> Vec<u32> {
        STANDARD_RATES
            .iter()
            .copied()
            .filter(|&r| self.contains(r))
            .collect()
    }

    /// PipeWire reports huge ranges (often 1..i32::MAX) for nodes that will
    /// resample anything. That says nothing about the hardware.
    pub fn is_unbounded(&self) -> bool {
        matches!(self, Self::Range { max, .. } if *max >= 10_000_000)
    }

    /// Everything either set accepts. Disjoint ranges collapse to their standard rates.
    pub fn union(self, other: RateSet) -> RateSet {
        match (self, other) {
            (Self::Discrete { rates: a }, Self::Discrete { rates: b }) => {
                Self::discrete(a.into_iter().chain(b))
            }
            (Self::Range { min: a0, max: a1 }, Self::Range { min: b0, max: b1 }) => {
                if a0 <= b1.saturating_add(1) && b0 <= a1.saturating_add(1) {
                    Self::range(a0.min(b0), a1.max(b1))
                } else {
                    let a = Self::range(a0, a1).standard_rates();
                    let b = Self::range(b0, b1).standard_rates();
                    Self::discrete(a.into_iter().chain(b))
                }
            }
            (r @ Self::Range { .. }, Self::Discrete { rates })
            | (Self::Discrete { rates }, r @ Self::Range { .. }) => {
                if rates.iter().all(|&x| r.contains(x)) {
                    r
                } else {
                    Self::discrete(r.standard_rates().into_iter().chain(rates))
                }
            }
        }
    }
}

/// What a device can do. Each field records which source it came from.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    pub rates: Option<Sourced<RateSet>>,
    pub formats: Option<Sourced<Vec<SampleFormat>>>,
    pub channels: Option<Sourced<Vec<u32>>>,
    /// Period size the driver accepts, in frames (kernel probe only).
    pub period_frames: Option<Sourced<FrameRange>>,
    /// Buffer size the driver accepts, in frames (kernel probe only).
    pub buffer_frames: Option<Sourced<FrameRange>>,
}

/// An inclusive range of frame counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameRange {
    pub min: u32,
    pub max: u32,
}

/// How the direct kernel probe went for a device (spec §3.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum KernelProbeStatus {
    Ok,
    /// Another program (usually the sound server) has the device open. This is normal.
    Busy,
    /// No permission to open `/dev/snd/*` (not in the `audio` group?).
    PermissionDenied,
    Failed {
        message: String,
    },
}

/// Parameters of an open stream on the hardware.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamParams {
    pub rate: u32,
    pub format: Option<SampleFormat>,
    pub channels: Option<u32>,
    pub period_frames: Option<u32>,
    pub buffer_frames: Option<u32>,
}

impl StreamParams {
    pub fn period_ms(&self) -> Option<f64> {
        frames_to_ms(self.period_frames?, self.rate)
    }

    pub fn buffer_ms(&self) -> Option<f64> {
        frames_to_ms(self.buffer_frames?, self.rate)
    }
}

/// What the device is doing right now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum Running {
    /// Not open, or suspended by the sound server.
    Idle,
    Active(StreamParams),
}

/// ALSA address of a PCM device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlsaPcm {
    pub card: u32,
    pub card_id: Option<String>,
    pub device: u32,
    pub card_driver: Option<String>,
    pub pcm_name: Option<String>,
}

impl AlsaPcm {
    /// `hw:CARD=PCH,DEV=0`, or `hw:0,0` when the card id is unknown.
    pub fn hw_id(&self) -> String {
        match &self.card_id {
            Some(id) => format!("hw:CARD={id},DEV={}", self.device),
            None => format!("hw:{},{}", self.card, self.device),
        }
    }
}

/// The PipeWire node that represents a device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PipeWireNode {
    pub object_id: u32,
    pub node_name: String,
    pub media_class: String,
    pub state: Option<String>,
    pub is_default: bool,
}

/// One playback or capture endpoint, merged from every source that knows about it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Device {
    /// Stable selector: the PipeWire `node.name`, or `alsa:<card id>:<device>:<direction>`.
    pub id: String,
    pub description: String,
    pub direction: Direction,
    pub kind: DeviceKind,
    pub alsa: Option<AlsaPcm>,
    pub pipewire: Option<PipeWireNode>,
    pub capabilities: Capabilities,
    pub running: Option<Sourced<Running>>,
    /// `None` when the kernel wasn't probed (no `/dev/snd`, Bluetooth, fixture without data).
    pub kernel_probe: Option<KernelProbeStatus>,
}

/// PipeWire's global clock settings (the `settings` metadata).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClockSettings {
    pub rate: Option<u32>,
    pub allowed_rates: Option<Vec<u32>>,
    pub quantum: Option<u32>,
    pub min_quantum: Option<u32>,
    pub max_quantum: Option<u32>,
    pub force_rate: Option<u32>,
    pub force_quantum: Option<u32>,
}

/// Something a probe couldn't do, in plain English.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeNote {
    pub source: Source,
    pub message: String,
}

/// Everything Ratewise knows about the system at one moment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub schema_version: u32,
    pub devices: Vec<Device>,
    pub clock: Option<Sourced<ClockSettings>>,
    pub pipewire_version: Option<String>,
    pub stack: StackReport,
    pub notes: Vec<ProbeNote>,
}

/// Which sound server owns audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SoundServer {
    PipeWire,
    PulseAudio,
    /// A standalone JACK server (Ratewise shows it read-only).
    Jack,
    /// No sound server: programs use ALSA devices directly.
    Alsa,
    /// No sound server and no sound cards.
    None,
}

/// WirePlumber's config format changed completely between 0.4 (Lua) and 0.5 (SPA-JSON).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WirePlumberConfig {
    #[serde(rename = "lua-0.4")]
    Lua04,
    #[serde(rename = "spa-json-0.5")]
    SpaJson05,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "name", rename_all = "kebab-case")]
pub enum SessionManager {
    #[serde(rename = "wireplumber")]
    WirePlumber {
        version: Option<String>,
        config: Option<WirePlumberConfig>,
    },
    PipewireMediaSession,
}

/// What's running and which versions (spec §3.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StackReport {
    pub server: SoundServer,
    pub pipewire_version: Option<String>,
    /// PipeWire's PulseAudio replacement is running.
    pub pipewire_pulse: bool,
    pub session_manager: Option<SessionManager>,
    /// Only for a real PulseAudio daemon (not pipewire-pulse).
    pub pulseaudio_version: Option<String>,
    pub jack_running: bool,
    /// Whether `systemctl --user` can restart services. `None` if unknown.
    pub systemd_user_session: Option<bool>,
}

impl StackReport {
    /// One line for the sidebar pill and the CLI, e.g. "PipeWire 1.6.10 · WirePlumber 0.5.18".
    pub fn summary(&self) -> String {
        let with_version = |name: &str, v: &Option<String>| match v {
            Some(v) => format!("{name} {v}"),
            None => name.to_string(),
        };
        let mut parts = vec![match self.server {
            SoundServer::PipeWire => with_version("PipeWire", &self.pipewire_version),
            SoundServer::PulseAudio => with_version("PulseAudio", &self.pulseaudio_version),
            SoundServer::Jack => "JACK".into(),
            SoundServer::Alsa => "ALSA only (no sound server)".into(),
            SoundServer::None => "No sound server".into(),
        }];
        if self.server == SoundServer::PipeWire {
            match &self.session_manager {
                Some(SessionManager::WirePlumber { version, .. }) => {
                    parts.push(with_version("WirePlumber", version))
                }
                Some(SessionManager::PipewireMediaSession) => {
                    parts.push("pipewire-media-session".into())
                }
                None => parts.push("no session manager".into()),
            }
            if self.pipewire_pulse {
                parts.push("pipewire-pulse".into());
            }
        }
        if self.jack_running && self.server != SoundServer::Jack {
            parts.push("JACK".into());
        }
        parts.join(" · ")
    }
}

/// Shown when no devices were found at all (spec §5).
pub const NO_DEVICES_MESSAGE: &str = "No sound devices found. This usually means you don't have access to /dev/snd. Are you in the `audio` group, or running inside a container?";

/// `44100` → `"44.1"`, `48000` → `"48"`, `11025` → `"11.025"`.
pub fn khz(rate: u32) -> String {
    let whole = rate / 1000;
    let frac = rate % 1000;
    if frac == 0 {
        whole.to_string()
    } else {
        let frac = format!("{frac:03}");
        format!("{whole}.{}", frac.trim_end_matches('0'))
    }
}

fn frames_to_ms(frames: u32, rate: u32) -> Option<f64> {
    (rate > 0).then(|| f64::from(frames) * 1000.0 / f64::from(rate))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sourced_serializes_with_provenance() {
        let s = Sourced::new(48_000u32, Source::Procfs);
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(json, r#"{"value":48000,"source":"Procfs"}"#);
    }

    #[test]
    fn pipewire_and_alsa_names_agree() {
        let pairs = [
            ("S16LE", "S16_LE"),
            ("S24_32LE", "S24_LE"),
            ("S24LE", "S24_3LE"),
            ("S32LE", "S32_LE"),
            ("F32LE", "FLOAT_LE"),
            ("F32P", "FLOAT_LE"),
            ("S24_32P", "S24_LE"),
            ("U8P", "U8"),
        ];
        for (pw, alsa) in pairs {
            assert_eq!(
                SampleFormat::from_pipewire(pw),
                SampleFormat::from_alsa(alsa),
                "{pw}"
            );
        }
        assert_eq!(
            SampleFormat::from_pipewire("DSD_U32BE"),
            SampleFormat::Other("DSD_U32BE".into())
        );
    }

    #[test]
    fn depth_labels() {
        assert_eq!(SampleFormat::S24_3Le.depth_label(), "24-bit");
        assert_eq!(SampleFormat::FloatLe.depth_label(), "32-bit float");
        assert_eq!(SampleFormat::Other("X".into()).depth_label(), "X");
    }

    #[test]
    fn khz_formatting() {
        assert_eq!(khz(44_100), "44.1");
        assert_eq!(khz(48_000), "48");
        assert_eq!(khz(11_025), "11.025");
        assert_eq!(khz(352_800), "352.8");
    }

    #[test]
    fn rate_set_union() {
        let a = RateSet::discrete([48_000, 44_100]);
        let b = RateSet::discrete([96_000, 48_000]);
        assert_eq!(a.union(b), RateSet::discrete([44_100, 48_000, 96_000]));

        let r = RateSet::range(8_000, 48_000);
        assert_eq!(r.clone().union(RateSet::discrete([44_100])), r);
        assert_eq!(
            r.union(RateSet::discrete([96_000])).standard_rates(),
            vec![8000, 11025, 16000, 22050, 32000, 44100, 48000, 96000]
        );

        let disjoint = RateSet::range(8_000, 16_000).union(RateSet::range(44_100, 48_000));
        assert_eq!(
            disjoint,
            RateSet::discrete([8000, 11025, 16000, 44100, 48000])
        );
    }

    #[test]
    fn unbounded_ranges() {
        assert!(RateSet::range(1, i32::MAX as u32).is_unbounded());
        assert!(!RateSet::range(44_100, 768_000).is_unbounded());
    }

    #[test]
    fn latency() {
        let p = StreamParams {
            rate: 48_000,
            format: None,
            channels: None,
            period_frames: Some(1024),
            buffer_frames: Some(4096),
        };
        assert!((p.period_ms().unwrap() - 21.333).abs() < 0.01);
        assert!((p.buffer_ms().unwrap() - 85.333).abs() < 0.01);
    }
}
