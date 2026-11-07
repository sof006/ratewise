//! Parses `pw-dump` JSON and the `settings`/`default` metadata (spec §3.1).
//!
//! Field names and value types vary between PipeWire versions (numbers may be
//! strings, metadata values may be JSON inside strings), so everything is read
//! defensively from `serde_json::Value`. Unknown objects are ignored.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Map, Value};

use crate::core::facts::{PwBluezCard, PwClient, PwDump, PwFormat, PwNode, PwProfile};
use crate::core::model::{ClockSettings, RateSet, SampleFormat};
use crate::probe::command::{self, RunError};

/// Where `pw-dump` output comes from: the real tool, or a fixture file.
pub trait PwDumpSource: Send + Sync {
    fn pw_dump(&self) -> Result<String, PipeWireError>;
}

/// Runs the installed `pw-dump`.
pub struct PwDumpCommand {
    pub timeout: Duration,
}

impl Default for PwDumpCommand {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(5),
        }
    }
}

impl PwDumpSource for PwDumpCommand {
    fn pw_dump(&self) -> Result<String, PipeWireError> {
        let out = command::run("pw-dump", &[], self.timeout).map_err(|e| match e {
            RunError::NotFound(_) => PipeWireError::NotInstalled,
            RunError::Timeout { timeout, .. } => PipeWireError::Timeout {
                secs: timeout.as_secs(),
            },
            RunError::Io { source, .. } => PipeWireError::Io(source),
        })?;
        if !out.status.success() {
            let detail = out
                .stderr
                .lines()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("no error message")
                .trim()
                .to_string();
            return Err(PipeWireError::NotRunning { detail });
        }
        Ok(out.stdout)
    }
}

/// Reads `pw-dump.json` from a fixture directory.
pub struct PwDumpFile {
    pub path: PathBuf,
}

impl PwDumpSource for PwDumpFile {
    fn pw_dump(&self) -> Result<String, PipeWireError> {
        std::fs::read_to_string(&self.path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => PipeWireError::NotInFixture,
            _ => PipeWireError::Io(e),
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PipeWireError {
    #[error(
        "pw-dump isn't installed, so PipeWire details aren't available. If you use PipeWire, install your distribution's PipeWire tools (usually the `pipewire` or `pipewire-bin` package)."
    )]
    NotInstalled,
    #[error(
        "pw-dump couldn't reach PipeWire ({detail}). PipeWire probably isn't running for this user."
    )]
    NotRunning { detail: String },
    #[error(
        "pw-dump didn't answer within {secs} seconds. PipeWire may be stuck; restarting it usually helps."
    )]
    Timeout { secs: u64 },
    #[error("couldn't run pw-dump: {0}")]
    Io(#[source] std::io::Error),
    #[error("this fixture has no pw-dump.json, so it simulates a system without PipeWire")]
    NotInFixture,
    #[error(transparent)]
    Parse(#[from] PwDumpParseError),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PwDumpParseError {
    #[error(
        "pw-dump output isn't valid JSON ({0}). Please report this with `ratewise capture-fixture`."
    )]
    Json(String),
    #[error(
        "pw-dump output isn't a JSON array. Please report this with `ratewise capture-fixture`."
    )]
    NotAnArray,
}

type Props = Map<String, Value>;

pub fn parse_pw_dump(json: &str) -> Result<PwDump, PwDumpParseError> {
    let root: Value =
        serde_json::from_str(json).map_err(|e| PwDumpParseError::Json(e.to_string()))?;
    let objects = root.as_array().ok_or(PwDumpParseError::NotAnArray)?;

    let mut dump = PwDump::default();
    let mut devices: HashMap<u32, &Props> = HashMap::new();
    let mut nodes = Vec::new();

    for obj in objects.iter().filter_map(Value::as_object) {
        let ty = obj.get("type").and_then(Value::as_str).unwrap_or_default();
        match ty {
            "PipeWire:Interface:Core" => {
                if let Some(v) = info(obj).and_then(|i| i.get("version")).and_then(as_string) {
                    dump.core_version = Some(v);
                }
            }
            "PipeWire:Interface:Device" => {
                if let (Some(id), Some(p)) = (obj.get("id").and_then(as_u32), props(obj)) {
                    devices.insert(id, p);
                    if let Some(card) = parse_bluez_card(id, obj, p) {
                        dump.bluez_cards.push(card);
                    }
                }
            }
            "PipeWire:Interface:Node" => nodes.push(obj),
            "PipeWire:Interface:Client" => {
                if let Some(p) = props(obj) {
                    dump.clients.push(PwClient {
                        application_name: get_str(p, "application.name"),
                        application_version: get_str(p, "application.version"),
                        config_name: get_str(p, "config.name"),
                    });
                }
            }
            "PipeWire:Interface:Metadata" => parse_metadata(obj, &mut dump),
            _ => {}
        }
    }

    dump.nodes = nodes
        .into_iter()
        .filter_map(|obj| parse_node(obj, &devices))
        .collect();
    Ok(dump)
}

fn info(obj: &Props) -> Option<&Props> {
    obj.get("info").and_then(Value::as_object)
}

/// Props live under `info.props` for most objects, but at the top level for metadata.
fn props(obj: &Props) -> Option<&Props> {
    info(obj)
        .and_then(|i| i.get("props"))
        .or_else(|| obj.get("props"))
        .and_then(Value::as_object)
}

fn parse_node(obj: &Props, devices: &HashMap<u32, &Props>) -> Option<PwNode> {
    let id = obj.get("id").and_then(as_u32)?;
    let p = props(obj)?;
    let media_class = get_str(p, "media.class")?;
    if !media_class.starts_with("Audio/") {
        return None;
    }
    let name = get_str(p, "node.name")?;
    let parent = get_u32(p, "device.id").and_then(|d| devices.get(&d).copied());
    // Node props first, then the parent device's.
    let lookup = |key: &str| get_str(p, key).or_else(|| parent.and_then(|d| get_str(d, key)));
    let lookup_u32 = |keys: &[&str]| {
        keys.iter()
            .find_map(|k| get_u32(p, k).or_else(|| parent.and_then(|d| get_u32(d, k))))
    };

    let params = info(obj)
        .and_then(|i| i.get("params"))
        .and_then(Value::as_object);
    let param_list = |key: &str| {
        params
            .and_then(|ps| ps.get(key))
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(parse_format).collect::<Vec<_>>())
            .unwrap_or_default()
    };

    Some(PwNode {
        id,
        description: get_str(p, "node.description")
            .or_else(|| get_str(p, "node.nick"))
            .or_else(|| parent.and_then(|d| get_str(d, "device.description"))),
        media_class,
        state: info(obj).and_then(|i| i.get("state")).and_then(as_string),
        device_api: lookup("device.api"),
        bus: lookup("device.bus"),
        alsa_card: lookup_u32(&["alsa.card", "api.alsa.pcm.card", "api.alsa.card"]),
        alsa_device: lookup_u32(&["alsa.device", "api.alsa.pcm.device"]),
        alsa_path: lookup("api.alsa.path"),
        profile_name: get_str(p, "device.profile.name"),
        device_id: get_u32(p, "device.id"),
        bluez_codec: get_str(p, "api.bluez5.codec"),
        enum_formats: param_list("EnumFormat"),
        current_format: param_list("Format").into_iter().next(),
        name,
    })
}

/// A Bluetooth card with its `EnumProfile` and `Profile` params.
fn parse_bluez_card(id: u32, obj: &Props, p: &Props) -> Option<PwBluezCard> {
    if get_str(p, "device.api").as_deref() != Some("bluez5") {
        return None;
    }
    let params = info(obj)
        .and_then(|i| i.get("params"))
        .and_then(Value::as_object);
    let list = |key: &str| -> Vec<PwProfile> {
        params
            .and_then(|ps| ps.get(key))
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(parse_profile).collect())
            .unwrap_or_default()
    };
    Some(PwBluezCard {
        id,
        name: get_str(p, "device.name").unwrap_or_else(|| format!("bluez_card.{id}")),
        description: get_str(p, "device.description").or_else(|| get_str(p, "device.alias")),
        profiles: list("EnumProfile"),
        profile: list("Profile").into_iter().next(),
    })
}

fn parse_profile(v: &Value) -> Option<PwProfile> {
    let o = v.as_object()?;
    Some(PwProfile {
        index: o.get("index").and_then(as_u32)?,
        name: o.get("name").and_then(as_string)?,
        description: o.get("description").and_then(as_string),
        available: match o.get("available") {
            Some(Value::String(s)) => s != "no",
            Some(Value::Bool(b)) => *b,
            _ => true,
        },
        priority: o.get("priority").and_then(as_u32).unwrap_or(0),
    })
}

/// One `EnumFormat`/`Format` object. Non-raw formats (IEC958, DSD) are skipped.
fn parse_format(v: &Value) -> Option<PwFormat> {
    let o = v.as_object()?;
    let media_type = o.get("mediaType").and_then(Value::as_str);
    let subtype = o.get("mediaSubtype").and_then(Value::as_str);
    if media_type.is_some_and(|t| t != "audio") || subtype.is_some_and(|t| t != "raw") {
        return None;
    }
    let formats = o
        .get("format")
        .map(|f| {
            choice(f)
                .values()
                .filter_map(as_string)
                .map(|s| SampleFormat::from_pipewire(&s))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let rates = o.get("rate").and_then(|r| match choice(r) {
        Choice::Range { min, max } => Some(RateSet::range(as_u32(min)?, as_u32(max)?)),
        c => {
            let rates: Vec<u32> = c.values().filter_map(as_u32).collect();
            (!rates.is_empty()).then(|| RateSet::discrete(rates))
        }
    });
    let mut channels: Vec<u32> = o
        .get("channels")
        .map(|c| match choice(c) {
            Choice::Range { min, max } => match (as_u32(min), as_u32(max)) {
                (Some(lo), Some(hi)) if hi >= lo && hi - lo <= 64 => (lo..=hi).collect(),
                (lo, hi) => lo.into_iter().chain(hi).collect(),
            },
            c => c.values().filter_map(as_u32).collect(),
        })
        .unwrap_or_default();
    channels.sort_unstable();
    channels.dedup();
    let mut formats = formats;
    formats.sort();
    formats.dedup();
    Some(PwFormat {
        formats,
        rates,
        channels,
    })
}

/// pw-dump prints SPA choices as `{"default": x, "min": a, "max": b}` (range/step),
/// `{"default": x, "alt1": a, "alt2": b}` (enum), or just the plain value.
enum Choice<'a> {
    Values(Vec<&'a Value>),
    Range { min: &'a Value, max: &'a Value },
}

impl<'a> Choice<'a> {
    fn values(self) -> impl Iterator<Item = &'a Value> {
        match self {
            Choice::Values(v) => v.into_iter(),
            Choice::Range { min, max } => vec![min, max].into_iter(),
        }
    }
}

fn choice(v: &Value) -> Choice<'_> {
    match v {
        Value::Object(m) => {
            if let (Some(min), Some(max)) = (m.get("min"), m.get("max")) {
                return Choice::Range { min, max };
            }
            let mut alts: Vec<(u32, &Value)> = m
                .iter()
                .filter_map(|(k, v)| {
                    let n = k.strip_prefix("alt").or_else(|| k.strip_prefix("flag"))?;
                    Some((n.parse().ok()?, v))
                })
                .collect();
            alts.sort_by_key(|(n, _)| *n);
            let mut values: Vec<&Value> = m.get("default").into_iter().collect();
            values.extend(alts.into_iter().map(|(_, v)| v));
            Choice::Values(values)
        }
        Value::Array(a) => Choice::Values(a.iter().collect()),
        other => Choice::Values(vec![other]),
    }
}

fn parse_metadata(obj: &Props, dump: &mut PwDump) {
    let Some(name) = props(obj).and_then(|p| get_str(p, "metadata.name")) else {
        return;
    };
    let entries = obj
        .get("metadata")
        .and_then(Value::as_array)
        .map(|a| a.as_slice())
        .unwrap_or_default();
    let global = entries.iter().filter_map(Value::as_object).filter(|e| {
        // Subject 0 is the global scope.
        e.get("subject").and_then(as_u32).unwrap_or(0) == 0
    });

    match name.as_str() {
        "settings" => {
            let clock = dump.clock.get_or_insert_with(ClockSettings::default);
            for e in global {
                let (Some(key), Some(value)) =
                    (e.get("key").and_then(Value::as_str), e.get("value"))
                else {
                    continue;
                };
                match key {
                    "clock.rate" => clock.rate = as_u32(value),
                    "clock.allowed-rates" => clock.allowed_rates = number_list(value),
                    "clock.quantum" => clock.quantum = as_u32(value),
                    "clock.min-quantum" => clock.min_quantum = as_u32(value),
                    "clock.max-quantum" => clock.max_quantum = as_u32(value),
                    "clock.force-rate" => clock.force_rate = as_u32(value),
                    "clock.force-quantum" => clock.force_quantum = as_u32(value),
                    _ => {}
                }
            }
        }
        "sm-settings" | "persistent-sm-settings" => {
            let saved = name == "persistent-sm-settings";
            for e in global {
                if e.get("key").and_then(Value::as_str)
                    != Some("bluetooth.autoswitch-to-headset-profile")
                {
                    continue;
                }
                let v = e.get("value").and_then(as_bool);
                let w = &mut dump.wireplumber;
                if saved {
                    w.headset_autoswitch_saved = v;
                } else {
                    w.headset_autoswitch = v;
                }
            }
        }
        "default" => {
            for e in global {
                let (Some(key), Some(value)) =
                    (e.get("key").and_then(Value::as_str), e.get("value"))
                else {
                    continue;
                };
                match key {
                    "default.audio.sink" => dump.default_sink = default_name(value),
                    "default.audio.source" => dump.default_source = default_name(value),
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

/// `{"name": "..."}`, the same as a JSON string, or a bare name.
fn default_name(v: &Value) -> Option<String> {
    match v {
        Value::Object(o) => o.get("name").and_then(as_string),
        Value::String(s) => match serde_json::from_str::<Value>(s) {
            Ok(Value::Object(o)) => o.get("name").and_then(as_string),
            _ => Some(s.clone()),
        },
        _ => None,
    }
}

/// `[44100, 48000]`, or a string like `"[ 44100 48000 ]"` (SPA-JSON, commas optional).
fn number_list(v: &Value) -> Option<Vec<u32>> {
    let list: Vec<u32> = match v {
        Value::Array(a) => a.iter().filter_map(as_u32).collect(),
        Value::String(s) => s
            .split(|c: char| !c.is_ascii_digit())
            .filter_map(|n| n.parse().ok())
            .collect(),
        other => as_u32(other).into_iter().collect(),
    };
    (!list.is_empty()).then_some(list)
}

fn get_str(p: &Props, key: &str) -> Option<String> {
    p.get(key).and_then(as_string)
}

fn get_u32(p: &Props, key: &str) -> Option<u32> {
    p.get(key).and_then(as_u32)
}

/// Strings as-is; numbers and bools in their JSON spelling.
fn as_string(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// `true`, or `"true"` (metadata values may be JSON inside strings).
fn as_bool(v: &Value) -> Option<bool> {
    match v {
        Value::Bool(b) => Some(*b),
        Value::String(s) => match s.trim() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

/// Non-negative integers, from a JSON number or a numeric string.
fn as_u32(v: &Value) -> Option<u32> {
    match v {
        Value::Number(n) => n.as_u64().and_then(|n| u32::try_from(n).ok()),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn choices() {
        let f = parse_format(&json!({
            "mediaType": "audio", "mediaSubtype": "raw",
            "format": {"default": "S32LE", "alt1": "S32LE", "alt2": "S16LE"},
            "rate": {"default": 48000, "min": 44100, "max": 192000},
            "channels": 2
        }))
        .unwrap();
        assert_eq!(f.formats, vec![SampleFormat::S16Le, SampleFormat::S32Le]);
        assert_eq!(f.rates, Some(RateSet::range(44_100, 192_000)));
        assert_eq!(f.channels, vec![2]);

        let f = parse_format(&json!({
            "format": "F32P",
            "rate": {"default": 48000, "alt1": 44100, "alt2": 48000, "alt3": 96000},
            "channels": {"default": 2, "min": 1, "max": 8}
        }))
        .unwrap();
        assert_eq!(f.formats, vec![SampleFormat::FloatLe]);
        assert_eq!(f.rates, Some(RateSet::discrete([44_100, 48_000, 96_000])));
        assert_eq!(f.channels, (1..=8).collect::<Vec<_>>());

        assert!(parse_format(&json!({"mediaType": "audio", "mediaSubtype": "iec958"})).is_none());
    }

    #[test]
    fn metadata_value_spellings() {
        assert_eq!(
            number_list(&json!([44100, 48000])),
            Some(vec![44_100, 48_000])
        );
        assert_eq!(
            number_list(&json!("[ 44100 48000 ]")),
            Some(vec![44_100, 48_000])
        );
        assert_eq!(number_list(&json!("[]")), None);
        assert_eq!(default_name(&json!({"name": "a"})), Some("a".into()));
        assert_eq!(default_name(&json!("{\"name\": \"b\"}")), Some("b".into()));
        assert_eq!(as_u32(&json!("48000")), Some(48_000));
        assert_eq!(as_u32(&json!(-1)), None);
    }

    #[test]
    fn rejects_non_arrays() {
        assert_eq!(parse_pw_dump("{}"), Err(PwDumpParseError::NotAnArray));
        assert!(matches!(
            parse_pw_dump("not json"),
            Err(PwDumpParseError::Json(_))
        ));
        assert_eq!(parse_pw_dump("[]").unwrap(), PwDump::default());
    }

    #[test]
    fn node_inherits_parent_device_props() {
        let dump = parse_pw_dump(
            &json!([
                {"id": 45, "type": "PipeWire:Interface:Device",
                 "info": {"props": {"device.api": "alsa", "device.bus": "usb", "api.alsa.card": 2}}},
                {"id": 70, "type": "PipeWire:Interface:Node",
                 "info": {"state": "idle", "props": {
                    "media.class": "Audio/Sink", "node.name": "alsa_output.usb", "device.id": 45,
                    "alsa.device": "0"}}},
                {"id": 71, "type": "PipeWire:Interface:Node",
                 "info": {"props": {"media.class": "Video/Source", "node.name": "cam"}}}
            ])
            .to_string(),
        )
        .unwrap();
        assert_eq!(dump.nodes.len(), 1);
        let n = &dump.nodes[0];
        assert_eq!(n.bus.as_deref(), Some("usb"));
        assert_eq!(n.alsa_card, Some(2));
        assert_eq!(n.alsa_device, Some(0));
        assert_eq!(n.state.as_deref(), Some("idle"));
    }
}
