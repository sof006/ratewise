//! What a user can change (spec §4), and Ratewise's record of what it has saved.
//!
//! The record (`saved.json`) is the source of truth for every file Ratewise
//! writes: each `set --save` updates the record and regenerates the files from it.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::core::model::{Device, Direction, SampleFormat, khz};

pub const SAVED_VERSION: u32 = 1;

/// A new value, or back to the default (removes the setting).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value<T> {
    Set(T),
    Default,
}

impl<T> Value<T> {
    pub fn into_option(self) -> Option<T> {
        match self {
            Self::Set(v) => Some(v),
            Self::Default => None,
        }
    }
    pub fn as_option(&self) -> Option<&T> {
        match self {
            Self::Set(v) => Some(v),
            Self::Default => None,
        }
    }
}

/// One requested change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// Global default rate, or a device's rate with `--device`.
    Rate(Value<u32>),
    AllowedRates(Value<Vec<u32>>),
    Quantum(Value<u32>),
    MinQuantum(Value<u32>),
    MaxQuantum(Value<u32>),
    /// Live only. 0 releases it.
    ForceRate(u32),
    /// Live only. 0 releases it.
    ForceQuantum(u32),
    /// Per device on PipeWire; global on PulseAudio and ALSA.
    Format(Value<SampleFormat>),
    PeriodSize(Value<u32>),
    Headroom(Value<u32>),
    /// `Set(false)` keeps the device open when idle.
    Suspend(Value<bool>),
    ResampleQuality(Value<u8>),
    /// A Bluetooth card's profile, which picks both the mode and the codec. The text is
    /// what the user asked for: a profile name (`a2dp-sink-ldac`), a mode (`headset`) or
    /// a codec (`ldac`); [`crate::core::bluetooth::resolve`] finds the profile.
    BluetoothProfile(String),
    /// Whether WirePlumber switches Bluetooth headsets to headset mode when a program
    /// opens the microphone. Global.
    HeadsetAutoswitch(Value<bool>),
    /// LDAC's bitrate on one Bluetooth device.
    LdacQuality(Value<LdacQuality>),
    /// Whether LE Audio (LC3) on one Bluetooth device favours low latency or a reliable
    /// connection. `Default` is PipeWire's balanced choice.
    LeAudioLatency(Value<LeAudioLatency>),
    /// One of the LE Audio link settings PipeWire lets you set by hand.
    LeAudioQos(LeAudioQos),
}

/// A manual LE Audio (LC3) link setting on one Bluetooth device. `Default` leaves it to
/// PipeWire. All are `bluez5.bap.*` device properties, new in PipeWire 1.6.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeAudioQos {
    /// `bluez5.bap.preset`: a name from [`crate::core::lc3::PRESETS`].
    Preset(Value<String>),
    /// `bluez5.bap.rtn`.
    Retransmissions(Value<u8>),
    /// `bluez5.bap.latency`: the maximum transport latency, in ms.
    TransportLatency(Value<u16>),
    /// `bluez5.bap.delay`: the presentation delay, in µs.
    PresentationDelay(Value<u32>),
    /// `bluez5.bap.framing`: `true` is framed.
    Framing(Value<bool>),
}

impl LeAudioQos {
    /// The CLI names, in display order.
    pub const NAMES: [&'static str; 5] = [
        "le-audio-preset",
        "le-audio-retransmissions",
        "le-audio-transport-latency",
        "le-audio-presentation-delay",
        "le-audio-framing",
    ];

    pub fn name(&self) -> &'static str {
        Self::NAMES[match self {
            Self::Preset(_) => 0,
            Self::Retransmissions(_) => 1,
            Self::TransportLatency(_) => 2,
            Self::PresentationDelay(_) => 3,
            Self::Framing(_) => 4,
        }]
    }

    /// "LC3 preset", "maximum transport latency", ...
    pub fn label(&self) -> &'static str {
        match self {
            Self::Preset(_) => "LC3 preset",
            Self::Retransmissions(_) => "retransmissions",
            Self::TransportLatency(_) => "maximum transport latency",
            Self::PresentationDelay(_) => "presentation delay",
            Self::Framing(_) => "framing",
        }
    }

    pub fn is_default(&self) -> bool {
        self.text().is_none()
    }

    /// The value in words: "48_5_1", "5", "20 ms", "40 ms", "framed". None for default.
    pub fn text(&self) -> Option<String> {
        use crate::core::lc3::ms_text;
        match self {
            Self::Preset(v) => v.as_option().cloned(),
            Self::Retransmissions(v) => v.as_option().map(|n| n.to_string()),
            Self::TransportLatency(v) => v.as_option().map(|ms| format!("{ms} ms")),
            Self::PresentationDelay(v) => v.as_option().map(|us| ms_text(*us)),
            Self::Framing(v) => v
                .as_option()
                .map(|f| if *f { "framed" } else { "unframed" }.to_string()),
        }
    }

    /// What a text field shows for it: the number alone ("40", "7.5"), or "" for default.
    pub fn input(&self) -> String {
        match self {
            Self::TransportLatency(Value::Set(ms)) => ms.to_string(),
            Self::PresentationDelay(Value::Set(us)) => crate::core::lc3::ms_number(*us),
            other => other.text().unwrap_or_default(),
        }
    }

    /// The setting called `name` (one of [`Self::NAMES`]) at `value`. `auto` and
    /// `default` go back to PipeWire's choice. Latency and delay are in ms.
    pub fn parse(name: &str, value: &str) -> Result<Self, String> {
        let v = value.trim();
        let auto = matches!(
            v.to_ascii_lowercase().as_str(),
            "" | "auto" | "default" | "automatic" | "unset" | "none"
        );
        let num = |unit: &str| -> String {
            let t = v.to_ascii_lowercase();
            t.strip_suffix(unit).unwrap_or(&t).trim().to_string()
        };
        let bad = |hint: &str| format!("\"{v}\": {hint}, or `auto`");
        Ok(match name {
            "le-audio-preset" => Self::Preset(if auto {
                Value::Default
            } else {
                let p = crate::core::lc3::find(v)
                    .ok_or_else(|| bad("use an LC3 preset name such as `48_5_1`"))?;
                Value::Set(p.name.to_string())
            }),
            "le-audio-retransmissions" => Self::Retransmissions(if auto {
                Value::Default
            } else {
                Value::Set(
                    num("")
                        .parse()
                        .map_err(|_| bad("use a number from 0 to 255"))?,
                )
            }),
            "le-audio-transport-latency" => Self::TransportLatency(if auto {
                Value::Default
            } else {
                Value::Set(
                    num("ms")
                        .parse()
                        .map_err(|_| bad("use a whole number of milliseconds"))?,
                )
            }),
            "le-audio-presentation-delay" => Self::PresentationDelay(if auto {
                Value::Default
            } else {
                Value::Set(
                    crate::core::lc3::parse_delay(v)
                        .ok_or_else(|| bad("use milliseconds, such as `40` or `7.5`"))?,
                )
            }),
            "le-audio-framing" => Self::Framing(if auto {
                Value::Default
            } else {
                Value::Set(match v.to_ascii_lowercase().as_str() {
                    "framed" | "true" | "yes" | "on" | "1" => true,
                    "unframed" | "false" | "no" | "off" | "0" => false,
                    _ => return Err(bad("use `framed` or `unframed`")),
                })
            }),
            other => return Err(format!("\"{other}\" isn't an LE Audio setting")),
        })
    }
}

/// `bluez5.bap.force-target-latency`: which of the LE Audio (BAP) QoS tables PipeWire
/// picks link settings from. Unset, it uses both ("balanced"), and since 1.6 prefers the
/// high-reliability ones at 44.1 and 48 kHz.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LeAudioLatency {
    /// BAP's low-latency settings: about 10-30 ms on the link, fewer retries.
    Low,
    /// BAP's high-reliability settings: about 75-100 ms on the link, more retries.
    Reliable,
}

impl LeAudioLatency {
    pub const ALL: [LeAudioLatency; 2] = [Self::Low, Self::Reliable];

    /// The value PipeWire takes.
    pub fn key(self) -> &'static str {
        match self {
            Self::Low => "low-latency",
            Self::Reliable => "high-reliability",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Low => "low latency",
            Self::Reliable => "high reliability",
        }
    }

    /// `low`, `low-latency`, `gaming`, `reliable`, ...
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "low" | "low-latency" | "latency" | "fast" | "game" | "gaming" => Self::Low,
            "reliable" | "high-reliability" | "reliability" | "stable" => Self::Reliable,
            _ => return None,
        })
    }
}

/// `bluez5.a2dp.ldac.quality`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LdacQuality {
    /// Adapts to the connection (PipeWire's default).
    Auto,
    /// 990 kbps.
    Hq,
    /// 660 kbps.
    Sq,
    /// 330 kbps.
    Mq,
}

impl LdacQuality {
    pub const ALL: [LdacQuality; 4] = [Self::Auto, Self::Hq, Self::Sq, Self::Mq];

    /// The value PipeWire takes.
    pub fn key(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Hq => "hq",
            Self::Sq => "sq",
            Self::Mq => "mq",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "adaptive",
            Self::Hq => "best quality (990 kbps)",
            Self::Sq => "standard (660 kbps)",
            Self::Mq => "most stable connection (330 kbps)",
        }
    }

    /// `hq`, `best`, `990`, ...
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "auto" | "adaptive" | "abr" => Self::Auto,
            "hq" | "high" | "best" | "990" => Self::Hq,
            "sq" | "standard" | "normal" | "660" => Self::Sq,
            "mq" | "low" | "stable" | "connection" | "330" => Self::Mq,
            _ => return None,
        })
    }
}

impl Change {
    /// The same change in canonical form: allowed rates sorted, without repeats.
    pub fn normalised(mut self) -> Self {
        if let Self::AllowedRates(Value::Set(rs)) = &mut self {
            rs.sort_unstable();
            rs.dedup();
        }
        self
    }

    /// The CLI name, e.g. `allowed-rates`.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Rate(_) => "rate",
            Self::AllowedRates(_) => "allowed-rates",
            Self::Quantum(_) => "quantum",
            Self::MinQuantum(_) => "min-quantum",
            Self::MaxQuantum(_) => "max-quantum",
            Self::ForceRate(_) => "force-rate",
            Self::ForceQuantum(_) => "force-quantum",
            Self::Format(_) => "format",
            Self::PeriodSize(_) => "period-size",
            Self::Headroom(_) => "headroom",
            Self::Suspend(_) => "suspend",
            Self::ResampleQuality(_) => "resample-quality",
            Self::BluetoothProfile(_) => "profile",
            Self::HeadsetAutoswitch(_) => "headset-autoswitch",
            Self::LdacQuality(_) => "ldac-quality",
            Self::LeAudioLatency(_) => "le-audio-latency",
            Self::LeAudioQos(q) => q.name(),
        }
    }

    /// Settings that only make sense for one device.
    pub fn is_device_only(&self) -> bool {
        matches!(
            self,
            Self::PeriodSize(_)
                | Self::Headroom(_)
                | Self::Suspend(_)
                | Self::BluetoothProfile(_)
                | Self::LdacQuality(_)
                | Self::LeAudioLatency(_)
                | Self::LeAudioQos(_)
        )
    }

    /// Settings that can't target a device.
    pub fn is_global_only(&self) -> bool {
        matches!(
            self,
            Self::AllowedRates(_)
                | Self::Quantum(_)
                | Self::MinQuantum(_)
                | Self::MaxQuantum(_)
                | Self::ForceRate(_)
                | Self::ForceQuantum(_)
                | Self::ResampleQuality(_)
                | Self::HeadsetAutoswitch(_)
        )
    }

    /// Plain English, e.g. "the default sample rate to 96 kHz".
    pub fn describe(&self, device: Option<&Device>) -> String {
        let of = |what: &str| match device {
            Some(d) => format!("{what} of {}", d.description),
            None => what.to_string(),
        };
        let to = |what: String, v: Option<String>| match v {
            Some(v) => format!("set {what} to {v}"),
            None => format!("set {what} back to the default"),
        };
        let frames = |v: &Value<u32>| v.as_option().map(|f| format!("{f} frames"));
        match self {
            Self::Rate(v) => to(
                match device {
                    Some(_) => of("the sample rate"),
                    None => "the default sample rate".into(),
                },
                v.as_option().map(|r| format!("{} kHz", khz(*r))),
            ),
            Self::AllowedRates(v) => to(
                "the allowed sample rates".into(),
                v.as_option().map(|r| format!("{} kHz", rate_list(r))),
            ),
            Self::Quantum(v) => to("the default quantum (buffer size)".into(), frames(v)),
            Self::MinQuantum(v) => to("the minimum quantum".into(), frames(v)),
            Self::MaxQuantum(v) => to("the maximum quantum".into(), frames(v)),
            Self::ForceRate(0) => "release the forced sample rate".into(),
            Self::ForceRate(r) => format!("force the sample rate to {} kHz", khz(*r)),
            Self::ForceQuantum(0) => "release the forced quantum".into(),
            Self::ForceQuantum(q) => format!("force the quantum to {q} frames"),
            Self::Format(v) => to(
                match device {
                    Some(_) => of("the sample format"),
                    None => "the default sample format".into(),
                },
                v.as_option().map(|f| match f.bits() {
                    Some(_) => format!("{f} ({})", f.depth_label()),
                    None => f.to_string(),
                }),
            ),
            Self::PeriodSize(v) => to(of("the period size"), frames(v)),
            Self::Headroom(v) => to(of("the headroom"), frames(v)),
            Self::Suspend(v) => {
                let name = device.map_or("the device", |d| d.description.as_str());
                if matches!(v, Value::Set(false)) {
                    format!("keep {name} open when idle (no suspend)")
                } else {
                    format!("let {name} suspend when idle again")
                }
            }
            Self::ResampleQuality(v) => to(
                "the resampler quality".into(),
                v.as_option().map(|q| q.to_string()),
            ),
            Self::BluetoothProfile(want) => {
                let name = device.map_or("the device", |d| d.description.as_str());
                let profile = device
                    .and_then(|d| d.bluetooth.as_ref())
                    .and_then(|bt| crate::core::bluetooth::resolve(bt, want).ok());
                match profile {
                    Some(p) => format!(
                        "switch {name} to {}",
                        crate::core::bluetooth::profile_label(p)
                    ),
                    None => format!("switch {name} to the {want} profile"),
                }
            }
            Self::HeadsetAutoswitch(Value::Set(true)) => {
                "switch Bluetooth headsets to headset mode when a program uses the microphone"
                    .into()
            }
            Self::HeadsetAutoswitch(Value::Set(false)) => {
                "keep Bluetooth headsets in their mode when a program uses the microphone".into()
            }
            Self::HeadsetAutoswitch(Value::Default) => {
                "set Bluetooth headset switching back to the default".into()
            }
            Self::LdacQuality(v) => to(
                of("the LDAC quality"),
                v.as_option().map(|q| q.label().to_string()),
            ),
            Self::LeAudioLatency(v) => to(
                of("the LE Audio latency"),
                Some(v.as_option().map_or("balanced", |l| l.label()).to_string()),
            ),
            Self::LeAudioQos(q) => {
                let what = of(&format!("the LE Audio {}", q.label()));
                match q.text() {
                    Some(v) => format!("set {what} to {v}"),
                    None => format!("set {what} back to automatic"),
                }
            }
        }
    }
}

pub fn rate_list(rates: &[u32]) -> String {
    rates.iter().map(|&r| khz(r)).collect::<Vec<_>>().join(", ")
}

/// Live: this session only, through `pw-metadata`. Save: written to config files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Live,
    Save,
}

impl fmt::Display for Mode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Live => "live",
            Self::Save => "saved",
        })
    }
}

/// Everything Ratewise has saved. Serialized as `saved.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedConfig {
    pub version: u32,
    pub global: GlobalSaved,
    /// Keyed by device id (the PipeWire `node.name`, or `alsa:<card>:<dev>:<dir>`).
    pub devices: BTreeMap<String, DeviceSaved>,
    /// Bluetooth settings, keyed by the card's `device.name` (`bluez_card.*`): a card's
    /// nodes come and go with its profile, but the card's name stays.
    pub bluetooth: BTreeMap<String, BluetoothSaved>,
    /// Set when Ratewise created `~/.config/pulse/daemon.conf`: a hash of what it
    /// wrote, so `reset` removes the file only if nobody has edited it since.
    pub created_pulse_daemon_conf: Option<String>,
}

impl Default for SavedConfig {
    fn default() -> Self {
        Self {
            version: SAVED_VERSION,
            global: GlobalSaved::default(),
            devices: BTreeMap::new(),
            bluetooth: BTreeMap::new(),
            created_pulse_daemon_conf: None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GlobalSaved {
    pub rate: Option<u32>,
    pub allowed_rates: Option<Vec<u32>>,
    pub quantum: Option<u32>,
    pub min_quantum: Option<u32>,
    pub max_quantum: Option<u32>,
    /// PulseAudio and ALSA only; PipeWire sets formats per device.
    pub format: Option<SampleFormat>,
    pub resample_quality: Option<u8>,
    /// WirePlumber's `bluetooth.autoswitch-to-headset-profile`.
    pub headset_autoswitch: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BluetoothSaved {
    pub description: String,
    #[serde(default)]
    pub ldac_quality: Option<LdacQuality>,
    #[serde(default)]
    pub le_audio_latency: Option<LeAudioLatency>,
    #[serde(default, skip_serializing_if = "LeAudioQosSaved::is_empty")]
    pub le_audio_qos: LeAudioQosSaved,
}

impl BluetoothSaved {
    pub fn is_empty(&self) -> bool {
        self.ldac_quality.is_none()
            && self.le_audio_latency.is_none()
            && self.le_audio_qos.is_empty()
    }
}

/// The manual LE Audio link settings saved for one device. Unset fields are left out of
/// `saved.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LeAudioQosSaved {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retransmissions: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transport_latency_ms: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presentation_delay_us: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub framed: Option<bool>,
}

impl LeAudioQosSaved {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    pub fn set(&mut self, q: &LeAudioQos) {
        match q {
            LeAudioQos::Preset(v) => self.preset = v.clone().into_option(),
            LeAudioQos::Retransmissions(v) => self.retransmissions = v.clone().into_option(),
            LeAudioQos::TransportLatency(v) => self.transport_latency_ms = v.clone().into_option(),
            LeAudioQos::PresentationDelay(v) => {
                self.presentation_delay_us = v.clone().into_option()
            }
            LeAudioQos::Framing(v) => self.framed = v.clone().into_option(),
        }
    }

    /// The saved value of the setting called `name`, as a change to it.
    pub fn get(&self, name: &str) -> Option<LeAudioQos> {
        fn v<T>(o: Option<T>) -> Value<T> {
            o.map_or(Value::Default, Value::Set)
        }
        Some(match name {
            "le-audio-preset" => LeAudioQos::Preset(v(self.preset.clone())),
            "le-audio-retransmissions" => LeAudioQos::Retransmissions(v(self.retransmissions)),
            "le-audio-transport-latency" => {
                LeAudioQos::TransportLatency(v(self.transport_latency_ms))
            }
            "le-audio-presentation-delay" => {
                LeAudioQos::PresentationDelay(v(self.presentation_delay_us))
            }
            "le-audio-framing" => LeAudioQos::Framing(v(self.framed)),
            _ => return None,
        })
    }

    /// Every setting, set or not, in display order.
    pub fn all(&self) -> Vec<LeAudioQos> {
        LeAudioQos::NAMES
            .iter()
            .filter_map(|n| self.get(n))
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceSaved {
    pub description: String,
    pub direction: Direction,
    /// Matched by WirePlumber rules.
    #[serde(default)]
    pub pipewire_node: Option<String>,
    /// Used for ALSA's per-card period size.
    #[serde(default)]
    pub alsa_card_id: Option<String>,
    #[serde(default)]
    pub format: Option<SampleFormat>,
    #[serde(default)]
    pub rate: Option<u32>,
    #[serde(default)]
    pub period_size: Option<u32>,
    #[serde(default)]
    pub headroom: Option<u32>,
    #[serde(default)]
    pub disable_suspend: bool,
}

impl DeviceSaved {
    pub fn new(d: &Device) -> Self {
        Self {
            description: d.description.clone(),
            direction: d.direction,
            pipewire_node: d.pipewire.as_ref().map(|p| p.node_name.clone()),
            alsa_card_id: d.alsa.as_ref().and_then(|a| a.card_id.clone()),
            format: None,
            rate: None,
            period_size: None,
            headroom: None,
            disable_suspend: false,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.format.is_none()
            && self.rate.is_none()
            && self.period_size.is_none()
            && self.headroom.is_none()
            && !self.disable_suspend
    }
}

impl SavedConfig {
    pub fn is_empty(&self) -> bool {
        self.global == GlobalSaved::default()
            && self.devices.is_empty()
            && self.bluetooth.is_empty()
    }

    /// The record with `change` applied. Live-only changes leave it unchanged.
    pub fn with(&self, change: &Change, device: Option<&Device>) -> SavedConfig {
        let mut next = self.clone();
        next.version = SAVED_VERSION;
        let g = &mut next.global;
        match (change, device) {
            (Change::Rate(v), None) => g.rate = v.clone().into_option(),
            (Change::AllowedRates(v), _) => g.allowed_rates = v.clone().into_option(),
            (Change::Quantum(v), _) => g.quantum = v.clone().into_option(),
            (Change::MinQuantum(v), _) => g.min_quantum = v.clone().into_option(),
            (Change::MaxQuantum(v), _) => g.max_quantum = v.clone().into_option(),
            (Change::Format(v), None) => g.format = v.clone().into_option(),
            (Change::ResampleQuality(v), _) => g.resample_quality = v.clone().into_option(),
            (Change::HeadsetAutoswitch(v), _) => g.headset_autoswitch = v.clone().into_option(),
            (
                Change::LdacQuality(_) | Change::LeAudioLatency(_) | Change::LeAudioQos(_),
                Some(d),
            ) => {
                if let Some(bt) = &d.bluetooth {
                    let entry = next
                        .bluetooth
                        .entry(bt.card_name.clone())
                        .or_insert_with(|| BluetoothSaved {
                            description: d.description.clone(),
                            ldac_quality: None,
                            le_audio_latency: None,
                            le_audio_qos: LeAudioQosSaved::default(),
                        });
                    entry.description = d.description.clone();
                    match change {
                        Change::LdacQuality(v) => entry.ldac_quality = v.clone().into_option(),
                        Change::LeAudioLatency(v) => {
                            entry.le_audio_latency = v.clone().into_option()
                        }
                        Change::LeAudioQos(q) => entry.le_audio_qos.set(q),
                        _ => {}
                    }
                    if entry.is_empty() {
                        next.bluetooth.remove(&bt.card_name);
                    }
                }
            }
            // WirePlumber remembers the profile itself.
            (Change::ForceRate(_) | Change::ForceQuantum(_) | Change::BluetoothProfile(_), _) => {}
            (_, Some(d)) => {
                let entry = next
                    .devices
                    .entry(d.id.clone())
                    .or_insert_with(|| DeviceSaved::new(d));
                // Keep the record's description and node name current.
                let fresh = DeviceSaved::new(d);
                entry.description = fresh.description;
                entry.pipewire_node = fresh.pipewire_node.or(entry.pipewire_node.take());
                entry.alsa_card_id = fresh.alsa_card_id.or(entry.alsa_card_id.take());
                match change {
                    Change::Rate(v) => entry.rate = v.clone().into_option(),
                    Change::Format(v) => entry.format = v.clone().into_option(),
                    Change::PeriodSize(v) => entry.period_size = v.clone().into_option(),
                    Change::Headroom(v) => entry.headroom = v.clone().into_option(),
                    Change::Suspend(v) => entry.disable_suspend = matches!(v, Value::Set(false)),
                    _ => {}
                }
                if entry.is_empty() {
                    next.devices.remove(&d.id);
                }
            }
            (
                Change::PeriodSize(_)
                | Change::Headroom(_)
                | Change::Suspend(_)
                | Change::LdacQuality(_)
                | Change::LeAudioLatency(_)
                | Change::LeAudioQos(_),
                None,
            ) => {}
        }
        next
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::{Capabilities, DeviceKind, PipeWireNode};

    pub(crate) fn dac() -> Device {
        Device {
            id: "alsa_output.usb-dac".into(),
            description: "USB DAC".into(),
            direction: Direction::Playback,
            kind: DeviceKind::Usb,
            alsa: None,
            pipewire: Some(PipeWireNode {
                object_id: 50,
                node_name: "alsa_output.usb-dac".into(),
                media_class: "Audio/Sink".into(),
                state: None,
                is_default: true,
            }),
            capabilities: Capabilities::default(),
            running: None,
            kernel_probe: None,
            bluetooth: None,
        }
    }

    #[test]
    fn device_entries_come_and_go() {
        let d = dac();
        let s = SavedConfig::default().with(&Change::Rate(Value::Set(96_000)), Some(&d));
        assert_eq!(s.devices[&d.id].rate, Some(96_000));
        assert_eq!(
            s.devices[&d.id].pipewire_node.as_deref(),
            Some("alsa_output.usb-dac")
        );
        assert_eq!(s.global.rate, None, "a device rate isn't the global rate");
        let s = s.with(&Change::Suspend(Value::Set(false)), Some(&d));
        assert!(s.devices[&d.id].disable_suspend);
        let s = s
            .with(&Change::Rate(Value::Default), Some(&d))
            .with(&Change::Suspend(Value::Default), Some(&d));
        assert!(s.devices.is_empty());
        assert!(s.is_empty());
    }

    #[test]
    fn live_only_changes_do_not_touch_the_record() {
        let s = SavedConfig::default();
        assert_eq!(s.with(&Change::ForceRate(96_000), None), s);
    }

    #[test]
    fn descriptions_read_well() {
        let d = dac();
        assert_eq!(
            Change::Rate(Value::Set(96_000)).describe(None),
            "set the default sample rate to 96 kHz"
        );
        assert_eq!(
            Change::Rate(Value::Set(44_100)).describe(Some(&d)),
            "set the sample rate of USB DAC to 44.1 kHz"
        );
        assert_eq!(
            Change::Quantum(Value::Default).describe(None),
            "set the default quantum (buffer size) back to the default"
        );
        assert_eq!(
            Change::Suspend(Value::Set(false)).describe(Some(&d)),
            "keep USB DAC open when idle (no suspend)"
        );
        assert_eq!(
            Change::Suspend(Value::Default).describe(Some(&d)),
            "let USB DAC suspend when idle again"
        );
        assert_eq!(
            Change::Format(Value::Set(SampleFormat::S24_3Le)).describe(Some(&d)),
            "set the sample format of USB DAC to S24_3LE (24-bit)"
        );
        assert_eq!(
            Change::ForceRate(0).describe(None),
            "release the forced sample rate"
        );
    }

    #[test]
    fn saved_json_round_trips_and_tolerates_missing_fields() {
        let d = dac();
        let s = SavedConfig::default()
            .with(
                &Change::AllowedRates(Value::Set(vec![44_100, 48_000])),
                None,
            )
            .with(&Change::Format(Value::Set(SampleFormat::S32Le)), Some(&d));
        let json = serde_json::to_string_pretty(&s).unwrap();
        assert_eq!(serde_json::from_str::<SavedConfig>(&json).unwrap(), s);
        let old: SavedConfig = serde_json::from_str(r#"{"global":{"rate":48000}}"#).unwrap();
        assert_eq!(old.global.rate, Some(48_000));
    }
}
