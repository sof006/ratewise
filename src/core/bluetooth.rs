//! Picking a Bluetooth profile from what a user asks for: a mode ("headset"), a codec
//! ("ldac") or a profile name ("a2dp-sink-ldac"). Pure.
//!
//! PipeWire names profiles `<base>` or `<base>-<codec>` (`a2dp-sink-aac`). Since 0.3.78
//! the best codec takes the bare name, so `a2dp-sink` may well be LDAC; the codec is
//! then only in the description ("High Fidelity Playback (A2DP Sink, codec LDAC)").

use crate::core::model::{Bluetooth, BluetoothMode, BluetoothProfile, codec_key, codec_label};

const BASES: [&str; 7] = [
    "a2dp-sink",
    "a2dp-duplex",
    "headset-head-unit",
    "bap-sink",
    "bap-source",
    "bap-duplex",
    "asha-sink",
];

/// The codec a profile uses: from its name's suffix, else from "codec X" in its
/// description, else `None` (PipeWire picks).
pub fn profile_codec(name: &str, description: &str) -> Option<String> {
    let from_name = BASES.iter().find_map(|b| {
        let rest = name.strip_prefix(b)?.strip_prefix('-')?;
        (!rest.is_empty()).then(|| rest.to_string())
    });
    from_name.or_else(|| {
        let start = description.find("codec ")? + "codec ".len();
        let rest = &description[start..];
        let end = rest.find([')', ',']).unwrap_or(rest.len());
        let written = rest[..end].trim();
        (!written.is_empty()).then(|| codec_key(written))
    })
}

/// "High quality (LDAC)", "Headset (mSBC)", "High quality (codec picked automatically)", "Off".
pub fn profile_label(p: &BluetoothProfile) -> String {
    match (p.mode(), p.codec()) {
        (BluetoothMode::Off, _) => "Off".into(),
        (BluetoothMode::Other, _) => p.description.clone(),
        (m, Some(c)) => format!("{} ({})", short_mode(m), codec_label(c)),
        (m, None) if BASES.contains(&p.name.as_str()) => {
            format!("{} (codec picked automatically)", short_mode(m))
        }
        (_, None) => p.description.clone(),
    }
}

fn short_mode(m: BluetoothMode) -> &'static str {
    match m {
        BluetoothMode::HighQuality => "High quality",
        BluetoothMode::Headset => "Headset",
        BluetoothMode::Off => "Off",
        BluetoothMode::Other => "Other",
    }
}

fn mode_alias(s: &str) -> Option<BluetoothMode> {
    Some(match s {
        "high-quality" | "high_quality" | "hq" | "a2dp" | "music" | "playback" => {
            BluetoothMode::HighQuality
        }
        "headset" | "hfp" | "hsp" | "call" | "calls" | "mic" | "microphone" => {
            BluetoothMode::Headset
        }
        "off" => BluetoothMode::Off,
        _ => return None,
    })
}

/// The profile to switch to for `mode`: the current one if it's already in that mode,
/// else the available one PipeWire ranks highest.
pub fn profile_for_mode(bt: &Bluetooth, mode: BluetoothMode) -> Option<&BluetoothProfile> {
    if let Some(a) = bt.active()
        && a.mode() == mode
    {
        return Some(a);
    }
    bt.profiles
        .iter()
        .filter(|p| p.available && p.mode() == mode)
        // max_by_key keeps the last of equals; reverse so the first one wins.
        .rev()
        .max_by_key(|p| p.priority)
}

/// The profile for `codec`, preferring the current mode, then high quality.
pub fn profile_for_codec<'a>(bt: &'a Bluetooth, codec: &str) -> Option<&'a BluetoothProfile> {
    let key = codec_key(codec);
    let current = bt.mode();
    bt.profiles
        .iter()
        .filter(|p| p.codec() == Some(key.as_str()))
        .min_by_key(|p| {
            (
                !p.available,
                match p.mode() {
                    m if m == current => 0,
                    BluetoothMode::HighQuality => 1,
                    _ => 2,
                },
            )
        })
}

/// Finds the profile `want` names, or says what's on offer.
pub fn resolve<'a>(bt: &'a Bluetooth, want: &str) -> Result<&'a BluetoothProfile, String> {
    let w = want.trim();
    let lower = w.to_ascii_lowercase();
    let found = bt
        .profiles
        .iter()
        .find(|p| p.name == w)
        .or_else(|| {
            lower
                .parse::<u32>()
                .ok()
                .and_then(|i| bt.profiles.iter().find(|p| p.index == i))
        })
        .map(Ok)
        .or_else(|| {
            mode_alias(&lower).map(|m| {
                profile_for_mode(bt, m).ok_or_else(|| {
                    format!(
                        "This device has no {} profile right now.",
                        short_mode(m).to_lowercase()
                    )
                })
            })
        })
        .or_else(|| profile_for_codec(bt, &lower).map(Ok));
    match found {
        Some(Ok(p)) if p.available => Ok(p),
        Some(Ok(p)) => Err(format!(
            "{} isn't available right now: the device or the Bluetooth adapter can't use it.",
            profile_label(p)
        )),
        Some(Err(e)) => Err(format!("{e} {}", offer(bt))),
        None => Err(format!(
            "\"{w}\" isn't a profile, mode or codec of this device. {}",
            offer(bt)
        )),
    }
}

/// "Pick high-quality, headset or off, or a codec: SBC, AAC, LDAC."
fn offer(bt: &Bluetooth) -> String {
    let modes: Vec<&str> = bt
        .modes()
        .into_iter()
        .map(|m| match m {
            BluetoothMode::HighQuality => "high-quality",
            BluetoothMode::Headset => "headset",
            _ => "off",
        })
        .collect();
    let mut codecs: Vec<String> = Vec::new();
    for m in [BluetoothMode::HighQuality, BluetoothMode::Headset] {
        for c in bt.codecs(m) {
            let l = codec_label(c);
            if !codecs.contains(&l) {
                codecs.push(l);
            }
        }
    }
    let mut s = String::new();
    if !modes.is_empty() {
        s.push_str(&format!("Pick {}", or_list(&modes)));
    }
    if !codecs.is_empty() {
        s.push_str(if s.is_empty() {
            "Pick a codec: "
        } else {
            ", or a codec: "
        });
        s.push_str(&codecs.join(", "));
    }
    if s.is_empty() {
        "It offers no profiles Ratewise can switch to.".into()
    } else {
        s + "."
    }
}

fn or_list(items: &[&str]) -> String {
    match items {
        [] => String::new(),
        [one] => one.to_string(),
        [rest @ .., last] => format!("{} or {last}", rest.join(", ")),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn profile(index: u32, name: &str, description: &str, priority: u32) -> BluetoothProfile {
        BluetoothProfile {
            index,
            name: name.into(),
            description: description.into(),
            available: true,
            priority,
            codec: profile_codec(name, description),
        }
    }

    /// PipeWire 1.x naming: the best codec (LDAC) takes the bare `a2dp-sink`.
    pub(crate) fn headphones() -> Bluetooth {
        let mut aptx = profile(
            131078,
            "a2dp-sink-aptx_hd",
            "High Fidelity Playback (A2DP Sink, codec aptX HD)",
            131,
        );
        aptx.available = false;
        Bluetooth {
            card_name: "bluez_card.00_00_00_00_00_01".into(),
            card_object_id: 48,
            profile: Some("a2dp-sink-aac".into()),
            codec: Some("aac".into()),
            profiles: vec![
                profile(0, "off", "Off", 0),
                profile(
                    131079,
                    "a2dp-sink",
                    "High Fidelity Playback (A2DP Sink, codec LDAC)",
                    135,
                ),
                profile(
                    131076,
                    "a2dp-sink-aac",
                    "High Fidelity Playback (A2DP Sink, codec AAC)",
                    133,
                ),
                profile(
                    131074,
                    "a2dp-sink-sbc_xq",
                    "High Fidelity Playback (A2DP Sink, codec SBC-XQ)",
                    131,
                ),
                profile(
                    131073,
                    "a2dp-sink-sbc",
                    "High Fidelity Playback (A2DP Sink, codec SBC)",
                    130,
                ),
                aptx,
                profile(
                    196865,
                    "headset-head-unit",
                    "Headset Head Unit (HSP/HFP, codec mSBC)",
                    3,
                ),
                profile(
                    196864,
                    "headset-head-unit-cvsd",
                    "Headset Head Unit (HSP/HFP, codec CVSD)",
                    2,
                ),
            ],
        }
    }

    /// PipeWire 0.3.21 to 0.3.77: a codec-less profile next to the per-codec ones.
    fn old_headset() -> Bluetooth {
        Bluetooth {
            card_name: "bluez_card.00_00_00_00_00_02".into(),
            card_object_id: 60,
            profile: Some("headset-head-unit-msbc".into()),
            codec: Some("msbc".into()),
            profiles: vec![
                profile(0, "off", "Off", 0),
                profile(1, "a2dp-sink", "High Fidelity Playback (A2DP Sink)", 16),
                profile(
                    65537,
                    "a2dp-sink-sbc",
                    "High Fidelity Playback (A2DP Sink, codec SBC)",
                    17,
                ),
                profile(
                    65538,
                    "a2dp-sink-aac",
                    "High Fidelity Playback (A2DP Sink, codec AAC)",
                    18,
                ),
                profile(2, "headset-head-unit", "Headset Head Unit (HSP/HFP)", 1),
                profile(
                    131073,
                    "headset-head-unit-cvsd",
                    "Headset Head Unit (HSP/HFP, codec CVSD)",
                    1,
                ),
                profile(
                    131074,
                    "headset-head-unit-msbc",
                    "Headset Head Unit (HSP/HFP, codec mSBC)",
                    2,
                ),
            ],
        }
    }

    #[test]
    fn codecs_come_from_names_or_descriptions() {
        assert_eq!(
            profile_codec("a2dp-sink-aptx_hd", ""),
            Some("aptx_hd".into())
        );
        assert_eq!(
            profile_codec(
                "a2dp-sink",
                "High Fidelity Playback (A2DP Sink, codec LDAC)"
            ),
            Some("ldac".into())
        );
        assert_eq!(
            profile_codec(
                "a2dp-sink",
                "High Fidelity Playback (A2DP Sink, codec aptX HD)"
            ),
            Some("aptx_hd".into())
        );
        assert_eq!(
            profile_codec("a2dp-sink", "High Fidelity Playback (A2DP Sink)"),
            None
        );
        assert_eq!(profile_codec("off", "Off"), None);
    }

    #[test]
    fn modes_codecs_and_names_resolve() {
        let bt = headphones();
        let name = |w: &str| resolve(&bt, w).map(|p| p.name.clone());
        assert_eq!(
            name("headset").unwrap(),
            "headset-head-unit",
            "highest priority"
        );
        assert_eq!(name("HFP").unwrap(), "headset-head-unit");
        assert_eq!(
            name("high-quality").unwrap(),
            "a2dp-sink-aac",
            "already in that mode"
        );
        assert_eq!(name("ldac").unwrap(), "a2dp-sink", "the bare name is LDAC");
        assert_eq!(name("LDAC").unwrap(), "a2dp-sink");
        assert_eq!(name("sbc-xq").unwrap(), "a2dp-sink-sbc_xq");
        assert_eq!(name("msbc").unwrap(), "headset-head-unit");
        assert_eq!(name("cvsd").unwrap(), "headset-head-unit-cvsd");
        assert_eq!(name("131076").unwrap(), "a2dp-sink-aac");
        assert_eq!(name("off").unwrap(), "off");
        let e = name("aptx-hd").unwrap_err();
        assert!(e.contains("isn't available"), "{e}");
        let e = name("opus").unwrap_err();
        assert!(e.contains("high-quality, headset or off"), "{e}");
        assert!(e.contains("LDAC, AAC, SBC-XQ, SBC, mSBC, CVSD"), "{e}");
    }

    #[test]
    fn older_pipewire_has_an_automatic_profile() {
        let bt = old_headset();
        assert_eq!(bt.summary(), "Headset (with microphone) · mSBC");
        assert_eq!(
            resolve(&bt, "high-quality").unwrap().name,
            "a2dp-sink-aac",
            "priority"
        );
        let auto = resolve(&bt, "a2dp-sink").unwrap();
        assert_eq!(
            profile_label(auto),
            "High quality (codec picked automatically)"
        );
        assert_eq!(bt.codecs(BluetoothMode::HighQuality), ["sbc", "aac"]);
    }

    #[test]
    fn labels_and_summary() {
        let bt = headphones();
        assert_eq!(bt.summary(), "High quality · AAC");
        assert_eq!(
            bt.codecs(BluetoothMode::HighQuality),
            ["ldac", "aac", "sbc_xq", "sbc"]
        );
        assert_eq!(bt.codecs(BluetoothMode::Headset), ["msbc", "cvsd"]);
        assert_eq!(
            profile_label(resolve(&bt, "ldac").unwrap()),
            "High quality (LDAC)"
        );
        assert_eq!(
            bt.modes(),
            [
                BluetoothMode::HighQuality,
                BluetoothMode::Headset,
                BluetoothMode::Off
            ]
        );
    }
}
