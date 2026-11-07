//! Builds what the window shows from the probed state, the saved record and the
//! pending edits. No I/O: everything comes from [`Inputs`].

use slint::{ModelRc, SharedString, VecModel};

use super::edits::Edits;
use super::{Chip, ChipRow, ClockView, DeviceView, Fact, KV, Line, NavItem, Opt, Stat};
use crate::app::Inputs;
use crate::core::bluetooth;
use crate::core::model::{
    Bluetooth, BluetoothMode, Device, DeviceKind, Direction, KernelProbeStatus, Running,
    SampleFormat, SoundServer, codec_label, khz,
};
use crate::core::plan::{self, PlanContext};
use crate::core::settings::{Change, DeviceSaved, LdacQuality, LeAudioLatency, Value, rate_list};

/// Rates offered in selectors. Higher ones appear only for devices that can use them.
const COMMON_RATES: [u32; 10] = [
    44_100, 48_000, 88_200, 96_000, 176_400, 192_000, 352_800, 384_000, 705_600, 768_000,
];
/// The rate grid on the overview: each row is a family, each column a multiple.
const RATE_GRID: [(&str, [u32; 5]); 3] = [
    ("44.1 kHz", [44_100, 88_200, 176_400, 352_800, 705_600]),
    ("48 kHz", [48_000, 96_000, 192_000, 384_000, 768_000]),
    ("Lower", [8_000, 11_025, 16_000, 22_050, 32_000]),
];
const FORMATS: [(SampleFormat, &str); 5] = [
    (SampleFormat::S16Le, "16-bit"),
    (SampleFormat::S24_3Le, "24-bit packed"),
    (SampleFormat::S24Le, "24-bit"),
    (SampleFormat::S32Le, "32-bit"),
    (SampleFormat::FloatLe, "32-bit float"),
];

/// What a view needs: the state, the edits, and whether changes can be applied.
pub struct Ctx<'a> {
    pub inputs: &'a Inputs,
    pub edits: &'a Edits,
    /// Why nothing can be applied (a captured system without a sandbox, no `$HOME`).
    pub read_only: Option<&'a str>,
}

impl Ctx<'_> {
    fn plan(&self) -> PlanContext<'_> {
        self.inputs.ctx()
    }
    fn server(&self) -> SoundServer {
        self.inputs.snapshot.stack.server
    }
    fn clock(&self) -> Option<&crate::core::model::ClockSettings> {
        self.inputs.snapshot.clock.as_ref().map(|c| &c.value)
    }
    fn saved_device(&self, id: &str) -> Option<&DeviceSaved> {
        self.inputs.saved.devices.get(id)
    }
}

pub fn model<T: Clone + 'static>(v: Vec<T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(v))
}

fn s(text: impl AsRef<str>) -> SharedString {
    text.as_ref().into()
}

fn opt(
    value: impl AsRef<str>,
    label: impl AsRef<str>,
    selected: bool,
    check: Result<Vec<String>, String>,
) -> Opt {
    let (enabled, tip) = match check {
        Ok(warnings) => (true, warnings.into_iter().next().unwrap_or_default()),
        Err(why) => (false, why),
    };
    Opt {
        value: s(value),
        label: s(label),
        enabled,
        selected,
        tip: s(tip),
    }
}

fn checked(
    ctx: &PlanContext,
    change: Change,
    device: Option<&Device>,
) -> Result<Vec<String>, String> {
    plan::check(ctx, &change, device).map_err(|e| e.0)
}

fn value_str<T: ToString>(v: Option<&Value<T>>) -> Option<String> {
    v.map(|v| match v {
        Value::Set(x) => x.to_string(),
        Value::Default => "default".into(),
    })
}

// ---- sidebar -----------------------------------------------------------------------

pub fn running_short(d: &Device) -> String {
    match d.running.as_ref().map(|r| &r.value) {
        Some(Running::Active(p)) => match &p.format {
            Some(f) => format!("{} kHz · {}", khz(p.rate), f.depth_label()),
            None => format!("{} kHz", khz(p.rate)),
        },
        Some(Running::Idle) => "idle".into(),
        None => String::new(),
    }
}

fn is_active(d: &Device) -> bool {
    matches!(
        d.running.as_ref().map(|r| &r.value),
        Some(Running::Active(_))
    )
}

fn is_default(d: &Device) -> bool {
    d.pipewire.as_ref().is_some_and(|p| p.is_default)
}

/// The sidebar and header icon for a device.
fn kind_icon(d: &Device) -> &'static str {
    match d.kind {
        DeviceKind::Bluetooth => "bluetooth",
        _ if d.direction == Direction::Capture => "mic",
        DeviceKind::Usb => "usb",
        DeviceKind::Hdmi => "hdmi",
        DeviceKind::BuiltIn => "builtin",
        DeviceKind::Unknown => "unknown",
    }
}

pub fn nav(inputs: &Inputs) -> Vec<NavItem> {
    let mut out = Vec::new();
    let header = |title: &str| NavItem {
        kind: s("header"),
        title: s(title),
        ..Default::default()
    };
    let device = |d: &Device| {
        let mut running = running_short(d);
        if let Some(bt) = &d.bluetooth {
            running = match (bt.mode(), bt.current_codec()) {
                (BluetoothMode::Off, _) => "off".into(),
                (_, Some(codec)) if running.is_empty() => codec_label(codec),
                (_, Some(codec)) => format!("{} · {running}", codec_label(codec)),
                _ => running,
            };
        }
        NavItem {
            kind: s("device"),
            key: s(format!("device:{}", d.id)),
            title: s(&d.description),
            // The icon shows the kind, so the second line is what it's doing.
            badge: s(match running.as_str() {
                "" => capitalise(d.kind.label()),
                r => capitalise(r),
            }),
            icon: s(kind_icon(d)),
            tag: s(if is_default(d) { "Default" } else { "" }),
            is_default: is_default(d),
            active: is_active(d),
        }
    };
    let devices = &inputs.snapshot.devices;
    let outputs: Vec<&Device> = devices
        .iter()
        .filter(|d| d.direction != Direction::Capture)
        .collect();
    let inputs_: Vec<&Device> = devices
        .iter()
        .filter(|d| d.direction == Direction::Capture)
        .collect();
    if !outputs.is_empty() {
        out.push(header("OUTPUTS"));
        out.extend(outputs.into_iter().map(device));
    }
    if !inputs_.is_empty() {
        out.push(header("INPUTS"));
        out.extend(inputs_.into_iter().map(device));
    }
    out.push(header("SYSTEM"));
    for (key, title, icon) in [
        ("global", "Global settings", "sliders"),
        ("doctor", "About this system", "pulse"),
    ] {
        out.push(NavItem {
            kind: s("page"),
            key: s(key),
            title: s(title),
            icon: s(icon),
            ..Default::default()
        });
    }
    out
}

// ---- device page -------------------------------------------------------------------

pub fn device_view(c: &Ctx, d: &Device) -> DeviceView {
    let pc = c.plan();
    let saved = c.saved_device(&d.id);
    let pending = |name: &str| c.edits.find(name, Some(&d.id));

    // Running now.
    let (running_main, running_detail, running_source) = match &d.running {
        Some(r) => match &r.value {
            Running::Active(p) => {
                let mut detail = Vec::new();
                if let Some(f) = &p.format {
                    detail.push(f.alsa_name().to_string());
                }
                if let Some(ch) = p.channels {
                    detail.push(format!("{ch} channels"));
                }
                if let (Some(n), Some(ms)) = (p.period_frames, p.period_ms()) {
                    detail.push(format!("period {n} frames ({ms:.1} ms)"));
                }
                if let (Some(n), Some(ms)) = (p.buffer_frames, p.buffer_ms()) {
                    detail.push(format!("buffer {n} frames ({ms:.1} ms)"));
                }
                (running_short(d), detail.join(" · "), r.source.label().to_string())
            }
            Running::Idle => (
                "Idle".to_string(),
                "Nothing is playing through it right now. The hardware starts when a program uses it.".to_string(),
                r.source.label().to_string(),
            ),
        },
        None => ("Unknown".into(), "Ratewise couldn't read its state.".into(), String::new()),
    };

    // The big numbers on the overview, and the details under them.
    let mut readout = Vec::new();
    let mut readout_detail = Vec::new();
    let state = match d.running.as_ref().map(|r| &r.value) {
        Some(Running::Active(p)) => {
            let stat = |label: &str, value: String, unit: &str| Stat {
                label: s(label),
                value: s(value),
                unit: s(unit),
            };
            readout.push(stat("RATE", khz(p.rate), "kHz"));
            if let Some(f) = &p.format {
                match f.bits() {
                    Some(b) => readout.push(stat(
                        "DEPTH",
                        b.to_string(),
                        if f.is_float() { "bit float" } else { "bit" },
                    )),
                    None => readout.push(stat("FORMAT", f.alsa_name().to_string(), "")),
                }
            }
            if let Some(ch) = p.channels {
                readout.push(stat("CHANNELS", ch.to_string(), ""));
            }
            let detail = |key: &str, value: String| KV {
                key: s(key),
                value: s(value),
                source: SharedString::new(),
            };
            if let Some(f) = &p.format {
                readout_detail.push(detail("Format", f.alsa_name().to_string()));
            }
            if let (Some(n), Some(ms)) = (p.period_frames, p.period_ms()) {
                readout_detail.push(detail("Period", format!("{n} frames · {ms:.1} ms")));
            }
            if let (Some(n), Some(ms)) = (p.buffer_frames, p.buffer_ms()) {
                readout_detail.push(detail("Buffer", format!("{n} frames · {ms:.1} ms")));
            }
            "playing"
        }
        Some(Running::Idle) => "idle",
        None => "unknown",
    };
    let current_rate = match d.running.as_ref().map(|r| &r.value) {
        Some(Running::Active(p)) => Some(p.rate),
        _ => None,
    };

    // Configured.
    let mut configured = Vec::new();
    let pipewire = c.server() == SoundServer::PipeWire;
    let row = |key: &str, value: String| KV {
        key: s(key),
        value: s(value),
        source: SharedString::new(),
    };
    let describe =
        |pending: Option<&Change>, saved: Option<String>, otherwise: String| match (pending, saved)
        {
            (Some(ch), _) => format!("{} (not applied yet)", pending_value(ch)),
            (None, Some(v)) => format!("{v} (saved by Ratewise)"),
            (None, None) => otherwise,
        };
    let clock_rate = c.clock().and_then(|k| k.rate);
    configured.push(row(
        "Rate",
        describe(
            pending("rate"),
            saved
                .and_then(|s| s.rate)
                .map(|r| format!("{} kHz", khz(r))),
            match (pipewire, clock_rate) {
                (true, Some(r)) => format!("Follows PipeWire ({} kHz)", khz(r)),
                (true, None) => "Follows PipeWire".into(),
                (false, _) => "Set globally".into(),
            },
        ),
    ));
    configured.push(row(
        "Format",
        describe(
            pending("format"),
            saved
                .and_then(|s| s.format.as_ref())
                .map(|f| format!("{} ({f})", f.depth_label())),
            if pipewire {
                "Picked by PipeWire".into()
            } else {
                "Set globally".into()
            },
        ),
    ));
    configured.push(row(
        "Period",
        describe(
            pending("period-size"),
            saved
                .and_then(|s| s.period_size)
                .map(|n| format!("{n} frames")),
            "Default".into(),
        ),
    ));
    if pending("headroom").is_some() || saved.and_then(|s| s.headroom).is_some() {
        configured.push(row(
            "Headroom",
            describe(
                pending("headroom"),
                saved
                    .and_then(|s| s.headroom)
                    .map(|n| format!("{n} frames")),
                "Default".into(),
            ),
        ));
    }
    configured.push(row(
        "When idle",
        describe(
            pending("suspend"),
            saved
                .filter(|s| s.disable_suspend)
                .map(|_| "Kept open".to_string()),
            "Suspends".into(),
        ),
    ));

    if let Some(bt) = &d.bluetooth {
        // Rate, format and period come from the codec, so show the profile instead.
        let pending_profile = match pending("profile") {
            Some(Change::BluetoothProfile(want)) => bluetooth::resolve(bt, want).ok(),
            _ => None,
        };
        configured = vec![
            row(
                "Mode",
                match pending_profile {
                    Some(p) => format!("{} (not applied yet)", bluetooth::profile_label(p)),
                    None => bt.summary(),
                },
            ),
            row("Rate & format", "Set by the codec".into()),
            row(
                "When idle",
                describe(
                    pending("suspend"),
                    saved
                        .filter(|s| s.disable_suspend)
                        .map(|_| "Kept open".to_string()),
                    "Suspends".into(),
                ),
            ),
        ];
    }

    // Supports.
    let caps = &d.capabilities;
    let mut supports = Vec::new();
    if let Some(bt) = &d.bluetooth {
        for (key, mode) in [
            ("Codecs", BluetoothMode::HighQuality),
            ("For calls", BluetoothMode::Headset),
        ] {
            let list = bt.codecs(mode);
            if !list.is_empty() {
                supports.push(KV {
                    key: s(key),
                    value: s(list
                        .iter()
                        .map(|c| codec_label(c))
                        .collect::<Vec<_>>()
                        .join(", ")),
                    source: s("PipeWire"),
                });
            }
        }
    }
    let src = |source: crate::core::model::Source| s(source.label());
    let mut rate_rows = Vec::new();
    let mut rate_source = String::new();
    match &caps.rates {
        Some(r) if !r.value.is_unbounded() => {
            // The two families in octaves, so a column is one multiple; low rates last,
            // and only when the device has any.
            let chips = |rates: &[u32]| -> Vec<Chip> {
                rates
                    .iter()
                    .map(|&rate| Chip {
                        label: s(khz(rate)),
                        on: r.value.contains(rate),
                        current: current_rate == Some(rate),
                    })
                    .collect()
            };
            for (label, rates) in RATE_GRID {
                let row = chips(&rates);
                if label == "Lower" && !row.iter().any(|c| c.on || c.current) {
                    continue;
                }
                rate_rows.push(ChipRow {
                    label: s(label),
                    chips: model(row),
                });
            }
            rate_source = r.source.label().to_string();
        }
        Some(r) => supports.push(KV {
            key: s("Rates"),
            value: s("Any: PipeWire converts, so this says nothing about the hardware"),
            source: src(r.source),
        }),
        None => supports.push(row("Rates", "Unknown".into())),
    }
    match &caps.formats {
        Some(f) => supports.push(KV {
            key: s("Formats"),
            value: s(f
                .value
                .iter()
                .map(|f| format!("{} ({f})", f.depth_label()))
                .collect::<Vec<_>>()
                .join(", ")),
            source: src(f.source),
        }),
        None => supports.push(row("Formats", "Unknown".into())),
    }
    if let Some(ch) = &caps.channels {
        supports.push(KV {
            key: s("Channels"),
            value: s(ch
                .value
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(", ")),
            source: src(ch.source),
        });
    }
    for (key, range) in [
        ("Period", &caps.period_frames),
        ("Buffer", &caps.buffer_frames),
    ] {
        if let Some(r) = range {
            supports.push(KV {
                key: s(key),
                value: s(format!("{}–{} frames", r.value.min, r.value.max)),
                source: src(r.source),
            });
        }
    }

    // Notices.
    let mut facts = Vec::new();
    let fact = |level: &str, text: String| Fact {
        text: s(text),
        level: s(level),
    };
    match &d.kernel_probe {
        Some(KernelProbeStatus::Busy) => facts.push(fact(
            "info",
            "Another program has it open, so the driver couldn't be asked directly. The details come from PipeWire and /proc/asound instead.".into(),
        )),
        Some(KernelProbeStatus::PermissionDenied) => facts.push(fact(
            "warn",
            "Ratewise isn't allowed to open this device to ask the driver what it supports. Are you in the `audio` group?".into(),
        )),
        Some(KernelProbeStatus::Failed { message }) => {
            facts.push(fact("warn", format!("Asking the driver failed: {message}.")))
        }
        _ => {}
    }
    if d.pipewire.is_none()
        && d.bluetooth
            .as_ref()
            .is_some_and(|b| b.mode() == BluetoothMode::Off)
    {
        facts.push(fact(
            "info",
            "Its profile is off, so it's connected but plays nothing. Switch it on in the Bluetooth tab.".into(),
        ));
    }
    if pipewire && d.pipewire.is_none() && d.kind != DeviceKind::Bluetooth {
        facts.push(fact(
            "info",
            "PipeWire isn't using this device: its card's profile doesn't include it. Pick a profile that does in your sound settings to use it.".into(),
        ));
    }

    // Controls.
    let can_save = |change: Change| plan::availability(&pc, &change, Some(d)).1;
    let mut edit_why = String::new();
    if pipewire && let Err(why) = can_save(Change::Format(Value::Default)) {
        edit_why = why;
    }
    let editable = edit_why.is_empty();
    let per_server = |change: Change| -> Result<(), String> {
        if !editable {
            return Err(String::new());
        }
        if !pipewire && matches!(change, Change::Rate(_) | Change::Format(_)) {
            return Err(format!(
                "{} uses one {} for every device. Set it on the Global settings page.",
                server_name(c.server()),
                if matches!(change, Change::Rate(_)) {
                    "sample rate"
                } else {
                    "sample format"
                }
            ));
        }
        can_save(change)
    };

    let saved_rate = saved.and_then(|s| s.rate);
    let (rate_opts, rate_why) = match per_server(Change::Rate(Value::Default)) {
        Ok(()) => {
            let selected = value_str(match pending("rate") {
                Some(Change::Rate(v)) => Some(v),
                _ => None,
            })
            .or(saved_rate.map(|r| r.to_string()))
            .unwrap_or_else(|| "default".into());
            let top = caps
                .rates
                .as_ref()
                .filter(|r| !r.value.is_unbounded())
                .and_then(|r| {
                    COMMON_RATES
                        .iter()
                        .rev()
                        .find(|&&x| r.value.contains(x))
                        .copied()
                })
                .unwrap_or(192_000)
                .max(192_000);
            let mut opts = vec![opt(
                "default",
                "Default",
                selected == "default",
                Ok(vec![match clock_rate {
                    Some(r) => format!("Follow PipeWire's rate ({} kHz now).", khz(r)),
                    None => "Follow PipeWire's rate.".into(),
                }]),
            )];
            for r in COMMON_RATES.iter().copied().filter(|&r| r <= top) {
                opts.push(opt(
                    r.to_string(),
                    khz(r),
                    selected == r.to_string(),
                    checked(&pc, Change::Rate(Value::Set(r)), Some(d)),
                ));
            }
            (opts, String::new())
        }
        Err(why) => (Vec::new(), why),
    };

    let (format_opts, format_why) = match per_server(Change::Format(Value::Default)) {
        Ok(()) => {
            let selected = match pending("format") {
                Some(Change::Format(Value::Set(f))) => f.alsa_name().to_string(),
                Some(_) => "default".into(),
                None => saved
                    .and_then(|s| s.format.as_ref())
                    .map(|f| f.alsa_name().to_string())
                    .unwrap_or_else(|| "default".into()),
            };
            let mut opts = vec![opt(
                "default",
                "Default",
                selected == "default",
                Ok(vec![
                    "Let PipeWire pick the best format the device supports.".into(),
                ]),
            )];
            for (f, label) in FORMATS {
                let check =
                    checked(&pc, Change::Format(Value::Set(f.clone())), Some(d)).map(|mut w| {
                        w.insert(0, format!("{} in ALSA terms.", f.alsa_name()));
                        w
                    });
                opts.push(opt(f.alsa_name(), label, selected == f.alsa_name(), check));
            }
            (opts, String::new())
        }
        Err(why) => (Vec::new(), why),
    };

    let frames_opts =
        |name: &str, change: fn(Value<u32>) -> Change, current: Option<u32>, values: &[u32]| {
            match per_server(change(Value::Default)) {
                Ok(()) => {
                    let selected = match c.edits.find(name, Some(&d.id)) {
                        Some(Change::PeriodSize(v) | Change::Headroom(v)) => value_str(Some(v)),
                        _ => None,
                    }
                    .or(current.map(|n| n.to_string()))
                    .unwrap_or_else(|| "default".into());
                    let mut opts =
                        vec![opt("default", "Default", selected == "default", Ok(vec![]))];
                    for &n in values {
                        opts.push(opt(
                            n.to_string(),
                            n.to_string(),
                            selected == n.to_string(),
                            checked(&pc, change(Value::Set(n)), Some(d)),
                        ));
                    }
                    (opts, String::new())
                }
                Err(why) => (Vec::new(), why),
            }
        };
    let (period_opts, period_why) = frames_opts(
        "period-size",
        Change::PeriodSize,
        saved.and_then(|s| s.period_size),
        &[64, 128, 256, 512, 1024, 2048, 4096],
    );
    let (headroom_opts, headroom_why) = frames_opts(
        "headroom",
        Change::Headroom,
        saved.and_then(|s| s.headroom),
        &[256, 512, 1024, 2048, 4096, 8192],
    );

    let (suspend_opts, suspend_why) = match per_server(Change::Suspend(Value::Default)) {
        Ok(()) => {
            let off = match pending("suspend") {
                Some(Change::Suspend(v)) => *v == Value::Set(false),
                _ => saved.is_some_and(|s| s.disable_suspend),
            };
            (
                vec![
                    opt("default", "Suspend when idle", !off, Ok(vec!["PipeWire's default: closes the device after a few seconds of silence.".into()])),
                    opt("off", "Keep open", off, Ok(vec!["Fixes pops and delays at the start of playback. Uses slightly more power.".into()])),
                ],
                String::new(),
            )
        }
        Err(why) => (Vec::new(), why),
    };

    let (bt_mode_opts, bt_codec_opts, bt_why) = match &d.bluetooth {
        Some(bt) => bluetooth_opts(c, d, bt),
        None => (Vec::new(), Vec::new(), String::new()),
    };
    let (le_latency_opts, le_latency_why) = match &d.bluetooth {
        Some(bt) => le_latency_opts(c, d, bt),
        None => (Vec::new(), String::new()),
    };
    let (ldac_opts, ldac_why) = match &d.bluetooth {
        Some(bt) => ldac_opts(c, d, bt),
        None => (Vec::new(), String::new()),
    };

    let raw = serde_json::to_string_pretty(&serde_json::json!({
        "device": d,
        "saved": saved,
    }))
    .unwrap_or_default();

    let mut subtitle = format!("{} {}", d.kind.label(), d.direction.short());
    if let Some(a) = &d.alsa {
        subtitle.push_str(&format!(" · {}", a.hw_id()));
    }
    if let Some(p) = &d.pipewire {
        subtitle.push_str(&format!(" · {}", p.node_name));
    }
    let mut tags = Vec::new();
    if is_default(d) {
        tags.push(s("Default"));
    }

    DeviceView {
        id: s(&d.id),
        name: s(&d.description),
        subtitle: s(subtitle),
        tags: model(tags),
        running_main: s(running_main),
        running_detail: s(running_detail),
        running_source: s(running_source),
        active: is_active(d),
        configured: model(configured),
        supports: model(supports),
        rate_rows: model(rate_rows),
        rate_source: s(rate_source),
        icon: s(kind_icon(d)),
        state: s(state),
        readout: model(readout),
        readout_detail: model(readout_detail),
        facts: model(facts),
        edit_why: s(edit_why),
        rate_opts: model(rate_opts),
        rate_why: s(rate_why),
        format_opts: model(format_opts),
        format_why: s(format_why),
        period_opts: model(period_opts),
        period_why: s(period_why),
        headroom_opts: model(headroom_opts),
        headroom_why: s(headroom_why),
        suspend_opts: model(suspend_opts),
        suspend_why: s(suspend_why),
        bluetooth: d.bluetooth.is_some(),
        bt_mode_opts: model(bt_mode_opts),
        bt_codec_opts: model(bt_codec_opts),
        bt_why: s(bt_why),
        ldac_opts: model(ldac_opts),
        ldac_why: s(ldac_why),
        le_latency_opts: model(le_latency_opts),
        le_latency_why: s(le_latency_why),
        raw: s(raw),
    }
}

/// LDAC quality, for devices that offer LDAC. Saved only.
fn ldac_opts(c: &Ctx, d: &Device, bt: &Bluetooth) -> (Vec<Opt>, String) {
    let ldac = bt.codec.as_deref() == Some("ldac")
        || bt.profiles.iter().any(|p| p.codec() == Some("ldac"));
    if !ldac {
        return (Vec::new(), String::new());
    }
    let pc = c.plan();
    if let Err(why) = plan::availability(&pc, &Change::LdacQuality(Value::Default), Some(d)).1 {
        return (Vec::new(), why);
    }
    let selected = match c.edits.find("ldac-quality", Some(&d.id)) {
        Some(Change::LdacQuality(v)) => v.as_option().map(|q| q.key()),
        _ => c
            .inputs
            .saved
            .bluetooth
            .get(&bt.card_name)
            .and_then(|b| b.ldac_quality)
            .map(LdacQuality::key),
    }
    .unwrap_or("default");
    let mut opts = vec![opt(
        "default",
        "Default",
        selected == "default",
        Ok(vec!["PipeWire's default: adapts to the connection.".into()]),
    )];
    for q in LdacQuality::ALL {
        let label = match q {
            LdacQuality::Auto => "Adaptive",
            LdacQuality::Hq => "990 kbps",
            LdacQuality::Sq => "660 kbps",
            LdacQuality::Mq => "330 kbps",
        };
        opts.push(opt(
            q.key(),
            label,
            selected == q.key(),
            checked(&pc, Change::LdacQuality(Value::Set(q)), Some(d)).map(|mut w| {
                w.insert(0, capitalise_first(q.label()));
                w
            }),
        ));
    }
    (opts, String::new())
}

/// LE Audio latency, for devices that offer LE Audio. Saved only.
fn le_latency_opts(c: &Ctx, d: &Device, bt: &Bluetooth) -> (Vec<Opt>, String) {
    if !bluetooth::is_le_audio(bt) {
        return (Vec::new(), String::new());
    }
    let pc = c.plan();
    let change = |v| Change::LeAudioLatency(v);
    if let Err(why) = plan::availability(&pc, &change(Value::Default), Some(d)).1 {
        return (Vec::new(), why);
    }
    let selected = match c.edits.find("le-audio-latency", Some(&d.id)) {
        Some(Change::LeAudioLatency(v)) => v.as_option().map(|l| l.key()),
        _ => c
            .inputs
            .saved
            .bluetooth
            .get(&bt.card_name)
            .and_then(|b| b.le_audio_latency)
            .map(LeAudioLatency::key),
    }
    .unwrap_or("default");
    let mut opts = vec![opt(
        "default",
        "Balanced",
        selected == "default",
        Ok(vec![
            "PipeWire's default: it weighs latency against reliability itself.".into(),
        ]),
    )];
    for l in LeAudioLatency::ALL {
        let (label, tip) = match l {
            LeAudioLatency::Low => ("Low latency", "Less delay, for games and video."),
            LeAudioLatency::Reliable => ("Reliable", "Fewer dropouts, more delay."),
        };
        opts.push(opt(
            l.key(),
            label,
            selected == l.key(),
            checked(&pc, change(Value::Set(l)), Some(d)).map(|mut w| {
                w.insert(0, tip.to_string());
                w
            }),
        ));
    }
    (opts, String::new())
}

fn capitalise_first(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

/// The Bluetooth tab: one option per mode, and the codecs of the mode in effect (the
/// pending pick, else the profile in use). Every option is a profile name.
fn bluetooth_opts(c: &Ctx, d: &Device, bt: &Bluetooth) -> (Vec<Opt>, Vec<Opt>, String) {
    let pc = c.plan();
    let (live, _) = plan::availability(&pc, &Change::BluetoothProfile(String::new()), Some(d));
    // Read-only mode is handled by the action bar, as on every other page.
    let why = live.err().unwrap_or_default();
    let pending = match c.edits.find("profile", Some(&d.id)) {
        Some(Change::BluetoothProfile(want)) => bluetooth::resolve(bt, want).ok(),
        _ => None,
    };
    let effective = pending.or_else(|| bt.active());
    let selected = |name: &str| effective.is_some_and(|p| p.name == name);
    let check = |name: &str| {
        if !why.is_empty() {
            return Err(String::new());
        }
        checked(&pc, Change::BluetoothProfile(name.to_string()), Some(d))
    };

    let mut modes = Vec::new();
    for m in bt.modes() {
        let Some(p) = bluetooth::profile_for_mode(bt, m) else {
            continue;
        };
        let tip = match m {
            BluetoothMode::HighQuality => "Best sound (A2DP). The microphone is off.",
            BluetoothMode::Headset => "Microphone on, phone-call sound quality (HFP/HSP).",
            _ => "Stays connected but plays nothing.",
        };
        let label = match m {
            BluetoothMode::Headset => "Headset",
            m => m.label(),
        };
        let on = effective.is_some_and(|e| e.mode() == m);
        modes.push(opt(
            &p.name,
            label,
            on,
            check(&p.name).map(|mut w| {
                w.insert(0, tip.to_string());
                w
            }),
        ));
    }

    let mut codecs = Vec::new();
    if let Some(mode) = effective
        .map(|p| p.mode())
        .filter(|m| matches!(m, BluetoothMode::HighQuality | BluetoothMode::Headset))
    {
        for p in bt.profiles.iter().filter(|p| p.mode() == mode) {
            let label = match p.codec() {
                Some(codec) => codec_label(codec),
                None => "Automatic".into(),
            };
            let check = if p.available {
                check(&p.name).map(|mut w| {
                    if p.codec().is_none() {
                        w.insert(
                            0,
                            "Let PipeWire pick the best codec both sides support.".into(),
                        );
                    }
                    w
                })
            } else {
                Err(
                    "Not available: the device or your Bluetooth adapter doesn't support it."
                        .into(),
                )
            };
            codecs.push(opt(&p.name, label, selected(&p.name), check));
        }
    }
    (modes, codecs, why)
}

fn server_name(s: SoundServer) -> &'static str {
    match s {
        SoundServer::PipeWire => "PipeWire",
        SoundServer::PulseAudio => "PulseAudio",
        SoundServer::Jack => "JACK",
        SoundServer::Alsa => "ALSA",
        SoundServer::None => "This system",
    }
}

/// The value of a pending change, for "(not applied yet)" rows.
fn pending_value(c: &Change) -> String {
    let frames = |v: &Value<u32>| match v {
        Value::Set(n) => format!("{n} frames"),
        Value::Default => "Default".into(),
    };
    match c {
        Change::Rate(Value::Set(r)) => format!("{} kHz", khz(*r)),
        Change::Format(Value::Set(f)) => format!("{} ({f})", f.depth_label()),
        Change::PeriodSize(v) | Change::Headroom(v) => frames(v),
        Change::Suspend(Value::Set(false)) => "Kept open".into(),
        Change::Suspend(_) => "Suspends".into(),
        _ => "Default".into(),
    }
}

// ---- global page -------------------------------------------------------------------

/// The allowed-rates list in effect: pending, else saved, else running.
pub fn effective_allowed(c: &Ctx) -> Vec<u32> {
    match c.edits.find("allowed-rates", None) {
        Some(Change::AllowedRates(Value::Set(rs))) => rs.clone(),
        Some(_) => Vec::new(),
        None => c
            .inputs
            .saved
            .global
            .allowed_rates
            .clone()
            .or_else(|| c.clock().and_then(|k| k.allowed_rates.clone()))
            .unwrap_or_default(),
    }
}

/// The quantum the slider shows: pending, else saved, else running.
pub fn effective_quantum(c: &Ctx) -> u32 {
    match c.edits.find("quantum", None) {
        Some(Change::Quantum(Value::Set(q))) => *q,
        _ => c
            .inputs
            .saved
            .global
            .quantum
            .or_else(|| c.clock().and_then(|k| k.quantum))
            .unwrap_or(1024),
    }
}

/// The slider position for a quantum: log2, clamped to 32..8192.
pub fn quantum_index(q: u32) -> f32 {
    (q.max(1) as f32).log2().round().clamp(5.0, 13.0)
}

pub fn clock_view(c: &Ctx) -> ClockView {
    let pc = c.plan();
    let g = &c.inputs.saved.global;
    let clock = c.clock();
    let server = c.server();
    let pipewire = server == SoundServer::PipeWire;
    let avail = |change: Change| plan::availability(&pc, &change, None);
    let either = |change: Change| {
        let (live, save) = avail(change);
        match (live, save) {
            (Ok(()), _) | (_, Ok(())) => Ok(()),
            (Err(_), Err(why)) => Err(why),
        }
    };
    let pending = |name: &str| c.edits.find(name, None);

    let intro = match server {
        SoundServer::PipeWire => {
            "These apply to every device. Apply them live to try them until PipeWire restarts, or save them permanently."
        }
        SoundServer::PulseAudio => {
            "PulseAudio uses one rate and format for every device. Changes are saved in ~/.config/pulse and take effect when PulseAudio restarts."
        }
        SoundServer::Alsa => {
            "No sound server is running, so these are ALSA's defaults for programs that share a device. They're saved in ~/.asoundrc."
        }
        SoundServer::Jack => {
            "A standalone JACK server owns audio. Change its settings with your JACK tool (such as QjackCtl)."
        }
        SoundServer::None => crate::core::model::NO_DEVICES_MESSAGE,
    };

    // Default rate.
    let (rate_opts, rate_why) = match either(Change::Rate(Value::Default)) {
        Ok(()) => {
            let selected = value_str(match pending("rate") {
                Some(Change::Rate(v)) => Some(v),
                _ => None,
            })
            .or(g.rate.map(|r| r.to_string()))
            .unwrap_or_else(|| "default".into());
            let label = match clock.and_then(|k| k.rate) {
                Some(r) if g.rate.is_none() => format!("Default ({})", khz(r)),
                _ => "Default".into(),
            };
            let mut opts = vec![opt("default", label, selected == "default", Ok(vec![]))];
            for r in [44_100, 48_000, 88_200, 96_000, 176_400, 192_000] {
                opts.push(opt(
                    r.to_string(),
                    format!("{} kHz", khz(r)),
                    selected == r.to_string(),
                    checked(&pc, Change::Rate(Value::Set(r)), None),
                ));
            }
            (opts, String::new())
        }
        Err(why) => (Vec::new(), why),
    };

    // Allowed rates.
    let (allowed_opts, allowed_why) = match either(Change::AllowedRates(Value::Default)) {
        Ok(()) => {
            let now = effective_allowed(c);
            let opts = [
                44_100, 48_000, 88_200, 96_000, 176_400, 192_000, 352_800, 384_000,
            ]
            .into_iter()
            .map(|r| {
                let on = now.contains(&r);
                let check = match super::edits::global_change("allowed", &r.to_string(), &now) {
                    Some(ch @ Change::AllowedRates(Value::Set(_))) => checked(&pc, ch, None),
                    _ => Ok(vec![]),
                };
                // Turning a rate off is always possible.
                let check = if on { check.or(Ok(vec![])) } else { check };
                opt(r.to_string(), format!("{} kHz", khz(r)), on, check)
            })
            .collect();
            (opts, String::new())
        }
        Err(why) => (Vec::new(), why),
    };

    // Global format (PulseAudio and ALSA; PipeWire sets it per device).
    let (format_opts, format_why) = if pipewire
        || matches!(server, SoundServer::Jack | SoundServer::None)
    {
        (Vec::new(), String::new())
    } else {
        match either(Change::Format(Value::Default)) {
            Ok(()) => {
                let selected = match pending("format") {
                    Some(Change::Format(Value::Set(f))) => f.alsa_name().to_string(),
                    Some(_) => "default".into(),
                    None => g
                        .format
                        .as_ref()
                        .map(|f| f.alsa_name().to_string())
                        .unwrap_or_else(|| "default".into()),
                };
                let mut opts = vec![opt("default", "Default", selected == "default", Ok(vec![]))];
                for (f, label) in FORMATS {
                    let check = checked(&pc, Change::Format(Value::Set(f.clone())), None);
                    opts.push(opt(f.alsa_name(), label, selected == f.alsa_name(), check));
                }
                (opts, String::new())
            }
            Err(why) => (Vec::new(), why),
        }
    };

    // Quantum. The slider position is set separately: it's two-way bound.
    let quantum_ok = either(Change::Quantum(Value::Default));
    let quantum_label = match (pending("quantum"), g.quantum, clock.and_then(|k| k.quantum)) {
        (Some(Change::Quantum(Value::Set(n))), _, _) => format!("{n} isn't applied yet."),
        (Some(_), _, _) => "Going back to the default isn't applied yet.".into(),
        (None, Some(n), _) => format!("{n} is saved."),
        (None, None, Some(n)) => format!("Running at {n} now."),
        _ => String::new(),
    };
    let rate_hz = match pending("rate") {
        Some(Change::Rate(Value::Set(r))) => Some(*r),
        _ => None,
    }
    .or(g.rate)
    .or(clock.and_then(|k| k.rate))
    .unwrap_or(48_000);
    let frames_opts =
        |name: &str, change: fn(Value<u32>) -> Change, saved: Option<u32>, values: &[u32]| {
            let selected = match pending(name) {
                Some(Change::MinQuantum(v) | Change::MaxQuantum(v)) => value_str(Some(v)),
                _ => None,
            }
            .or(saved.map(|n| n.to_string()))
            .unwrap_or_else(|| "default".into());
            let mut opts = vec![opt("default", "Default", selected == "default", Ok(vec![]))];
            for &n in values {
                opts.push(opt(
                    n.to_string(),
                    n.to_string(),
                    selected == n.to_string(),
                    checked(&pc, change(Value::Set(n)), None),
                ));
            }
            opts
        };
    let (min_opts, max_opts) = if quantum_ok.is_ok() {
        (
            frames_opts(
                "min-quantum",
                Change::MinQuantum,
                g.min_quantum,
                &[16, 32, 64, 128, 256],
            ),
            frames_opts(
                "max-quantum",
                Change::MaxQuantum,
                g.max_quantum,
                &[1024, 2048, 4096, 8192],
            ),
        )
    } else {
        (Vec::new(), Vec::new())
    };

    // Forced values: live only.
    let force_ok = avail(Change::ForceRate(0)).0;
    let (force_rate_opts, force_quantum_opts, force_why) = match &force_ok {
        Ok(()) => {
            let now_rate = match pending("force-rate") {
                Some(Change::ForceRate(n)) => *n,
                _ => clock.and_then(|k| k.force_rate).unwrap_or(0),
            };
            let now_q = match pending("force-quantum") {
                Some(Change::ForceQuantum(n)) => *n,
                _ => clock.and_then(|k| k.force_quantum).unwrap_or(0),
            };
            let mut rates = vec![opt("off", "Off", now_rate == 0, Ok(vec![]))];
            for r in [44_100, 48_000, 88_200, 96_000, 192_000] {
                rates.push(opt(
                    r.to_string(),
                    khz(r),
                    now_rate == r,
                    checked(&pc, Change::ForceRate(r), None),
                ));
            }
            let mut qs = vec![opt("off", "Off", now_q == 0, Ok(vec![]))];
            for n in [64, 128, 256, 512, 1024, 2048] {
                qs.push(opt(
                    n.to_string(),
                    n.to_string(),
                    now_q == n,
                    checked(&pc, Change::ForceQuantum(n), None),
                ));
            }
            (rates, qs, String::new())
        }
        Err(why) => (Vec::new(), Vec::new(), why.clone()),
    };

    // Resampler.
    let (resample_opts, resample_why) = match avail(Change::ResampleQuality(Value::Default)).1 {
        Ok(()) => {
            let selected = match pending("resample-quality") {
                Some(Change::ResampleQuality(v)) => value_str(Some(v)).unwrap_or_default(),
                _ => g
                    .resample_quality
                    .map(|q| q.to_string())
                    .unwrap_or_else(|| "default".into()),
            };
            let mut opts = vec![opt(
                "default",
                "Default",
                selected == "default",
                Ok(vec!["PipeWire's default is 4.".into()]),
            )];
            for q in [0u8, 2, 4, 6, 8, 10, 12, 14] {
                let tip = match q {
                    0 => vec!["Fastest, lowest quality.".to_string()],
                    4 => vec!["PipeWire's default.".to_string()],
                    14 => vec!["Best quality, most CPU.".to_string()],
                    _ => vec![],
                };
                opts.push(opt(
                    q.to_string(),
                    q.to_string(),
                    selected == q.to_string(),
                    Ok(tip),
                ));
            }
            (opts, String::new())
        }
        Err(why) => (Vec::new(), why),
    };

    // Bluetooth headset switching.
    let headset_show = c
        .inputs
        .snapshot
        .devices
        .iter()
        .any(|d| d.bluetooth.is_some())
        || g.headset_autoswitch.is_some();
    let (headset_opts, headset_why) = match avail(Change::HeadsetAutoswitch(Value::Default)) {
        (Err(why), Err(_)) => (Vec::new(), why),
        (live, _) => {
            let selected = match pending("headset-autoswitch") {
                Some(Change::HeadsetAutoswitch(v)) => v.as_option().copied(),
                _ => g.headset_autoswitch,
            };
            let now = c.inputs.snapshot.wireplumber.headset_autoswitch;
            let default_tip = match now {
                Some(true) => "WirePlumber's default, switching now.",
                Some(false) => "WirePlumber's default, not switching now.",
                None => "WirePlumber's default: switch.",
            };
            let opts = vec![
                opt("default", "Default", selected.is_none(), Ok(vec![default_tip.into()])),
                opt(
                    "on",
                    "Switch",
                    selected == Some(true),
                    checked(&pc, Change::HeadsetAutoswitch(Value::Set(true)), None).map(|mut w| {
                        w.insert(0, "Calls can use the headset's microphone.".into());
                        w
                    }),
                ),
                opt(
                    "off",
                    "Stay in high quality",
                    selected == Some(false),
                    checked(&pc, Change::HeadsetAutoswitch(Value::Set(false)), None).map(
                        |mut w| {
                            w.insert(0, "Sound stays at full quality; switch to headset mode yourself for calls.".into());
                            w
                        },
                    ),
                ),
            ];
            let why = match live {
                Ok(()) => String::new(),
                Err(why) => format!("Saved only: {why}."),
            };
            (opts, why)
        }
    };

    // What's saved.
    let mut saved = Vec::new();
    let kv = |key: &str, value: String| KV {
        key: s(key),
        value: s(value),
        source: SharedString::new(),
    };
    if let Some(r) = g.rate {
        saved.push(kv("Rate", format!("{} kHz", khz(r))));
    }
    if let Some(rs) = &g.allowed_rates {
        saved.push(kv("Allowed", format!("{} kHz", rate_list(rs))));
    }
    if let Some(f) = &g.format {
        saved.push(kv("Format", format!("{} ({f})", f.depth_label())));
    }
    for (key, v) in [
        ("Quantum", g.quantum),
        ("Smallest", g.min_quantum),
        ("Largest", g.max_quantum),
    ] {
        if let Some(n) = v {
            saved.push(kv(key, format!("{n} frames")));
        }
    }
    if let Some(q) = g.resample_quality {
        saved.push(kv("Resampler", format!("quality {q}")));
    }
    for d in c.inputs.saved.devices.values() {
        let mut parts = Vec::new();
        if let Some(r) = d.rate {
            parts.push(format!("{} kHz", khz(r)));
        }
        if let Some(f) = &d.format {
            parts.push(f.alsa_name().to_string());
        }
        if let Some(n) = d.period_size {
            parts.push(format!("period {n}"));
        }
        if let Some(n) = d.headroom {
            parts.push(format!("headroom {n}"));
        }
        if d.disable_suspend {
            parts.push("kept open when idle".into());
        }
        saved.push(kv(&d.description, parts.join(" · ")));
    }
    if let Some(on) = g.headset_autoswitch {
        saved.push(kv(
            "Headsets",
            if on {
                "switch for calls"
            } else {
                "stay in high quality"
            }
            .into(),
        ));
    }
    for b in c.inputs.saved.bluetooth.values() {
        if let Some(q) = b.ldac_quality {
            saved.push(kv(&b.description, format!("LDAC {}", q.label())));
        }
        if let Some(l) = b.le_audio_latency {
            saved.push(kv(&b.description, format!("LE Audio {}", l.label())));
        }
    }
    let can_reset = c.read_only.is_none() && plan::plan_reset(&pc).is_ok_and(|p| !p.is_empty());

    ClockView {
        server: s(server_name(server)),
        intro: s(intro),
        rate_opts: model(rate_opts),
        rate_why: s(rate_why),
        allowed_opts: model(allowed_opts),
        allowed_why: s(allowed_why),
        format_opts: model(format_opts),
        format_why: s(format_why),
        quantum_show: quantum_ok.is_ok(),
        quantum_label: s(quantum_label),
        quantum_why: s(quantum_ok.err().unwrap_or_default()),
        rate_hz: rate_hz as i32,
        min_opts: model(min_opts),
        max_opts: model(max_opts),
        force_rate_opts: model(force_rate_opts),
        force_quantum_opts: model(force_quantum_opts),
        force_why: s(force_why),
        resample_opts: model(resample_opts),
        resample_why: s(resample_why),
        headset_show,
        headset_opts: model(headset_opts),
        headset_why: s(headset_why),
        saved: model(saved),
        can_reset,
    }
}

// ---- action bar --------------------------------------------------------------------

pub struct Pending {
    pub count: usize,
    pub summary: String,
    pub can_live: bool,
    pub can_save: bool,
    /// Why neither button works, or what's in the way.
    pub why: String,
}

/// The edits with their devices looked up. Edits for devices that went away are dropped.
pub fn resolved<'a>(c: &'a Ctx) -> Vec<(Change, Option<&'a Device>)> {
    c.edits
        .list()
        .iter()
        .filter_map(|e| match &e.device {
            None => Some((e.change.clone(), None)),
            Some(id) => c
                .inputs
                .snapshot
                .devices
                .iter()
                .find(|d| d.id == *id)
                .map(|d| (e.change.clone(), Some(d))),
        })
        .collect()
}

pub fn pending(c: &Ctx) -> Pending {
    let items = resolved(c);
    let pc = c.plan();
    let summary = items
        .iter()
        .map(|(ch, d)| ch.describe(*d))
        .collect::<Vec<_>>()
        .join("; ");
    let mut live_err = None;
    let mut save_err = None;
    for (ch, d) in &items {
        let (live, save) = plan::availability(&pc, ch, *d);
        if let (Err(e), None) = (live, &live_err) {
            live_err = Some(format!("{}: {e}", capitalise(&ch.describe(*d))));
        }
        if let (Err(e), None) = (save, &save_err) {
            save_err = Some(e);
        }
    }
    let any = !items.is_empty();
    let why = match (c.read_only, &live_err, &save_err) {
        _ if !any => String::new(),
        (Some(r), _, _) => r.to_string(),
        (None, Some(_), Some(e)) => e.clone(),
        _ => String::new(),
    };
    Pending {
        count: items.len(),
        summary: capitalise(&summary),
        can_live: any && c.read_only.is_none() && live_err.is_none(),
        can_save: any && c.read_only.is_none() && save_err.is_none(),
        why,
    }
}

fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

/// Notices across the top of the window.
pub fn banner(c: &Ctx, fixture: Option<&std::path::Path>, errors: &[String]) -> Vec<Fact> {
    let mut out = Vec::new();
    let mut push = |level: &str, text: String| {
        out.push(Fact {
            text: s(text),
            level: s(level),
        })
    };
    // The fixture note and why nothing can be applied go on one line.
    let mut first = fixture
        .map(|dir| {
            format!(
                "Showing a captured system ({}), not this computer.",
                dir.display()
            )
        })
        .into_iter()
        .collect::<Vec<_>>();
    if c.inputs.sandbox {
        first.push(crate::app::sandbox_note(&c.inputs.paths));
    } else if let Some(r) = c.read_only {
        first.push(r.to_string());
    }
    if !first.is_empty() {
        push("info", first.join(" "));
    }
    if let Some(e) = &c.inputs.saved_error {
        push("warn", e.clone());
    }
    if c.inputs.pulse_override && c.server() == SoundServer::PulseAudio {
        push("warn", "PULSE_CONFIG is set, so PulseAudio reads its settings from somewhere else and Ratewise can't change them.".into());
    }
    for e in errors {
        push("warn", e.clone());
    }
    out
}

// ---- review sheet ------------------------------------------------------------------

/// Splits a plan, as `render_plan_window` writes it, into lines the sheet can style:
/// headings, numbered steps, their reasons, diff lines and notes. Anything else (a
/// refusal, an error) is plain text.
pub fn review_lines(body: &str, plan: bool) -> Vec<Line> {
    let line = |kind: &str, text: &str| Line {
        kind: s(kind),
        text: s(text),
    };
    if !plan {
        return body
            .split('\n')
            .map(|l| line(if l.is_empty() { "gap" } else { "text" }, l))
            .collect();
    }
    let mut out: Vec<Line> = Vec::new();
    for l in body.lines() {
        let kind = if l.trim().is_empty() {
            "gap"
        } else if let Some(rest) = l.strip_prefix("       ") {
            if rest.starts_with("@@") {
                "hunk"
            } else if rest.starts_with('+') {
                "add"
            } else if rest.starts_with('-') {
                "del"
            } else {
                "ctx"
            }
        } else if l.starts_with("     ") {
            "why"
        } else if l.starts_with("  - ") {
            "note"
        } else if l
            .strip_prefix("  ")
            .and_then(|r| r.split_once(". "))
            .is_some_and(|(n, _)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
        {
            "step"
        } else if !l.contains(' ') {
            // "Steps", "Notes", "Afterwards".
            "head"
        } else {
            "text"
        };
        // Leading blank lines add nothing; the sheet has its own padding.
        if kind == "gap" && out.is_empty() {
            continue;
        }
        let text = match kind {
            "add" | "del" | "ctx" | "hunk" => &l[7..],
            "note" => l.trim_start().trim_start_matches("- "),
            "head" | "gap" => l,
            _ => l.trim_start(),
        };
        out.push(line(kind, text));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::paths::Paths;
    use crate::core::settings::SavedConfig;
    use crate::probe::Probes;
    use slint::Model;

    fn inputs(fixture: &str) -> Inputs {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(fixture);
        Inputs {
            snapshot: Probes::fixture(&dir).snapshot(),
            paths: Paths::new("/home/user".into(), None, None),
            saved: SavedConfig::default(),
            saved_error: None,
            files: Default::default(),
            system_pulse: None,
            tools: vec!["pw-metadata".into(), "systemctl".into()],
            pulse_override: false,
            sandbox: false,
            latest_backup: None,
        }
    }

    fn labels(m: &ModelRc<Opt>) -> Vec<(String, bool, bool)> {
        m.iter()
            .map(|o| (o.label.to_string(), o.enabled, o.selected))
            .collect()
    }

    #[test]
    fn review_lines_follow_the_plan_layout() {
        let body = "\nSteps\n  1. Update ~/.config/a.conf\n     Sets the rate.\n       @@ -1,2 +1,2 @@\n        keep\n       -old\n       +new\n\nNotes\n  - It applies to every device.\n\nAfterwards\n  Restart PipeWire.\n\nRatewise backs up every file first.\n";
        let got: Vec<(String, String)> = review_lines(body, true)
            .into_iter()
            .map(|l| (l.kind.to_string(), l.text.to_string()))
            .collect();
        let want = [
            ("head", "Steps"),
            ("step", "1. Update ~/.config/a.conf"),
            ("why", "Sets the rate."),
            ("hunk", "@@ -1,2 +1,2 @@"),
            ("ctx", " keep"),
            ("del", "-old"),
            ("add", "+new"),
            ("gap", ""),
            ("head", "Notes"),
            ("note", "It applies to every device."),
            ("gap", ""),
            ("head", "Afterwards"),
            ("text", "Restart PipeWire."),
            ("gap", ""),
            ("text", "Ratewise backs up every file first."),
        ];
        let want: Vec<(String, String)> = want
            .iter()
            .map(|(k, t)| (k.to_string(), t.to_string()))
            .collect();
        assert_eq!(got, want);
        let refused = review_lines("No.\n\nTry again.", false);
        assert_eq!(refused.len(), 3);
        assert!(refused.iter().all(|l| l.kind == "text" || l.kind == "gap"));
    }

    #[test]
    fn sidebar_groups_outputs_inputs_and_pages() {
        let i = inputs("pw-1.2-desktop");
        let nav = nav(&i);
        let kinds: Vec<String> = nav
            .iter()
            .map(|n| format!("{}:{}", n.kind, n.title))
            .collect();
        assert_eq!(kinds[0], "header:OUTPUTS");
        assert!(kinds.contains(&"header:INPUTS".to_string()));
        assert_eq!(kinds.last().unwrap(), "page:About this system");
        let d50 = nav
            .iter()
            .find(|n| n.title == "D50s Analog Stereo")
            .unwrap();
        assert_eq!(d50.badge, "96 kHz · 32-bit");
        assert_eq!(d50.icon, "usb");
        assert_eq!(d50.tag, "Default");
        assert!(d50.active);
    }

    #[test]
    fn device_options_grey_out_what_the_hardware_cant_do() {
        let i = inputs("pw-1.2-desktop");
        let edits = Edits::default();
        let c = Ctx {
            inputs: &i,
            edits: &edits,
            read_only: None,
        };
        let d = i
            .snapshot
            .devices
            .iter()
            .find(|d| d.id.contains("D50s"))
            .unwrap();
        let v = device_view(&c, d);
        let rates = labels(&v.rate_opts);
        assert_eq!(rates[0], ("Default".into(), true, true));
        assert!(rates.contains(&("96".into(), true, false)));
        assert!(
            rates.contains(&("768".into(), true, false)),
            "the D50s goes to 768 kHz"
        );
        let formats = labels(&v.format_opts);
        assert!(
            formats.contains(&("16-bit".into(), false, false)),
            "{formats:?}"
        );
        assert!(formats.contains(&("32-bit".into(), true, false)));
        let tip = v
            .format_opts
            .iter()
            .find(|o| o.label == "16-bit")
            .unwrap()
            .tip;
        assert!(tip.contains("doesn't support S16_LE"), "{tip}");
        assert_eq!(v.running_main, "96 kHz · 32-bit");
        assert_eq!(v.state, "playing");
        let stats: Vec<String> = v
            .readout
            .iter()
            .map(|s| format!("{} {}", s.value, s.unit))
            .collect();
        assert_eq!(stats[..2], ["96 kHz", "32 bit"]);
        // The grid marks the running rate in the 48 kHz row's ×2 column.
        let row = v.rate_rows.row_data(1).unwrap();
        assert_eq!(row.label, "48 kHz");
        let chip = row.chips.row_data(1).unwrap();
        assert!(chip.on && chip.current, "{}", chip.label);
        assert!(!v.rate_source.is_empty());

        let bt = i
            .snapshot
            .devices
            .iter()
            .find(|d| d.kind == DeviceKind::Bluetooth)
            .unwrap();
        let v = device_view(&c, bt);
        assert!(v.edit_why.contains("Bluetooth"), "{}", v.edit_why);
        assert_eq!(v.rate_opts.row_count(), 0);
        // AAC headphones: no LE Audio, so no latency row at all.
        assert_eq!(v.le_latency_opts.row_count(), 0);
        assert_eq!(v.le_latency_why, "");
    }

    #[test]
    fn le_audio_devices_offer_a_latency_choice() {
        let i = inputs("pw-1.6-le-audio");
        let mut edits = Edits::default();
        let bt = i
            .snapshot
            .devices
            .iter()
            .find(|d| d.id == "bluez_output.00_00_00_00_00_01.1")
            .unwrap();
        let c = Ctx {
            inputs: &i,
            edits: &edits,
            read_only: None,
        };
        let v = device_view(&c, bt);
        assert_eq!(v.le_latency_why, "");
        assert_eq!(
            labels(&v.le_latency_opts),
            [
                ("Balanced".into(), true, true),
                ("Low latency".into(), true, false),
                ("Reliable".into(), true, false),
            ]
        );

        edits.set(
            super::super::edits::Edit {
                change: Change::LeAudioLatency(Value::Set(LeAudioLatency::Low)),
                device: Some(bt.id.clone()),
            },
            &i.saved,
            None,
            Some(bt),
        );
        let c = Ctx {
            inputs: &i,
            edits: &edits,
            read_only: None,
        };
        let v = device_view(&c, bt);
        let picked: Vec<_> = labels(&v.le_latency_opts)
            .into_iter()
            .filter(|o| o.2)
            .map(|o| o.0)
            .collect();
        assert_eq!(picked, ["Low latency"]);
    }

    #[test]
    fn pulseaudio_devices_point_to_the_global_page() {
        let i = inputs("pulseaudio-only");
        let edits = Edits::default();
        let c = Ctx {
            inputs: &i,
            edits: &edits,
            read_only: None,
        };
        let v = device_view(&c, &i.snapshot.devices[0]);
        assert!(v.rate_why.contains("Global settings"), "{}", v.rate_why);
        let g = clock_view(&c);
        assert!(g.format_opts.row_count() > 0);
        assert!(!g.quantum_show);
        assert!(g.quantum_why.contains("per program"), "{}", g.quantum_why);
    }

    #[test]
    fn pending_edits_decide_the_buttons() {
        let i = inputs("pw-1.2-desktop");
        let mut edits = Edits::default();
        let clock = i.snapshot.clock.as_ref().map(|c| &c.value);
        edits.set(
            super::super::edits::Edit {
                change: Change::Quantum(Value::Set(256)),
                device: None,
            },
            &i.saved,
            clock,
            None,
        );
        let c = Ctx {
            inputs: &i,
            edits: &edits,
            read_only: None,
        };
        let p = pending(&c);
        assert!(p.can_live && p.can_save, "{}", p.why);

        let d50 = i
            .snapshot
            .devices
            .iter()
            .find(|d| d.id.contains("D50s"))
            .unwrap();
        edits.set(
            super::super::edits::Edit {
                change: Change::Suspend(Value::Set(false)),
                device: Some(d50.id.clone()),
            },
            &i.saved,
            clock,
            Some(d50),
        );
        let c = Ctx {
            inputs: &i,
            edits: &edits,
            read_only: None,
        };
        let p = pending(&c);
        assert_eq!(p.count, 2);
        assert!(!p.can_live && p.can_save);

        let c = Ctx {
            inputs: &i,
            edits: &edits,
            read_only: Some("read only"),
        };
        let p = pending(&c);
        assert!(!p.can_save && p.why == "read only");
    }
}
