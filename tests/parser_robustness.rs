//! Spec §8: garbage input to any parser produces an error, never a panic.

use proptest::prelude::*;
use ratewise::probe::pipewire::parse_pw_dump;
use ratewise::probe::procfs::{
    parse_cards, parse_hw_params, parse_pcm_dir_name, parse_pcm_info, parse_usb_stream,
};
use serde_json::Value;

/// Lines built from the vocabulary the parsers look for, so fuzzing reaches deep branches.
fn procfs_like() -> impl Strategy<Value = String> {
    let line = prop_oneof![
        Just("Playback:".to_string()),
        Just("Capture:".to_string()),
        Just("  Status: Running".to_string()),
        Just("  Interface 1".to_string()),
        Just("    Altset 2".to_string()),
        Just("    Interface = 1".to_string()),
        Just("    Momentary freq = ".to_string()),
        Just("    Rates: ".to_string()),
        Just(" - ".to_string()),
        Just("rate: ".to_string()),
        Just("format: ".to_string()),
        Just("closed".to_string()),
        Just(" 0 [".to_string()),
        Just("]: ".to_string()),
        Just("--- no soundcards ---".to_string()),
        "[0-9]{0,12}",
        "[ ,:\\-\\[\\]a-zA-Z0-9_]{0,24}",
        "\\PC{0,16}",
    ];
    prop::collection::vec(
        prop::collection::vec(line, 1..4).prop_map(|p| p.concat()),
        0..24,
    )
    .prop_map(|lines| lines.join("\n"))
}

fn arb_json() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(Value::from),
        any::<f64>()
            .prop_map(|f| serde_json::Number::from_f64(f).map_or(Value::Null, Value::Number)),
        prop_oneof![
            Just("PipeWire:Interface:Node".to_string()),
            Just("PipeWire:Interface:Device".to_string()),
            Just("PipeWire:Interface:Metadata".to_string()),
            Just("PipeWire:Interface:Core".to_string()),
            Just("Audio/Sink".to_string()),
            Just("settings".to_string()),
            Just("default".to_string()),
            Just("S32LE".to_string()),
            "\\PC{0,12}",
        ]
        .prop_map(Value::String),
    ];
    let keys = prop_oneof![
        Just("id"),
        Just("type"),
        Just("info"),
        Just("props"),
        Just("params"),
        Just("EnumFormat"),
        Just("Format"),
        Just("metadata"),
        Just("metadata.name"),
        Just("media.class"),
        Just("node.name"),
        Just("device.id"),
        Just("alsa.card"),
        Just("format"),
        Just("rate"),
        Just("channels"),
        Just("min"),
        Just("max"),
        Just("default"),
        Just("alt1"),
        Just("key"),
        Just("value"),
        Just("subject"),
        Just("clock.allowed-rates"),
        Just("default.audio.sink"),
    ]
    .prop_map(str::to_string);
    leaf.prop_recursive(5, 64, 8, move |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..6).prop_map(Value::Array),
            prop::collection::vec((keys.clone(), inner), 0..6)
                .prop_map(|kv| Value::Object(kv.into_iter().collect())),
        ]
    })
}

proptest! {
    #[test]
    fn procfs_parsers_never_panic(s in procfs_like()) {
        let _ = parse_cards(&s);
        let _ = parse_hw_params(&s);
        let _ = parse_usb_stream(&s);
        let _ = parse_pcm_info(&s);
        let _ = parse_pcm_dir_name(&s);
    }

    #[test]
    fn procfs_parsers_never_panic_on_any_text(s in "\\PC*") {
        let _ = parse_cards(&s);
        let _ = parse_hw_params(&s);
        let _ = parse_usb_stream(&s);
        let _ = parse_pcm_info(&s);
        let _ = parse_pcm_dir_name(&s);
    }

    #[test]
    fn pw_dump_never_panics_on_any_text(s in "\\PC*") {
        let _ = parse_pw_dump(&s);
    }

    #[test]
    fn pw_dump_never_panics_on_odd_json(v in prop::collection::vec(arb_json(), 0..8)) {
        let _ = parse_pw_dump(&Value::Array(v).to_string());
    }

    #[test]
    fn truncated_fixture_never_panics(cut in 0usize..20_000) {
        let text = include_str!("fixtures/pw-1.2-desktop/pw-dump.json");
        let mut cut = cut.min(text.len());
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        let _ = parse_pw_dump(&text[..cut]);
    }
}
