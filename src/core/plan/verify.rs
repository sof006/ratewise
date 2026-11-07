//! After applying: what was requested vs. what the system now reports (spec §4).

use std::fmt::Write;

use crate::core::model::{ClockSettings, Running, SampleFormat, Snapshot, khz};
use crate::core::settings::rate_list;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClockKey {
    Rate,
    AllowedRates,
    Quantum,
    MinQuantum,
    MaxQuantum,
    ForceRate,
    ForceQuantum,
}

impl ClockKey {
    /// The `settings` metadata key.
    pub fn metadata_key(self) -> &'static str {
        match self {
            Self::Rate => "clock.rate",
            Self::AllowedRates => "clock.allowed-rates",
            Self::Quantum => "clock.quantum",
            Self::MinQuantum => "clock.min-quantum",
            Self::MaxQuantum => "clock.max-quantum",
            Self::ForceRate => "clock.force-rate",
            Self::ForceQuantum => "clock.force-quantum",
        }
    }

    fn get(self, c: &ClockSettings) -> Option<ClockValue> {
        let n = |v: Option<u32>| v.map(ClockValue::Number);
        match self {
            Self::Rate => n(c.rate),
            Self::AllowedRates => c.allowed_rates.clone().map(ClockValue::Rates),
            Self::Quantum => n(c.quantum),
            Self::MinQuantum => n(c.min_quantum),
            Self::MaxQuantum => n(c.max_quantum),
            // Unset forces read as 0, which is what "released" means.
            Self::ForceRate => Some(ClockValue::Number(c.force_rate.unwrap_or(0))),
            Self::ForceQuantum => Some(ClockValue::Number(c.force_quantum.unwrap_or(0))),
        }
    }

    fn is_rate(self) -> bool {
        matches!(self, Self::Rate | Self::ForceRate)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClockValue {
    Number(u32),
    Rates(Vec<u32>),
}

impl ClockValue {
    /// The value as `pw-metadata` takes it.
    pub fn metadata_text(&self) -> String {
        match self {
            Self::Number(n) => n.to_string(),
            Self::Rates(rs) => {
                let items: Vec<String> = rs.iter().map(u32::to_string).collect();
                format!("[ {} ]", items.join(" "))
            }
        }
    }

    fn display(&self, key: ClockKey) -> String {
        match self {
            Self::Number(0) if matches!(key, ClockKey::ForceRate | ClockKey::ForceQuantum) => {
                "not forced".into()
            }
            Self::Number(n) if key.is_rate() => format!("{} kHz", khz(*n)),
            Self::Number(n) => format!("{n} frames"),
            Self::Rates(rs) => format!("{} kHz", rate_list(rs)),
        }
    }
}

/// Something the plan expects to be true afterwards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expect {
    Clock {
        key: ClockKey,
        want: ClockValue,
    },
    DeviceRate {
        id: String,
        description: String,
        rate: u32,
    },
    DeviceFormat {
        id: String,
        description: String,
        format: SampleFormat,
    },
    /// A Bluetooth card's active profile.
    BluetoothProfile {
        /// The card's `device.name`.
        card: String,
        description: String,
        profile: String,
        /// How to show the profile, e.g. "High quality (LDAC)".
        label: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Ok,
    Differs(String),
    /// Can't be checked yet, e.g. the device is idle.
    Waiting(String),
    Unknown(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub what: String,
    pub want: String,
    pub got: Option<String>,
    pub outcome: Outcome,
}

pub fn verify(expect: &[Expect], snap: &Snapshot) -> Vec<Check> {
    expect.iter().map(|e| check(e, snap)).collect()
}

fn check(e: &Expect, snap: &Snapshot) -> Check {
    match e {
        Expect::Clock { key, want } => {
            let what = format!("PipeWire {}", key.metadata_key());
            let want_text = want.display(*key);
            let Some(clock) = &snap.clock else {
                return Check {
                    what,
                    want: want_text,
                    got: None,
                    outcome: Outcome::Unknown("PipeWire's settings couldn't be read".into()),
                };
            };
            let got = key.get(&clock.value);
            let outcome = match &got {
                Some(g) if g == want => Outcome::Ok,
                Some(_) => Outcome::Differs(match key {
                    ClockKey::Rate if clock.value.force_rate.is_some_and(|r| r > 0) => {
                        "a forced rate (clock.force-rate) is active and overrides it".into()
                    }
                    _ => "another config file may set it after Ratewise's: look in \
                          ~/.config/pipewire/pipewire.conf.d/ for files sorting after 60-ratewise.conf"
                        .into(),
                }),
                None => Outcome::Unknown("PipeWire doesn't report this setting".into()),
            };
            Check {
                what,
                want: want_text,
                got: got.map(|g| g.display(*key)),
                outcome,
            }
        }
        Expect::DeviceRate {
            id,
            description,
            rate,
        } => device_check(
            snap,
            id,
            format!("{description} rate"),
            format!("{} kHz", khz(*rate)),
            |p| (p.rate == *rate, format!("{} kHz", khz(p.rate))),
        ),
        Expect::DeviceFormat {
            id,
            description,
            format,
        } => device_check(
            snap,
            id,
            format!("{description} format"),
            format.to_string(),
            |p| match &p.format {
                Some(f) => (f == format, f.to_string()),
                None => (false, "unknown".into()),
            },
        ),
        Expect::BluetoothProfile {
            card,
            description,
            profile,
            label,
        } => {
            let what = format!("{description} profile");
            let bt = snap
                .devices
                .iter()
                .filter_map(|d| d.bluetooth.as_ref())
                .find(|b| b.card_name == *card);
            let (got, outcome) = match bt {
                None => (
                    None,
                    Outcome::Unknown("the device isn't connected now".into()),
                ),
                Some(b) => {
                    let got = b
                        .active()
                        .map(crate::core::bluetooth::profile_label)
                        .or_else(|| b.profile.clone());
                    if b.profile.as_deref() == Some(profile.as_str()) {
                        (got, Outcome::Ok)
                    } else {
                        (
                            got,
                            Outcome::Differs(
                                "the headset or the Bluetooth adapter refused it, or WirePlumber \
                                 switched back (for example when a program opened the microphone)"
                                    .into(),
                            ),
                        )
                    }
                }
            };
            Check {
                what,
                want: label.clone(),
                got,
                outcome,
            }
        }
    }
}

fn device_check(
    snap: &Snapshot,
    id: &str,
    what: String,
    want: String,
    test: impl Fn(&crate::core::model::StreamParams) -> (bool, String),
) -> Check {
    let (got, outcome) = match snap.devices.iter().find(|d| d.id == id) {
        None => (
            None,
            Outcome::Unknown("the device isn't present now".into()),
        ),
        Some(d) => match d.running.as_ref().map(|r| &r.value) {
            None => (
                None,
                Outcome::Unknown("its running state can't be read".into()),
            ),
            Some(Running::Idle) => (
                Some("idle".into()),
                Outcome::Waiting("the device is idle; this is set when playback starts".into()),
            ),
            Some(Running::Active(p)) => {
                let (ok, got) = test(p);
                let outcome = if ok {
                    Outcome::Ok
                } else {
                    Outcome::Differs(
                        "programs that were already playing keep their settings until they stop and start again"
                            .into(),
                    )
                };
                (Some(got), outcome)
            }
        },
    };
    Check {
        what,
        want,
        got,
        outcome,
    }
}

pub fn render_checks(checks: &[Check]) -> String {
    let mut s = String::new();
    for c in checks {
        let got = c.got.as_deref().unwrap_or("unknown");
        let _ = match &c.outcome {
            Outcome::Ok => writeln!(s, "  ✓ {}: {}", c.what, c.want),
            Outcome::Differs(why) => writeln!(
                s,
                "  ✗ {}: asked for {}, but it reports {got}. Likely reason: {why}.",
                c.what, c.want
            ),
            Outcome::Waiting(why) => writeln!(s, "  … {}: {} requested; {why}.", c.what, c.want),
            Outcome::Unknown(why) => {
                writeln!(
                    s,
                    "  ? {}: {} requested; can't confirm: {why}.",
                    c.what, c.want
                )
            }
        };
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::{
        Capabilities, Device, DeviceKind, Direction, Source, Sourced, StreamParams,
    };

    fn snap(clock: ClockSettings, running: Running) -> Snapshot {
        Snapshot {
            schema_version: 1,
            devices: vec![Device {
                id: "dac".into(),
                description: "DAC".into(),
                direction: Direction::Playback,
                kind: DeviceKind::Usb,
                alsa: None,
                pipewire: None,
                capabilities: Capabilities::default(),
                running: Some(Sourced::new(running, Source::Procfs)),
                kernel_probe: None,
                bluetooth: None,
            }],
            clock: Some(Sourced::new(clock, Source::PipeWire)),
            pipewire_version: None,
            stack: crate::core::stack::detect(None, &Default::default(), false),
            wireplumber: Default::default(),
            notes: vec![],
        }
    }

    fn active(rate: u32) -> Running {
        Running::Active(StreamParams {
            rate,
            format: Some(SampleFormat::S32Le),
            channels: Some(2),
            period_frames: None,
            buffer_frames: None,
        })
    }

    #[test]
    fn clock_values_match_or_explain() {
        let s = snap(
            ClockSettings {
                rate: Some(48_000),
                force_rate: Some(96_000),
                ..Default::default()
            },
            Running::Idle,
        );
        let checks = verify(
            &[
                Expect::Clock {
                    key: ClockKey::ForceRate,
                    want: ClockValue::Number(96_000),
                },
                Expect::Clock {
                    key: ClockKey::Rate,
                    want: ClockValue::Number(96_000),
                },
                Expect::Clock {
                    key: ClockKey::ForceQuantum,
                    want: ClockValue::Number(0),
                },
            ],
            &s,
        );
        assert_eq!(checks[0].outcome, Outcome::Ok);
        assert!(matches!(&checks[1].outcome, Outcome::Differs(w) if w.contains("forced")));
        assert_eq!(checks[2].outcome, Outcome::Ok, "unset force means released");
        let text = render_checks(&checks);
        assert!(
            text.contains("✓ PipeWire clock.force-rate: 96 kHz"),
            "{text}"
        );
        assert!(
            text.contains("asked for 96 kHz, but it reports 48 kHz"),
            "{text}"
        );
    }

    #[test]
    fn idle_devices_wait_and_running_ones_are_compared() {
        let e = [Expect::DeviceRate {
            id: "dac".into(),
            description: "DAC".into(),
            rate: 96_000,
        }];
        let idle = verify(&e, &snap(ClockSettings::default(), Running::Idle));
        assert!(matches!(idle[0].outcome, Outcome::Waiting(_)));
        assert_eq!(
            verify(&e, &snap(ClockSettings::default(), active(96_000)))[0].outcome,
            Outcome::Ok
        );
        let wrong = verify(&e, &snap(ClockSettings::default(), active(48_000)));
        assert_eq!(wrong[0].got.as_deref(), Some("48 kHz"));
        assert!(matches!(wrong[0].outcome, Outcome::Differs(_)));
        let missing = verify(
            &[Expect::DeviceFormat {
                id: "gone".into(),
                description: "X".into(),
                format: SampleFormat::S16Le,
            }],
            &snap(ClockSettings::default(), Running::Idle),
        );
        assert!(matches!(missing[0].outcome, Outcome::Unknown(_)));
    }

    #[test]
    fn metadata_text_for_pw_metadata() {
        assert_eq!(
            ClockValue::Rates(vec![44_100, 48_000]).metadata_text(),
            "[ 44100 48000 ]"
        );
        assert_eq!(ClockValue::Number(256).metadata_text(), "256");
    }
}
