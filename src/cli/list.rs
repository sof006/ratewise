//! `ratewise list [--json]`.

use std::fmt::Write;

use crate::core::model::{
    ClockSettings, Device, Direction, FrameRange, KernelProbeStatus, NO_DEVICES_MESSAGE, RateSet,
    Running, SampleFormat, Snapshot, Sourced, khz,
};
use crate::probe::Probes;

pub fn run(probes: &Probes, json: bool) -> anyhow::Result<()> {
    let snap = probes.snapshot();
    if json {
        super::emit(&render_json(&snap)?)
    } else {
        super::emit(&render_text(&snap))
    }
}

/// The documented JSON schema (`docs/json-schema.md`).
pub fn render_json(snap: &Snapshot) -> anyhow::Result<String> {
    Ok(serde_json::to_string_pretty(snap)? + "\n")
}

pub fn render_text(snap: &Snapshot) -> String {
    let mut s = String::new();
    if snap.devices.is_empty() {
        let _ = writeln!(s, "{NO_DEVICES_MESSAGE}\n");
    }

    let groups: [(&str, &[Direction]); 2] = [
        ("Outputs", &[Direction::Playback, Direction::Duplex]),
        ("Inputs", &[Direction::Capture]),
    ];
    for (title, dirs) in groups {
        let devices: Vec<&Device> = snap
            .devices
            .iter()
            .filter(|d| dirs.contains(&d.direction))
            .collect();
        if devices.is_empty() {
            continue;
        }
        let _ = writeln!(s, "{title}");
        for d in devices {
            device(&mut s, d);
        }
        let _ = writeln!(s);
    }

    let _ = writeln!(s, "Sound server: {}", snap.stack.summary());
    if let Some(clock) = &snap.clock {
        let _ = writeln!(
            s,
            "PipeWire clock: {}  [from {}]",
            clock_text(&clock.value),
            clock.source
        );
    }
    let _ = writeln!(s);

    if !snap.notes.is_empty() {
        let _ = writeln!(s, "Notes");
        for n in &snap.notes {
            let _ = writeln!(s, "  {}: {}", n.source, n.message);
        }
    }
    s
}

pub(crate) fn device(s: &mut String, d: &Device) {
    let default = match &d.pipewire {
        Some(p) if p.is_default => ", default",
        _ => "",
    };
    let _ = writeln!(s, "  {} — {}{default}", d.description, d.kind.label());
    row(s, "running", d.running.as_ref(), running_text, "unknown");
    let caps = &d.capabilities;
    row(s, "rates", caps.rates.as_ref(), rates_text, "unknown");
    row(
        s,
        "formats",
        caps.formats.as_ref(),
        |f| formats_text(f),
        "unknown",
    );
    row(
        s,
        "channels",
        caps.channels.as_ref(),
        |c| join(c.iter().map(u32::to_string)),
        "unknown",
    );
    let frames = |r: &FrameRange| format!("{}–{} frames", r.min, r.max);
    if caps.period_frames.is_some() {
        row(s, "period", caps.period_frames.as_ref(), frames, "");
    }
    if caps.buffer_frames.is_some() {
        row(s, "buffer", caps.buffer_frames.as_ref(), frames, "");
    }
    let kernel_note = match &d.kernel_probe {
        Some(KernelProbeStatus::Busy) => Some(
            "busy: another program has it open, so the details above come from other sources"
                .to_string(),
        ),
        Some(KernelProbeStatus::PermissionDenied) => {
            Some("no permission to open it in /dev/snd. Are you in the `audio` group?".to_string())
        }
        Some(KernelProbeStatus::Failed { message }) => Some(format!("probe failed: {message}")),
        Some(KernelProbeStatus::Ok) | None => None,
    };
    if let Some(note) = kernel_note {
        let _ = writeln!(s, "    {:<9} {note}", "kernel");
    }
    let _ = writeln!(s, "    {:<9} {}", "id", d.id);
    if let Some(alsa) = &d.alsa {
        let _ = writeln!(s, "    {:<9} {}", "alsa", alsa.hw_id());
    }
}

fn row<T>(
    s: &mut String,
    label: &str,
    v: Option<&Sourced<T>>,
    text: impl Fn(&T) -> String,
    missing: &str,
) {
    match v {
        Some(v) => {
            let _ = writeln!(s, "    {label:<9} {}  [from {}]", text(&v.value), v.source);
        }
        None => {
            let _ = writeln!(s, "    {label:<9} {missing}");
        }
    }
}

fn running_text(r: &Running) -> String {
    let Running::Active(p) = r else {
        return "idle".into();
    };
    let mut parts = vec![format!("{} kHz", khz(p.rate))];
    if let Some(f) = &p.format {
        parts.push(format!("{} ({f})", f.depth_label()));
    }
    if let Some(c) = p.channels {
        parts.push(format!("{c} ch"));
    }
    if let (Some(frames), Some(ms)) = (p.period_frames, p.period_ms()) {
        parts.push(format!("period {frames} ({ms:.1} ms)"));
    }
    if let (Some(frames), Some(ms)) = (p.buffer_frames, p.buffer_ms()) {
        parts.push(format!("buffer {frames} ({ms:.1} ms)"));
    }
    parts.join(" · ")
}

fn rates_text(r: &RateSet) -> String {
    match r {
        _ if r.is_unbounded() => "any (PipeWire converts)".into(),
        RateSet::Discrete { rates } => format!("{} kHz", join(rates.iter().map(|&r| khz(r)))),
        RateSet::Range { min, max } => format!("{}–{} kHz (continuous)", khz(*min), khz(*max)),
    }
}

fn formats_text(f: &[SampleFormat]) -> String {
    join(f.iter().map(|f| match f.bits() {
        Some(_) => format!("{f} ({})", f.depth_label()),
        None => f.to_string(),
    }))
}

fn clock_text(c: &ClockSettings) -> String {
    let mut parts = Vec::new();
    if let Some(r) = c.rate {
        parts.push(format!("default {} kHz", khz(r)));
    }
    if let Some(rates) = &c.allowed_rates {
        parts.push(format!(
            "allowed {} kHz",
            join(rates.iter().map(|&r| khz(r)))
        ));
    }
    if let Some(q) = c.quantum {
        let bounds: Vec<String> = [("min", c.min_quantum), ("max", c.max_quantum)]
            .into_iter()
            .filter_map(|(k, v)| Some(format!("{k} {}", v?)))
            .collect();
        if bounds.is_empty() {
            parts.push(format!("quantum {q}"));
        } else {
            parts.push(format!("quantum {q} ({})", bounds.join(", ")));
        }
    }
    if let Some(r) = c.force_rate.filter(|&r| r > 0) {
        parts.push(format!("forced rate {} kHz", khz(r)));
    }
    if let Some(q) = c.force_quantum.filter(|&q| q > 0) {
        parts.push(format!("forced quantum {q}"));
    }
    if parts.is_empty() {
        "no settings reported".into()
    } else {
        parts.join(" · ")
    }
}

fn join(items: impl Iterator<Item = String>) -> String {
    items.collect::<Vec<_>>().join(", ")
}
