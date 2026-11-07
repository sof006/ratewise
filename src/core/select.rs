//! Picks a device from a user's selector (spec §7): the stable id or PipeWire
//! node name, an ALSA `hw:` id, or words from the description.

use crate::core::model::Device;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SelectError {
    #[error("no device matches \"{query}\". Run `ratewise list` to see device names and ids.")]
    NotFound { query: String },
    #[error(
        "\"{query}\" matches more than one device. Use one of these ids instead:\n{}",
        matches.iter().map(|m| format!("  {m}")).collect::<Vec<_>>().join("\n")
    )]
    Ambiguous { query: String, matches: Vec<String> },
}

pub fn select<'a>(devices: &'a [Device], query: &str) -> Result<&'a Device, SelectError> {
    let q = query.trim();
    let one = |found: Vec<&'a Device>| -> Result<&'a Device, SelectError> {
        match found.as_slice() {
            [d] => Ok(d),
            [] => Err(SelectError::NotFound {
                query: q.to_string(),
            }),
            many => Err(SelectError::Ambiguous {
                query: q.to_string(),
                matches: many.iter().map(|d| d.id.clone()).collect(),
            }),
        }
    };

    if let Some(d) = devices
        .iter()
        .find(|d| d.id == q || d.pipewire.as_ref().is_some_and(|p| p.node_name == q))
    {
        return Ok(d);
    }
    if let Some((card, dev)) = parse_hw(q) {
        return one(devices
            .iter()
            .filter(|d| {
                d.alsa.as_ref().is_some_and(|a| {
                    (a.card_id.as_deref() == Some(card.as_str()) || a.card.to_string() == card)
                        && a.device == dev
                })
            })
            .collect());
    }
    let words: Vec<String> = q.split_whitespace().map(str::to_lowercase).collect();
    if words.is_empty() {
        return one(Vec::new());
    }
    one(devices
        .iter()
        .filter(|d| {
            let desc = d.description.to_lowercase();
            words.iter().all(|w| desc.contains(w.as_str()))
        })
        .collect())
}

/// `hw:CARD=PCH,DEV=0`, `hw:PCH,0`, `hw:0,3` or `hw:0` (device 0).
fn parse_hw(q: &str) -> Option<(String, u32)> {
    let rest = q
        .strip_prefix("hw:")
        .or_else(|| q.strip_prefix("plughw:"))?;
    let mut card = None;
    let mut dev = 0;
    for (i, part) in rest.split(',').enumerate() {
        let (key, value) = match part.split_once('=') {
            Some((k, v)) => (k.trim().to_ascii_uppercase(), v.trim()),
            None => (
                (if i == 0 { "CARD" } else { "DEV" }).to_string(),
                part.trim(),
            ),
        };
        match key.as_str() {
            "CARD" => card = Some(value.to_string()),
            "DEV" => dev = value.parse().ok()?,
            _ => return None,
        }
    }
    Some((card.filter(|c| !c.is_empty())?, dev))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::{AlsaPcm, Capabilities, DeviceKind, Direction, PipeWireNode};

    fn dev(
        id: &str,
        desc: &str,
        card: (u32, &str, u32),
        dir: Direction,
        node: Option<&str>,
    ) -> Device {
        Device {
            id: id.into(),
            description: desc.into(),
            direction: dir,
            kind: DeviceKind::Unknown,
            alsa: Some(AlsaPcm {
                card: card.0,
                card_id: Some(card.1.into()),
                device: card.2,
                card_driver: None,
                pcm_name: None,
            }),
            pipewire: node.map(|n| PipeWireNode {
                object_id: 1,
                node_name: n.into(),
                media_class: "Audio/Sink".into(),
                state: None,
                is_default: false,
            }),
            capabilities: Capabilities::default(),
            running: None,
            kernel_probe: None,
            bluetooth: None,
        }
    }

    fn devices() -> Vec<Device> {
        vec![
            dev(
                "alsa_output.pci.analog",
                "Built-in Audio Analog Stereo",
                (0, "PCH", 0),
                Direction::Playback,
                Some("alsa_output.pci.analog"),
            ),
            dev(
                "alsa_input.pci.analog",
                "Built-in Audio Analog Stereo",
                (0, "PCH", 0),
                Direction::Capture,
                Some("alsa_input.pci.analog"),
            ),
            dev(
                "alsa:NVidia:7:playback",
                "HDA NVidia: HDMI 1",
                (1, "NVidia", 7),
                Direction::Playback,
                None,
            ),
            dev(
                "alsa_output.usb-D50s",
                "D50s Analog Stereo",
                (2, "D50s", 0),
                Direction::Playback,
                Some("alsa_output.usb-D50s"),
            ),
        ]
    }

    #[test]
    fn exact_ids_win() {
        let ds = devices();
        assert_eq!(
            select(&ds, "alsa:NVidia:7:playback").unwrap().id,
            "alsa:NVidia:7:playback"
        );
        assert_eq!(
            select(&ds, "alsa_output.usb-D50s").unwrap().id,
            "alsa_output.usb-D50s"
        );
    }

    #[test]
    fn hw_ids_in_every_spelling() {
        let ds = devices();
        for q in [
            "hw:CARD=D50s,DEV=0",
            "hw:D50s,0",
            "hw:2,0",
            "hw:2",
            "plughw:D50s",
        ] {
            assert_eq!(select(&ds, q).unwrap().id, "alsa_output.usb-D50s", "{q}");
        }
        assert_eq!(
            select(&ds, "hw:NVidia,7").unwrap().id,
            "alsa:NVidia:7:playback"
        );
        // PCH device 0 has both a playback and a capture side.
        assert!(matches!(
            select(&ds, "hw:PCH,0"),
            Err(SelectError::Ambiguous { .. })
        ));
        assert!(matches!(
            select(&ds, "hw:9,9"),
            Err(SelectError::NotFound { .. })
        ));
    }

    #[test]
    fn description_words_match_case_insensitively() {
        let ds = devices();
        assert_eq!(select(&ds, "d50s").unwrap().id, "alsa_output.usb-D50s");
        assert_eq!(
            select(&ds, "nvidia hdmi").unwrap().id,
            "alsa:NVidia:7:playback"
        );
        let err = select(&ds, "built-in").unwrap_err();
        assert!(err.to_string().contains("alsa_input.pci.analog"), "{err}");
        assert!(matches!(
            select(&ds, "  "),
            Err(SelectError::NotFound { .. })
        ));
        assert!(matches!(
            select(&ds, "speakers"),
            Err(SelectError::NotFound { .. })
        ));
    }
}
