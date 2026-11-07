//! Merges per-source facts into one device list, recording where each fact came from.
//!
//! Capabilities, per field: the kernel probe (the driver's own answer) over USB
//! stream files (exact per-altset data) over PipeWire's `EnumFormat`.
//! Running state: `hw_params` (what the hardware is really doing) over PipeWire's node state.

use std::collections::HashSet;

use crate::core::facts::{
    KernelOutcome, KernelPcmProbe, ProcAsound, ProcCard, ProcPcm, PwDump, PwFormat, PwNode,
};
use crate::core::model::{
    AlsaPcm, Capabilities, Device, DeviceKind, Direction, KernelProbeStatus, PipeWireNode,
    ProbeNote, RateSet, Running, SCHEMA_VERSION, Snapshot, Source, Sourced, StackReport,
    StreamParams,
};

/// Everything the probes found. Any source may be missing.
pub struct Facts<'a> {
    pub procfs: Option<&'a ProcAsound>,
    pub pipewire: Option<&'a PwDump>,
    pub kernel: Option<&'a [KernelPcmProbe]>,
    pub stack: StackReport,
}

pub fn merge(facts: Facts<'_>, notes: Vec<ProbeNote>) -> Snapshot {
    let Facts {
        procfs,
        pipewire: pw,
        kernel,
        stack,
    } = facts;
    let mut devices = Vec::new();
    let mut claimed: HashSet<(u32, u32, Direction)> = HashSet::new();
    if let Some(pw) = pw {
        for node in &pw.nodes {
            let Some(direction) = direction_from_media_class(&node.media_class) else {
                continue;
            };
            // Only hardware: skip virtual sinks (null-audio-sink, EasyEffects, ...).
            if !matches!(node.device_api.as_deref(), Some("alsa" | "bluez5")) {
                continue;
            }
            let pcm = procfs.and_then(|p| find_pcm(p, node, direction));
            if let Some((card, pcm)) = pcm {
                claimed.insert((card.number, pcm.device, pcm.direction));
            }
            devices.push(device_from_node(node, direction, pcm, pw));
        }
    }

    if let Some(procfs) = procfs {
        for card in &procfs.cards {
            for pcm in &card.pcms {
                if !claimed.contains(&(card.number, pcm.device, pcm.direction)) {
                    devices.push(device_from_pcm(card, pcm));
                }
            }
        }
    }

    if let Some(kernel) = kernel {
        for d in &mut devices {
            apply_kernel(d, kernel);
        }
    }

    devices.sort_by(|a, b| {
        let key = |d: &Device| {
            (
                d.direction,
                d.alsa
                    .as_ref()
                    .map_or((u32::MAX, u32::MAX), |a| (a.card, a.device)),
                d.description.clone(),
            )
        };
        key(a).cmp(&key(b))
    });

    Snapshot {
        schema_version: SCHEMA_VERSION,
        devices,
        clock: pw
            .and_then(|p| p.clock.clone())
            .map(|c| Sourced::new(c, Source::PipeWire)),
        pipewire_version: pw.and_then(|p| p.core_version.clone()),
        stack,
        notes,
    }
}

/// Overlays the kernel's answer for this device's PCM, and records how the probe went.
fn apply_kernel(d: &mut Device, probes: &[KernelPcmProbe]) {
    let Some(alsa) = &d.alsa else {
        return;
    };
    let want = match d.direction {
        Direction::Duplex => Direction::Playback,
        other => other,
    };
    let Some(probe) = probes
        .iter()
        .find(|p| p.card == alsa.card && p.device == alsa.device && p.direction == want)
    else {
        return;
    };
    d.kernel_probe = Some(match &probe.result {
        KernelOutcome::Ok(caps) => {
            let k = |v| Some(Sourced::new(v, Source::Kernel));
            let c = &mut d.capabilities;
            if !caps.formats.is_empty() {
                c.formats = k(caps.formats.clone());
            }
            if caps.rates != RateSet::discrete([]) {
                c.rates = Some(Sourced::new(caps.rates.clone(), Source::Kernel));
            }
            if !caps.channels.is_empty() {
                c.channels = Some(Sourced::new(caps.channels.clone(), Source::Kernel));
            }
            c.period_frames = caps.period_frames.map(|r| Sourced::new(r, Source::Kernel));
            c.buffer_frames = caps.buffer_frames.map(|r| Sourced::new(r, Source::Kernel));
            KernelProbeStatus::Ok
        }
        KernelOutcome::Busy => KernelProbeStatus::Busy,
        KernelOutcome::PermissionDenied => KernelProbeStatus::PermissionDenied,
        KernelOutcome::Failed { message } => KernelProbeStatus::Failed {
            message: message.clone(),
        },
    });
}

fn direction_from_media_class(class: &str) -> Option<Direction> {
    match class {
        "Audio/Sink" => Some(Direction::Playback),
        "Audio/Source" => Some(Direction::Capture),
        "Audio/Duplex" => Some(Direction::Duplex),
        _ => None,
    }
}

fn find_pcm<'a>(
    procfs: &'a ProcAsound,
    node: &PwNode,
    direction: Direction,
) -> Option<(&'a ProcCard, &'a ProcPcm)> {
    let card_no = node.alsa_card?;
    let device = node.alsa_device.unwrap_or(0);
    let want = match direction {
        Direction::Duplex => Direction::Playback,
        d => d,
    };
    let card = procfs.cards.iter().find(|c| c.number == card_no)?;
    let pcm = card
        .pcms
        .iter()
        .find(|p| p.device == device && p.direction == want)?;
    Some((card, pcm))
}

fn device_from_node(
    node: &PwNode,
    direction: Direction,
    pcm: Option<(&ProcCard, &ProcPcm)>,
    pw: &PwDump,
) -> Device {
    let alsa = match pcm {
        Some((card, pcm)) => Some(alsa_pcm(card, pcm)),
        None => node.alsa_card.map(|card| AlsaPcm {
            card,
            card_id: None,
            device: node.alsa_device.unwrap_or(0),
            card_driver: None,
            pcm_name: None,
        }),
    };

    let usb = pcm
        .map(|(card, pcm)| usb_capabilities(card, pcm))
        .unwrap_or_default();
    let from_pw = pw_capabilities(&node.enum_formats);
    let capabilities = Capabilities {
        rates: usb.rates.or(from_pw.rates),
        formats: usb.formats.or(from_pw.formats),
        channels: usb.channels.or(from_pw.channels),
        ..Default::default()
    };

    let running = pcm
        .and_then(|(_, pcm)| pcm.running.clone())
        .map(|r| Sourced::new(r, Source::Procfs))
        .or_else(|| pw_running(node));

    let is_default = [&pw.default_sink, &pw.default_source]
        .iter()
        .any(|d| d.as_deref() == Some(node.name.as_str()));

    Device {
        id: node.name.clone(),
        description: node
            .description
            .clone()
            .unwrap_or_else(|| node.name.clone()),
        direction,
        kind: kind_from_node(node, pcm),
        alsa,
        pipewire: Some(PipeWireNode {
            object_id: node.id,
            node_name: node.name.clone(),
            media_class: node.media_class.clone(),
            state: node.state.clone(),
            is_default,
        }),
        capabilities,
        running,
        kernel_probe: None,
    }
}

fn device_from_pcm(card: &ProcCard, pcm: &ProcPcm) -> Device {
    Device {
        id: format!("alsa:{}:{}:{}", card.id, pcm.device, pcm.direction.short()),
        description: format!("{}: {}", card.name, pcm.name),
        direction: pcm.direction,
        kind: kind_from_pcm(card, pcm),
        alsa: Some(alsa_pcm(card, pcm)),
        pipewire: None,
        capabilities: usb_capabilities(card, pcm),
        running: pcm.running.clone().map(|r| Sourced::new(r, Source::Procfs)),
        kernel_probe: None,
    }
}

fn alsa_pcm(card: &ProcCard, pcm: &ProcPcm) -> AlsaPcm {
    AlsaPcm {
        card: card.number,
        card_id: Some(card.id.clone()),
        device: pcm.device,
        card_driver: Some(card.driver.clone()),
        pcm_name: Some(pcm.name.clone()),
    }
}

/// Capabilities from the USB stream file for this PCM, if the card has one.
fn usb_capabilities(card: &ProcCard, pcm: &ProcPcm) -> Capabilities {
    let Some(stream) = card.usb_streams.iter().find(|s| s.device == pcm.device) else {
        return Capabilities::default();
    };
    let dir = match pcm.direction {
        Direction::Capture => stream.capture.as_ref(),
        _ => stream.playback.as_ref(),
    };
    let Some(dir) = dir else {
        return Capabilities::default();
    };

    let rates = dir
        .altsets
        .iter()
        .filter_map(|a| a.rates.clone())
        .reduce(RateSet::union);
    let formats = sorted_unique(dir.altsets.iter().flat_map(|a| a.formats.iter().cloned()));
    let channels = sorted_unique(dir.altsets.iter().filter_map(|a| a.channels));

    Capabilities {
        rates: rates.map(|r| Sourced::new(r, Source::Procfs)),
        formats: non_empty(formats).map(|f| Sourced::new(f, Source::Procfs)),
        channels: non_empty(channels).map(|c| Sourced::new(c, Source::Procfs)),
        ..Default::default()
    }
}

fn pw_capabilities(formats: &[PwFormat]) -> Capabilities {
    let rates = formats
        .iter()
        .filter_map(|f| f.rates.clone())
        .reduce(RateSet::union);
    let sample_formats = sorted_unique(formats.iter().flat_map(|f| f.formats.iter().cloned()));
    let channels = sorted_unique(formats.iter().flat_map(|f| f.channels.iter().copied()));
    Capabilities {
        rates: rates.map(|r| Sourced::new(r, Source::PipeWire)),
        formats: non_empty(sample_formats).map(|f| Sourced::new(f, Source::PipeWire)),
        channels: non_empty(channels).map(|c| Sourced::new(c, Source::PipeWire)),
        ..Default::default()
    }
}

/// Running state from PipeWire alone (Bluetooth, or when /proc/asound is unavailable).
fn pw_running(node: &PwNode) -> Option<Sourced<Running>> {
    let state = node.state.as_deref()?;
    let running = if state == "running" {
        let fmt = node.current_format.as_ref()?;
        let rate = match fmt.rates.as_ref()? {
            RateSet::Discrete { rates } => *rates.first()?,
            RateSet::Range { .. } => return None,
        };
        Running::Active(StreamParams {
            rate,
            format: fmt.formats.first().cloned(),
            channels: fmt.channels.first().copied(),
            period_frames: None,
            buffer_frames: None,
        })
    } else {
        Running::Idle
    };
    Some(Sourced::new(running, Source::PipeWire))
}

fn kind_from_node(node: &PwNode, pcm: Option<(&ProcCard, &ProcPcm)>) -> DeviceKind {
    if node.device_api.as_deref() == Some("bluez5") || node.bus.as_deref() == Some("bluetooth") {
        return DeviceKind::Bluetooth;
    }
    let hints = [
        node.profile_name.as_deref(),
        node.alsa_path.as_deref(),
        Some(node.name.as_str()),
        pcm.map(|(_, p)| p.id.as_str()),
    ];
    if hints.into_iter().flatten().any(looks_like_hdmi) {
        return DeviceKind::Hdmi;
    }
    if let Some((card, pcm)) = pcm {
        return kind_from_pcm(card, pcm);
    }
    match node.bus.as_deref() {
        Some("usb") => DeviceKind::Usb,
        Some("pci") => DeviceKind::BuiltIn,
        _ if node.device_api.as_deref() == Some("alsa") => DeviceKind::BuiltIn,
        _ => DeviceKind::Unknown,
    }
}

fn kind_from_pcm(card: &ProcCard, pcm: &ProcPcm) -> DeviceKind {
    if card.driver == "USB-Audio" {
        DeviceKind::Usb
    } else if looks_like_hdmi(&pcm.id) || looks_like_hdmi(&pcm.name) {
        DeviceKind::Hdmi
    } else {
        DeviceKind::BuiltIn
    }
}

fn looks_like_hdmi(s: &str) -> bool {
    let s = s.to_ascii_lowercase();
    s.contains("hdmi") || s.contains("displayport")
}

fn sorted_unique<T: Ord>(items: impl Iterator<Item = T>) -> Vec<T> {
    let mut v: Vec<T> = items.collect();
    v.sort();
    v.dedup();
    v
}

fn non_empty<T>(v: Vec<T>) -> Option<Vec<T>> {
    (!v.is_empty()).then_some(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::facts::{UsbAltset, UsbStream, UsbStreamDirection};
    use crate::core::model::SampleFormat;

    fn usb_card() -> ProcCard {
        ProcCard {
            number: 2,
            id: "DAC".into(),
            driver: "USB-Audio".into(),
            name: "USB DAC".into(),
            long_name: String::new(),
            pcms: vec![ProcPcm {
                device: 0,
                direction: Direction::Playback,
                id: "USB Audio".into(),
                name: "USB Audio".into(),
                running: Some(Running::Idle),
            }],
            usb_streams: vec![UsbStream {
                device: 0,
                playback: Some(UsbStreamDirection {
                    altsets: vec![
                        UsbAltset {
                            formats: vec![SampleFormat::S24_3Le],
                            rates: Some(RateSet::discrete([44_100, 48_000])),
                            channels: Some(2),
                            ..Default::default()
                        },
                        UsbAltset {
                            formats: vec![SampleFormat::S32Le],
                            rates: Some(RateSet::discrete([44_100, 48_000, 96_000])),
                            channels: Some(2),
                            ..Default::default()
                        },
                    ],
                    ..Default::default()
                }),
                capture: None,
            }],
        }
    }

    fn usb_node() -> PwNode {
        PwNode {
            id: 70,
            name: "alsa_output.usb-dac".into(),
            description: Some("USB DAC".into()),
            media_class: "Audio/Sink".into(),
            state: Some("suspended".into()),
            device_api: Some("alsa".into()),
            bus: Some("usb".into()),
            alsa_card: Some(2),
            alsa_device: Some(0),
            enum_formats: vec![PwFormat {
                formats: vec![SampleFormat::S32Le],
                rates: Some(RateSet::range(44_100, 768_000)),
                channels: vec![2],
            }],
            ..Default::default()
        }
    }

    #[test]
    fn usb_stream_beats_pipewire_and_pcm_is_claimed() {
        let procfs = ProcAsound {
            cards: vec![usb_card()],
        };
        let pw = PwDump {
            nodes: vec![usb_node()],
            default_sink: Some("alsa_output.usb-dac".into()),
            ..Default::default()
        };
        let snap = run(Some(&procfs), Some(&pw), None);
        assert_eq!(snap.devices.len(), 1, "PCM should merge into the node");
        let d = &snap.devices[0];
        assert_eq!(d.kind, DeviceKind::Usb);
        let rates = d.capabilities.rates.as_ref().unwrap();
        assert_eq!(rates.source, Source::Procfs);
        assert_eq!(rates.value, RateSet::discrete([44_100, 48_000, 96_000]));
        assert_eq!(
            d.capabilities.formats.as_ref().unwrap().value,
            vec![SampleFormat::S24_3Le, SampleFormat::S32Le]
        );
        assert_eq!(d.running.as_ref().unwrap().source, Source::Procfs);
        assert!(d.pipewire.as_ref().unwrap().is_default);
        assert_eq!(d.alsa.as_ref().unwrap().hw_id(), "hw:CARD=DAC,DEV=0");
    }

    #[test]
    fn pipewire_only_and_virtual_nodes_skipped() {
        let mut virt = usb_node();
        virt.name = "easyeffects_sink".into();
        virt.device_api = None;
        let pw = PwDump {
            nodes: vec![usb_node(), virt],
            ..Default::default()
        };
        let snap = run(None, Some(&pw), None);
        assert_eq!(snap.devices.len(), 1);
        let d = &snap.devices[0];
        assert_eq!(
            d.capabilities.rates.as_ref().unwrap().source,
            Source::PipeWire
        );
        assert_eq!(d.running.as_ref().unwrap().value, Running::Idle);
        assert_eq!(d.alsa.as_ref().unwrap().hw_id(), "hw:2,0");
    }

    #[test]
    fn procfs_only() {
        let procfs = ProcAsound {
            cards: vec![usb_card()],
        };
        let snap = run(Some(&procfs), None, None);
        assert_eq!(snap.devices[0].id, "alsa:DAC:0:playback");
        assert_eq!(snap.devices[0].description, "USB DAC: USB Audio");
        assert!(snap.clock.is_none());
    }

    fn run(
        procfs: Option<&ProcAsound>,
        pw: Option<&PwDump>,
        kernel: Option<&[KernelPcmProbe]>,
    ) -> Snapshot {
        let stack = crate::core::stack::detect(pw, &Default::default(), procfs.is_some());
        merge(
            Facts {
                procfs,
                pipewire: pw,
                kernel,
                stack,
            },
            vec![],
        )
    }

    #[test]
    fn kernel_beats_usb_stream_and_busy_keeps_other_sources() {
        use crate::core::facts::KernelCaps;
        use crate::core::model::FrameRange;

        let mut card = usb_card();
        card.pcms.push(ProcPcm {
            device: 1,
            direction: Direction::Playback,
            id: "USB Audio #1".into(),
            name: "USB Audio #1".into(),
            running: Some(Running::Idle),
        });
        let procfs = ProcAsound { cards: vec![card] };
        let kernel = [
            KernelPcmProbe {
                card: 2,
                device: 0,
                direction: Direction::Playback,
                result: KernelOutcome::Ok(KernelCaps {
                    formats: vec![SampleFormat::S32Le],
                    rates: RateSet::discrete([48_000, 96_000]),
                    channels: vec![2],
                    period_frames: Some(FrameRange { min: 16, max: 8192 }),
                    buffer_frames: None,
                }),
            },
            KernelPcmProbe {
                card: 2,
                device: 1,
                direction: Direction::Playback,
                result: KernelOutcome::Busy,
            },
        ];
        let snap = run(Some(&procfs), None, Some(&kernel));
        let d0 = &snap.devices[0];
        assert_eq!(d0.kernel_probe, Some(KernelProbeStatus::Ok));
        let rates = d0.capabilities.rates.as_ref().unwrap();
        assert_eq!(
            (rates.source, rates.value.clone()),
            (Source::Kernel, RateSet::discrete([48_000, 96_000]))
        );
        assert_eq!(
            d0.capabilities.period_frames.as_ref().unwrap().value,
            FrameRange { min: 16, max: 8192 }
        );
        assert!(d0.capabilities.buffer_frames.is_none());

        let d1 = &snap.devices[1];
        assert_eq!(d1.kernel_probe, Some(KernelProbeStatus::Busy));
        assert!(
            d1.capabilities.rates.is_none(),
            "no USB stream for device 1"
        );
    }
}
