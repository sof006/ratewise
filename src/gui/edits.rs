//! Changes the user has picked but not applied yet. Pure.
//!
//! The window collects edits; "Apply live" or "Save permanently" turns them into
//! one plan. Picking a value that matches what's already in effect drops the edit.

use crate::core::model::{ClockSettings, Device, SampleFormat};
use crate::core::settings::{Change, LdacQuality, LeAudioLatency, LeAudioQos, SavedConfig, Value};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub change: Change,
    /// Device id, for per-device settings.
    pub device: Option<String>,
}

impl Edit {
    fn same_slot(&self, other: &Edit) -> bool {
        self.change.name() == other.change.name() && self.device == other.device
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Edits {
    list: Vec<Edit>,
}

impl Edits {
    pub fn list(&self) -> &[Edit] {
        &self.list
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    pub fn clear(&mut self) {
        self.list.clear();
    }

    /// Adds `edit`, replacing an earlier edit of the same setting. An edit that changes
    /// nothing (same as saved and as running) is dropped instead.
    pub fn set(
        &mut self,
        edit: Edit,
        saved: &SavedConfig,
        clock: Option<&ClockSettings>,
        device: Option<&Device>,
    ) {
        self.list.retain(|e| !e.same_slot(&edit));
        if !is_noop(&edit.change, saved, clock, device) {
            self.list.push(edit);
        }
    }

    /// The pending edit for this setting, if any.
    pub fn find(&self, name: &str, device: Option<&str>) -> Option<&Change> {
        self.list
            .iter()
            .find(|e| e.change.name() == name && e.device.as_deref() == device)
            .map(|e| &e.change)
    }
}

/// Whether `change` would leave things as they are: it matches what's saved, or
/// nothing is saved for that setting and it matches what's running.
fn is_noop(
    change: &Change,
    saved: &SavedConfig,
    clock: Option<&ClockSettings>,
    device: Option<&Device>,
) -> bool {
    let g = &saved.global;
    let running = |saved_here: bool, now: Option<u32>, n: u32| !saved_here && now == Some(n);
    let c = |f: fn(&ClockSettings) -> Option<u32>| clock.and_then(f);
    match change {
        Change::ForceRate(n) => clock.and_then(|c| c.force_rate).unwrap_or(0) == *n,
        Change::ForceQuantum(n) => clock.and_then(|c| c.force_quantum).unwrap_or(0) == *n,
        // Never saved, so compare with the profile in use.
        Change::BluetoothProfile(want) => device
            .and_then(|d| d.bluetooth.as_ref())
            .and_then(|bt| {
                let p = crate::core::bluetooth::resolve(bt, want).ok()?;
                Some(bt.profile.as_deref() == Some(p.name.as_str()))
            })
            .unwrap_or(false),
        _ if saved.with(change, device) == *saved => true,
        Change::Rate(Value::Set(n)) if device.is_none() => {
            running(g.rate.is_some(), c(|c| c.rate), *n)
        }
        Change::Quantum(Value::Set(n)) => running(g.quantum.is_some(), c(|c| c.quantum), *n),
        Change::MinQuantum(Value::Set(n)) => {
            running(g.min_quantum.is_some(), c(|c| c.min_quantum), *n)
        }
        Change::MaxQuantum(Value::Set(n)) => {
            running(g.max_quantum.is_some(), c(|c| c.max_quantum), *n)
        }
        Change::AllowedRates(Value::Set(rs)) => {
            let mut rs = rs.clone();
            rs.sort_unstable();
            rs.dedup();
            g.allowed_rates.is_none() && clock.and_then(|c| c.allowed_rates.as_ref()) == Some(&rs)
        }
        _ => false,
    }
}

fn number(value: &str) -> Option<Value<u32>> {
    match value {
        "default" => Some(Value::Default),
        v => v.parse().ok().map(Value::Set),
    }
}

/// A per-device option picked in the window: `kind` is "rate", "format", "period",
/// "headroom", "suspend" or "profile" (a Bluetooth profile name).
pub fn device_change(kind: &str, value: &str) -> Option<Change> {
    Some(match kind {
        "profile" if !value.is_empty() => Change::BluetoothProfile(value.to_string()),
        "rate" => Change::Rate(number(value)?),
        "format" => Change::Format(match value {
            "default" => Value::Default,
            v => Value::Set(SampleFormat::from_alsa(v)),
        }),
        "period" => Change::PeriodSize(number(value)?),
        "headroom" => Change::Headroom(number(value)?),
        "suspend" => Change::Suspend(match value {
            "off" => Value::Set(false),
            _ => Value::Default,
        }),
        "ldac" => Change::LdacQuality(match value {
            "default" => Value::Default,
            v => Value::Set(LdacQuality::parse(v)?),
        }),
        "le-latency" => Change::LeAudioLatency(match value {
            "default" => Value::Default,
            v => Value::Set(LeAudioLatency::parse(v)?),
        }),
        k if k.starts_with("le-audio-") => Change::LeAudioQos(LeAudioQos::parse(k, value).ok()?),
        _ => return None,
    })
}

/// A global option picked in the window. `allowed` is the allowed-rates list in effect
/// now, which a toggle edits.
pub fn global_change(kind: &str, value: &str, allowed: &[u32]) -> Option<Change> {
    let off = |v: &str| match v {
        "off" => Some(0),
        v => v.parse().ok(),
    };
    Some(match kind {
        "rate" => Change::Rate(number(value)?),
        "allowed" => {
            let rate: u32 = value.parse().ok()?;
            let mut list: Vec<u32> = allowed.to_vec();
            match list.iter().position(|&r| r == rate) {
                Some(i) => {
                    list.remove(i);
                }
                None => list.push(rate),
            }
            list.sort_unstable();
            if list.is_empty() {
                Change::AllowedRates(Value::Default)
            } else {
                Change::AllowedRates(Value::Set(list))
            }
        }
        "quantum" => Change::Quantum(number(value)?),
        "min-quantum" => Change::MinQuantum(number(value)?),
        "max-quantum" => Change::MaxQuantum(number(value)?),
        "force-rate" => Change::ForceRate(off(value)?),
        "force-quantum" => Change::ForceQuantum(off(value)?),
        "resample" => Change::ResampleQuality(match value {
            "default" => Value::Default,
            v => Value::Set(v.parse().ok()?),
        }),
        "format" => Change::Format(match value {
            "default" => Value::Default,
            v => Value::Set(SampleFormat::from_alsa(v)),
        }),
        "headset-autoswitch" => Change::HeadsetAutoswitch(match value {
            "on" => Value::Set(true),
            "off" => Value::Set(false),
            "default" => Value::Default,
            _ => return None,
        }),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clock() -> ClockSettings {
        ClockSettings {
            rate: Some(48_000),
            allowed_rates: Some(vec![44_100, 48_000]),
            quantum: Some(1024),
            min_quantum: Some(32),
            max_quantum: Some(2048),
            force_rate: None,
            force_quantum: None,
        }
    }

    #[test]
    fn a_later_edit_replaces_an_earlier_one() {
        let saved = SavedConfig::default();
        let mut e = Edits::default();
        let c = clock();
        for q in [256, 512] {
            e.set(
                Edit {
                    change: Change::Quantum(Value::Set(q)),
                    device: None,
                },
                &saved,
                Some(&c),
                None,
            );
        }
        assert_eq!(e.list().len(), 1);
        assert_eq!(
            e.find("quantum", None),
            Some(&Change::Quantum(Value::Set(512)))
        );
    }

    #[test]
    fn picking_what_is_already_in_effect_drops_the_edit() {
        let saved = SavedConfig::default();
        let c = clock();
        let mut e = Edits::default();
        let q = |n| Edit {
            change: Change::Quantum(Value::Set(n)),
            device: None,
        };
        e.set(q(256), &saved, Some(&c), None);
        e.set(q(1024), &saved, Some(&c), None);
        assert!(e.is_empty(), "1024 is running and nothing is saved");

        // Once 256 is saved, picking 1024 is a real change again.
        let saved = saved.with(&Change::Quantum(Value::Set(256)), None);
        e.set(q(1024), &saved, Some(&c), None);
        assert_eq!(e.list().len(), 1);

        e.set(
            Edit {
                change: Change::ForceRate(0),
                device: None,
            },
            &saved,
            Some(&c),
            None,
        );
        assert_eq!(e.list().len(), 1, "force-rate 0 when nothing is forced");
    }

    #[test]
    fn options_parse_into_changes() {
        assert_eq!(
            device_change("rate", "96000"),
            Some(Change::Rate(Value::Set(96_000)))
        );
        assert_eq!(
            device_change("format", "S24_3LE"),
            Some(Change::Format(Value::Set(SampleFormat::S24_3Le)))
        );
        assert_eq!(
            device_change("suspend", "off"),
            Some(Change::Suspend(Value::Set(false)))
        );
        assert_eq!(device_change("period", "lots"), None);
        assert_eq!(
            device_change("ldac", "hq"),
            Some(Change::LdacQuality(Value::Set(LdacQuality::Hq)))
        );
        assert_eq!(
            device_change("le-latency", "low-latency"),
            Some(Change::LeAudioLatency(Value::Set(LeAudioLatency::Low)))
        );
        assert_eq!(
            device_change("le-audio-presentation-delay", "7.5"),
            Some(Change::LeAudioQos(LeAudioQos::PresentationDelay(
                Value::Set(7500)
            )))
        );
        assert_eq!(
            device_change("le-audio-preset", "default"),
            Some(Change::LeAudioQos(LeAudioQos::Preset(Value::Default)))
        );
        assert_eq!(device_change("le-audio-retransmissions", "lots"), None);
        assert_eq!(
            device_change("le-audio-framing", "unframed"),
            Some(Change::LeAudioQos(LeAudioQos::Framing(Value::Set(false))))
        );
        assert_eq!(
            global_change("headset-autoswitch", "off", &[]),
            Some(Change::HeadsetAutoswitch(Value::Set(false)))
        );
        assert_eq!(
            global_change("force-rate", "off", &[]),
            Some(Change::ForceRate(0))
        );
        assert_eq!(
            global_change("resample", "default", &[]),
            Some(Change::ResampleQuality(Value::Default))
        );
    }

    #[test]
    fn allowed_rate_chips_toggle() {
        let now = [44_100, 48_000];
        assert_eq!(
            global_change("allowed", "96000", &now),
            Some(Change::AllowedRates(Value::Set(vec![
                44_100, 48_000, 96_000
            ])))
        );
        assert_eq!(
            global_change("allowed", "44100", &now),
            Some(Change::AllowedRates(Value::Set(vec![48_000])))
        );
        assert_eq!(
            global_change("allowed", "48000", &[48_000]),
            Some(Change::AllowedRates(Value::Default))
        );
    }
}
