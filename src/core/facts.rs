//! Raw facts as each probe reports them, before merging. Pure data.
//!
//! Probes fill these in; [`crate::core::merge`] turns them into [`crate::core::model::Device`]s.

use crate::core::model::{ClockSettings, Direction, RateSet, Running, SampleFormat};

/// Parsed `/proc/asound`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProcAsound {
    pub cards: Vec<ProcCard>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcCard {
    pub number: u32,
    pub id: String,
    pub driver: String,
    pub name: String,
    pub long_name: String,
    pub pcms: Vec<ProcPcm>,
    pub usb_streams: Vec<UsbStream>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcPcm {
    pub device: u32,
    pub direction: Direction,
    pub id: String,
    pub name: String,
    /// From `sub*/hw_params`; `None` if it couldn't be read.
    pub running: Option<Running>,
}

/// One `/proc/asound/cardN/streamM` file (USB audio only).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UsbStream {
    /// `M` in `streamM`, which matches the PCM device number.
    pub device: u32,
    pub playback: Option<UsbStreamDirection>,
    pub capture: Option<UsbStreamDirection>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UsbStreamDirection {
    pub running: Option<bool>,
    pub momentary_freq: Option<u32>,
    pub altsets: Vec<UsbAltset>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UsbAltset {
    pub interface: u32,
    pub altset: u32,
    pub formats: Vec<SampleFormat>,
    pub channels: Option<u32>,
    pub rates: Option<RateSet>,
    pub bits: Option<u32>,
}

/// Parsed `pw-dump` output.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PwDump {
    pub core_version: Option<String>,
    pub nodes: Vec<PwNode>,
    pub clock: Option<ClockSettings>,
    pub default_sink: Option<String>,
    pub default_source: Option<String>,
}

/// An audio node, with missing props filled in from its parent device.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PwNode {
    pub id: u32,
    pub name: String,
    pub description: Option<String>,
    pub media_class: String,
    pub state: Option<String>,
    pub device_api: Option<String>,
    pub bus: Option<String>,
    pub alsa_card: Option<u32>,
    pub alsa_device: Option<u32>,
    pub alsa_path: Option<String>,
    pub profile_name: Option<String>,
    pub enum_formats: Vec<PwFormat>,
    pub current_format: Option<PwFormat>,
}

/// One `EnumFormat` or `Format` param.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PwFormat {
    pub formats: Vec<SampleFormat>,
    pub rates: Option<RateSet>,
    pub channels: Vec<u32>,
}
