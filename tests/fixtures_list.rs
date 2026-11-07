//! `ratewise list` against every fixture: golden output plus targeted checks.

mod common;

use common::{assert_golden, fixture};
use ratewise::cli::list::{render_json, render_text};
use ratewise::core::model::{
    BluetoothMode, DeviceKind, Direction, FrameRange, KernelProbeStatus, RateSet, Running,
    SampleFormat, SessionManager, Snapshot, SoundServer, Source, WirePlumberConfig,
};
use ratewise::probe::Probes;

const FIXTURES: &[&str] = &[
    "arch-desktop",
    "arch-desktop-2",
    "pw-1.2-desktop",
    "pw-0.3-laptop",
    "alsa-only",
    "pulseaudio-only",
    "no-sound",
];

fn wireplumber(version: &str, config: WirePlumberConfig) -> Option<SessionManager> {
    Some(SessionManager::WirePlumber {
        version: Some(version.into()),
        config: Some(config),
    })
}

fn snapshot(name: &str) -> Snapshot {
    Probes::fixture(&fixture(name)).snapshot()
}

fn device<'a>(snap: &'a Snapshot, id: &str) -> &'a ratewise::core::model::Device {
    snap.devices
        .iter()
        .find(|d| d.id == id)
        .unwrap_or_else(|| panic!("no device {id}"))
}

#[test]
fn golden_output() {
    for name in FIXTURES {
        let snap = snapshot(name);
        assert_golden(&format!("list/{name}.json"), &render_json(&snap).unwrap());
        assert_golden(&format!("list/{name}.txt"), &render_text(&snap));
    }
}

#[test]
fn json_round_trips() {
    for name in FIXTURES {
        let snap = snapshot(name);
        let back: Snapshot = serde_json::from_str(&render_json(&snap).unwrap()).unwrap();
        assert_eq!(back, snap, "{name}");
    }
}

#[test]
fn pipewire_1_2_desktop() {
    let snap = snapshot("pw-1.2-desktop");
    assert!(snap.notes.is_empty(), "{:?}", snap.notes);
    assert_eq!(snap.pipewire_version.as_deref(), Some("1.2.7"));
    // 5 hardware nodes + HDMI 1, which PipeWire's profile doesn't use, + a Bluetooth
    // speaker that's switched off. The virtual EasyEffects sink and the Firefox
    // stream are not devices.
    assert_eq!(snap.devices.len(), 7);
    assert!(snap.devices.iter().all(|d| d.id != "easyeffects_sink"));

    let dac = device(&snap, "alsa_output.usb-Topping_D50s-00.analog-stereo");
    assert_eq!(dac.kind, DeviceKind::Usb);
    assert!(dac.pipewire.as_ref().unwrap().is_default);
    assert_eq!(dac.kernel_probe, Some(KernelProbeStatus::Busy));
    let rates = dac.capabilities.rates.as_ref().unwrap();
    assert_eq!(
        rates.source,
        Source::Procfs,
        "busy, so the USB stream file beats PipeWire's range"
    );
    assert_eq!(rates.value.standard_rates().len(), 10);
    assert_eq!(
        dac.capabilities.formats.as_ref().unwrap().value,
        vec![SampleFormat::S24_3Le, SampleFormat::S32Le]
    );
    let running = dac.running.as_ref().unwrap();
    assert_eq!(running.source, Source::Procfs);
    let Running::Active(p) = &running.value else {
        panic!("DAC should be running")
    };
    assert_eq!(
        (p.rate, p.format.clone()),
        (96_000, Some(SampleFormat::S32Le))
    );

    let hdmi = device(&snap, "alsa_output.pci-0000_01_00.1.hdmi-stereo");
    assert_eq!(hdmi.kind, DeviceKind::Hdmi);
    assert_eq!(hdmi.alsa.as_ref().unwrap().hw_id(), "hw:CARD=NVidia,DEV=3");
    assert_eq!(hdmi.kernel_probe, Some(KernelProbeStatus::Ok));
    let rates = hdmi.capabilities.rates.as_ref().unwrap();
    assert_eq!(rates.source, Source::Kernel, "idle, so the kernel answered");
    assert_eq!(rates.value.standard_rates().len(), 7);
    assert_eq!(
        hdmi.capabilities.channels.as_ref().unwrap().value,
        (2..=8).collect::<Vec<_>>()
    );
    assert_eq!(
        hdmi.capabilities.buffer_frames.as_ref().unwrap().value,
        FrameRange {
            min: 64,
            max: 1_048_576
        }
    );

    let unused_hdmi = device(&snap, "alsa:NVidia:7:playback");
    assert!(unused_hdmi.pipewire.is_none());
    assert_eq!(unused_hdmi.kind, DeviceKind::Hdmi);

    let bt = device(&snap, "bluez_output.00_00_00_00_00_01.1");
    assert_eq!(bt.kind, DeviceKind::Bluetooth);
    assert!(bt.alsa.is_none());
    assert_eq!(bt.running.as_ref().unwrap().value, Running::Idle);
    assert_eq!(bt.running.as_ref().unwrap().source, Source::PipeWire);

    let mic = device(&snap, "alsa_input.pci-0000_00_1f.3.analog-stereo");
    assert_eq!(mic.direction, Direction::Capture);
    assert!(mic.pipewire.as_ref().unwrap().is_default);

    let clock = &snap.clock.as_ref().unwrap().value;
    assert_eq!(clock.rate, Some(48_000));
    assert_eq!(
        clock.allowed_rates,
        Some(vec![44_100, 48_000, 88_200, 96_000])
    );
    assert_eq!(
        (clock.min_quantum, clock.max_quantum),
        (Some(32), Some(2048))
    );

    let st = &snap.stack;
    assert_eq!(st.server, SoundServer::PipeWire);
    assert_eq!(st.pipewire_version.as_deref(), Some("1.2.7"));
    assert!(st.pipewire_pulse);
    assert_eq!(
        st.session_manager,
        wireplumber("0.5.6", WirePlumberConfig::SpaJson05)
    );
    assert_eq!(st.systemd_user_session, Some(true));
    assert_eq!(
        st.summary(),
        "PipeWire 1.2.7 · WirePlumber 0.5.6 · pipewire-pulse"
    );
}

#[test]
fn pipewire_0_3_string_values() {
    let snap = snapshot("pw-0.3-laptop");
    assert!(snap.notes.is_empty(), "{:?}", snap.notes);
    let sink = device(&snap, "alsa_output.pci-0000_00_1f.3.analog-stereo");
    // alsa.card/alsa.device were strings; they still matched the PCM.
    assert_eq!(sink.alsa.as_ref().unwrap().hw_id(), "hw:CARD=PCH,DEV=0");
    assert!(sink.pipewire.as_ref().unwrap().is_default);
    assert_eq!(
        sink.capabilities.rates.as_ref().unwrap().value,
        RateSet::range(44_100, 192_000)
    );
    let clock = &snap.clock.as_ref().unwrap().value;
    assert_eq!(clock.allowed_rates, Some(vec![48_000]));
    assert_eq!(clock.quantum, Some(1024));
    assert_eq!(clock.min_quantum, None);
    // Not in the `audio` group: the kernel refused, PipeWire's answer stands.
    assert_eq!(sink.kernel_probe, Some(KernelProbeStatus::PermissionDenied));
    assert_eq!(
        sink.capabilities.rates.as_ref().unwrap().source,
        Source::PipeWire
    );
    // HDMI isn't in the active profile, so it shows up from /proc/asound only.
    let hdmi = device(&snap, "alsa:PCH:3:playback");
    assert!(hdmi.pipewire.is_none());
    assert!(matches!(
        hdmi.kernel_probe,
        Some(KernelProbeStatus::Failed { .. })
    ));
    assert_eq!(
        snap.stack.session_manager,
        wireplumber("0.4.8", WirePlumberConfig::Lua04)
    );
}

#[test]
fn alsa_only() {
    let snap = snapshot("alsa-only");
    assert!(snap.clock.is_none());
    assert_eq!(snap.notes.len(), 1);
    assert_eq!(snap.notes[0].source, Source::PipeWire);
    assert_eq!(snap.stack.server, SoundServer::Alsa);
    assert_eq!(snap.stack.summary(), "ALSA only (no sound server)");

    let mic = device(&snap, "alsa:Device:0:capture");
    assert_eq!(mic.kind, DeviceKind::Usb);
    // The kernel's per-rate answers replace the USB stream file's continuous range.
    let rates = mic.capabilities.rates.as_ref().unwrap();
    assert_eq!(rates.source, Source::Kernel);
    assert_eq!(
        rates.value,
        RateSet::discrete([8_000, 11_025, 16_000, 22_050, 32_000, 44_100, 48_000])
    );
    assert_eq!(mic.capabilities.channels.as_ref().unwrap().value, vec![1]);

    // The HDA output was running, so its device node was busy.
    let out = device(&snap, "alsa:PCH:0:playback");
    assert_eq!(out.kernel_probe, Some(KernelProbeStatus::Busy));
    assert!(out.capabilities.rates.is_none());
    assert!(matches!(
        out.running.as_ref().unwrap().value,
        Running::Active(_)
    ));
}

#[test]
fn pulseaudio_only() {
    let snap = snapshot("pulseaudio-only");
    let st = &snap.stack;
    assert_eq!(st.server, SoundServer::PulseAudio);
    assert_eq!(st.pulseaudio_version.as_deref(), Some("15.99.1"));
    assert!(!st.pipewire_pulse);
    assert_eq!(st.session_manager, None);
    assert_eq!(snap.notes.len(), 1, "{:?}", snap.notes);
    assert_eq!(snap.notes[0].source, Source::PipeWire);
}

#[test]
fn no_sound() {
    let snap = snapshot("no-sound");
    assert!(snap.devices.is_empty());
    assert_eq!(snap.stack.server, SoundServer::None);
    let sources: Vec<Source> = snap.notes.iter().map(|n| n.source).collect();
    assert_eq!(sources, [Source::Procfs, Source::PipeWire, Source::Kernel]);
    assert!(render_text(&snap).starts_with("No sound devices found."));
}

/// Real capture: Arch/CachyOS, PipeWire 1.6.10, WirePlumber 0.5.18.
#[test]
fn real_arch_desktop() {
    let snap = snapshot("arch-desktop");
    // Captured before kernel.json existed.
    assert_eq!(snap.notes.len(), 1, "{:?}", snap.notes);
    assert_eq!(snap.notes[0].source, Source::Kernel);
    // No processes.txt either: WirePlumber and pipewire-pulse are found from pw-dump's clients.
    let st = &snap.stack;
    assert_eq!(
        st.session_manager,
        wireplumber("0.5.18", WirePlumberConfig::SpaJson05)
    );
    assert!(st.pipewire_pulse);
    assert_eq!(snap.pipewire_version.as_deref(), Some("1.6.10"));
    // 4 hardware nodes + 3 HDMI PCMs the active profile doesn't use.
    assert_eq!(snap.devices.len(), 7);

    // Topping E30 II Lite on a UCM (HiFi) profile, playing at the time of capture.
    let dac = device(
        &snap,
        "alsa_output.usb-Topping_E30_II_Lite-00.HiFi__Headphones__sink",
    );
    assert_eq!(dac.kind, DeviceKind::Usb);
    assert!(dac.pipewire.as_ref().unwrap().is_default);
    assert_eq!(dac.alsa.as_ref().unwrap().hw_id(), "hw:CARD=Lite,DEV=0");
    let rates = dac.capabilities.rates.as_ref().unwrap();
    assert_eq!(rates.source, Source::Procfs);
    assert_eq!(rates.value.standard_rates().len(), 10);
    let formats = &dac.capabilities.formats.as_ref().unwrap().value;
    assert!(formats.contains(&SampleFormat::S32Le));
    assert!(formats.contains(&SampleFormat::Other("DSD_U32_BE".into())));
    let Running::Active(p) = &dac.running.as_ref().unwrap().value else {
        panic!("DAC was playing during capture")
    };
    assert_eq!(
        (p.rate, p.format.clone()),
        (48_000, Some(SampleFormat::S32Le))
    );
    assert_eq!(p.period_frames, Some(512));

    let hdmi = device(&snap, "alsa_output.pci-0000_03_00.1.hdmi-stereo-extra1");
    assert_eq!(hdmi.kind, DeviceKind::Hdmi);
    assert_eq!(hdmi.alsa.as_ref().unwrap().hw_id(), "hw:CARD=HDMI,DEV=7");
    assert_eq!(
        hdmi.capabilities.rates.as_ref().unwrap(),
        &ratewise::core::model::Sourced::new(RateSet::range(32_000, 48_000), Source::PipeWire)
    );
    for dev in [3, 8, 9] {
        assert!(
            device(&snap, &format!("alsa:HDMI:{dev}:playback"))
                .pipewire
                .is_none()
        );
    }

    let mic = device(
        &snap,
        "alsa_input.usb-SteelSeries_Arctis_Nova_7-00.mono-fallback",
    );
    assert_eq!(mic.direction, Direction::Capture);
    assert!(mic.pipewire.as_ref().unwrap().is_default);
    assert_eq!(mic.capabilities.channels.as_ref().unwrap().value, vec![1]);

    let clock = &snap.clock.as_ref().unwrap().value;
    assert_eq!(
        clock.allowed_rates,
        Some(vec![48_000]),
        "SPA-JSON string in 1.6"
    );
}

/// Real capture after M2, same machine: adds kernel.json and processes.txt.
#[test]
fn real_arch_desktop_with_kernel_readings() {
    let snap = snapshot("arch-desktop-2");
    assert!(snap.notes.is_empty(), "{:?}", snap.notes);
    assert_eq!(snap.devices.len(), 7);

    let st = &snap.stack;
    assert_eq!(st.server, SoundServer::PipeWire);
    assert!(st.pipewire_pulse);
    assert_eq!(
        st.session_manager,
        wireplumber("0.5.18", WirePlumberConfig::SpaJson05)
    );
    // pipewire-pulse reports a compatibility version (15.0.0); it isn't a real PulseAudio.
    assert_eq!(st.pulseaudio_version, None);
    assert_eq!(st.systemd_user_session, Some(true));

    // Playing during capture, so the kernel said busy and /proc/asound answered.
    let dac = device(
        &snap,
        "alsa_output.usb-Topping_E30_II_Lite-00.HiFi__Headphones__sink",
    );
    assert_eq!(dac.kernel_probe, Some(KernelProbeStatus::Busy));
    assert_eq!(
        dac.capabilities.rates.as_ref().unwrap().source,
        Source::Procfs
    );
    assert!(dac.capabilities.period_frames.is_none());

    // The connected monitor's limits come from the driver, via the monitor's ELD.
    let benq = device(&snap, "alsa_output.pci-0000_03_00.1.hdmi-stereo-extra1");
    let rates = benq.capabilities.rates.as_ref().unwrap();
    assert_eq!(rates.source, Source::Kernel);
    assert_eq!(rates.value, RateSet::discrete([32_000, 44_100, 48_000]));
    assert_eq!(benq.capabilities.channels.as_ref().unwrap().value, vec![2]);

    // An unconnected HDMI port offers the driver defaults; odd channel counts are refused.
    let unused = device(&snap, "alsa:HDMI:3:playback");
    assert_eq!(
        unused.capabilities.channels.as_ref().unwrap().value,
        vec![2, 4, 6, 8]
    );
    assert_eq!(
        unused
            .capabilities
            .rates
            .as_ref()
            .unwrap()
            .value
            .standard_rates()
            .len(),
        7
    );

    let headset = device(
        &snap,
        "alsa_output.usb-SteelSeries_Arctis_Nova_7-00.analog-stereo",
    );
    assert_eq!(
        headset.capabilities.formats.as_ref().unwrap(),
        &ratewise::core::model::Sourced::new(vec![SampleFormat::S24_3Le], Source::Kernel)
    );
    assert_eq!(
        headset.capabilities.period_frames.as_ref().unwrap().value,
        FrameRange {
            min: 48,
            max: 48_000
        }
    );
}

#[test]
fn bluetooth_cards_and_profiles() {
    let snap = snapshot("pw-1.2-desktop");
    assert_eq!(snap.wireplumber.headset_autoswitch, Some(true));
    assert_eq!(snap.wireplumber.headset_autoswitch_saved, None);
    let hp = device(&snap, "bluez_output.00_00_00_00_00_01.1");
    assert_eq!(hp.kind, DeviceKind::Bluetooth);
    let bt = hp.bluetooth.as_ref().unwrap();
    assert_eq!(bt.card_name, "bluez_card.00_00_00_00_00_01");
    assert_eq!(bt.card_object_id, 48);
    assert_eq!(bt.profile.as_deref(), Some("a2dp-sink-aac"));
    assert_eq!(bt.codec.as_deref(), Some("aac"));
    assert_eq!(bt.summary(), "High quality · AAC");
    // PipeWire 1.x: the best codec's profile has no suffix; its codec is in the description.
    let best = bt.profiles.iter().find(|p| p.name == "a2dp-sink").unwrap();
    assert_eq!(best.codec(), Some("ldac"));
    assert_eq!(
        bt.codecs(BluetoothMode::HighQuality),
        ["ldac", "aac", "sbc_xq", "sbc"]
    );
    assert_eq!(bt.codecs(BluetoothMode::Headset), ["msbc", "cvsd"]);

    // Switched off: a card without nodes is still listed, so it can be turned on.
    let speaker = device(&snap, "bluez_card.00_00_00_00_00_02");
    assert!(speaker.pipewire.is_none());
    assert_eq!(
        speaker.bluetooth.as_ref().unwrap().mode(),
        BluetoothMode::Off
    );

    // PipeWire 0.3: codec-less profiles next to per-codec ones, both sides of a headset.
    let snap = snapshot("pw-0.3-laptop");
    for id in [
        "bluez_output.00_00_00_00_00_03.headset-head-unit",
        "bluez_input.00_00_00_00_00_03.headset-head-unit",
    ] {
        let bt = device(&snap, id).bluetooth.clone().unwrap();
        assert_eq!(bt.mode(), BluetoothMode::Headset);
        assert_eq!(bt.current_codec(), Some("msbc"));
        let bare = bt.profiles.iter().find(|p| p.name == "a2dp-sink").unwrap();
        assert_eq!(bare.codec(), None);
    }
    // WirePlumber 0.4 has no sm-settings metadata.
    assert_eq!(snap.wireplumber.headset_autoswitch, None);
}
