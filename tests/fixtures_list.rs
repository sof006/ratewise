//! `ratewise list` against every fixture: golden output plus targeted checks.

mod common;

use common::{assert_golden, fixture};
use ratewise::cli::list::{render_json, render_text};
use ratewise::core::model::{
    DeviceKind, Direction, RateSet, Running, SampleFormat, Snapshot, Source,
};
use ratewise::probe::Probes;

const FIXTURES: &[&str] = &[
    "pw-1.2-desktop",
    "pw-0.3-laptop",
    "alsa-only",
    "pulseaudio-only",
    "no-sound",
];

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
    // 5 hardware nodes + HDMI 1, which PipeWire's profile doesn't use. The virtual
    // EasyEffects sink and the Firefox stream are not devices.
    assert_eq!(snap.devices.len(), 6);
    assert!(snap.devices.iter().all(|d| d.id != "easyeffects_sink"));

    let dac = device(&snap, "alsa_output.usb-Topping_D50s-00.analog-stereo");
    assert_eq!(dac.kind, DeviceKind::Usb);
    assert!(dac.pipewire.as_ref().unwrap().is_default);
    let rates = dac.capabilities.rates.as_ref().unwrap();
    assert_eq!(
        rates.source,
        Source::Procfs,
        "USB stream file beats PipeWire's range"
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
    assert_eq!(
        hdmi.capabilities.rates.as_ref().unwrap().source,
        Source::PipeWire
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
    // HDMI isn't in the active profile, so it shows up from /proc/asound only.
    assert!(device(&snap, "alsa:PCH:3:playback").pipewire.is_none());
}

#[test]
fn alsa_only() {
    let snap = snapshot("alsa-only");
    assert!(snap.clock.is_none());
    assert_eq!(snap.notes.len(), 1);
    assert_eq!(snap.notes[0].source, Source::PipeWire);
    let mic = device(&snap, "alsa:Device:0:capture");
    assert_eq!(mic.kind, DeviceKind::Usb);
    assert_eq!(
        mic.capabilities.rates.as_ref().unwrap().value,
        RateSet::range(8_000, 48_000)
    );
    assert_eq!(mic.capabilities.channels.as_ref().unwrap().value, vec![1]);
}

#[test]
fn no_sound() {
    let snap = snapshot("no-sound");
    assert!(snap.devices.is_empty());
    assert_eq!(snap.notes.len(), 2);
    assert!(render_text(&snap).starts_with("No sound devices found."));
}
