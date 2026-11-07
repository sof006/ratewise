//! Scrubs personal details from `pw-dump` output before it is saved as a fixture,
//! since fixtures get committed and attached to bug reports.
//!
//! Removes user/host names, machine and session ids, device serials, what's playing
//! (`media.*` titles), and Bluetooth MAC addresses (replaced consistently, so the
//! same device keeps the same placeholder everywhere it appears).

use std::collections::HashMap;

use serde_json::Value;

const REDACTED: &str = "REDACTED";

const SECRET_KEYS: &[&str] = &[
    "user-name",
    "host-name",
    "application.process.user",
    "application.process.host",
    "application.process.machine-id",
    "application.process.session-id",
    "device.serial",
    "api.alsa.card.serial",
];

/// `media.*` keys that describe the stream type rather than its content.
const SAFE_MEDIA_KEYS: &[&str] = &["media.class", "media.role", "media.type", "media.category"];

pub fn redact_pw_dump(value: &mut Value) {
    let mut macs = HashMap::new();
    walk(value, &mut macs);
}

fn walk(value: &mut Value, macs: &mut HashMap<String, String>) {
    match value {
        Value::Object(map) => {
            for (key, v) in map.iter_mut() {
                if is_secret_key(key) && !v.is_null() {
                    *v = Value::String(REDACTED.into());
                } else {
                    walk(v, macs);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|v| walk(v, macs)),
        Value::String(s) => {
            if let Some(replaced) = replace_macs(s, macs) {
                *s = replaced;
            }
        }
        _ => {}
    }
}

fn is_secret_key(key: &str) -> bool {
    SECRET_KEYS.contains(&key) || (key.starts_with("media.") && !SAFE_MEDIA_KEYS.contains(&key))
}

/// Replaces `AA:BB:CC:DD:EE:FF` and `AA_BB_CC_DD_EE_FF` with stable placeholders.
/// Returns `None` if the string contains no MAC address.
pub fn replace_macs(s: &str, macs: &mut HashMap<String, String>) -> Option<String> {
    const LEN: usize = 17;
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    let mut changed = false;
    while i < bytes.len() {
        if i + LEN <= bytes.len() && is_mac(&bytes[i..i + LEN]) {
            let sep = bytes[i + 2] as char;
            // Key on the normalised address so `:` and `_` spellings share a placeholder.
            let key = s[i..i + LEN].replace('_', ":").to_ascii_uppercase();
            let n = macs.len() + 1;
            let n = macs
                .entry(key)
                .or_insert_with(|| format!("{n:012X}"))
                .clone();
            let placeholder: Vec<&str> = (0..6).map(|k| &n[k * 2..k * 2 + 2]).collect();
            out.push_str(&placeholder.join(&sep.to_string()));
            i += LEN;
            changed = true;
        } else {
            // Copy one whole UTF-8 character.
            let ch = s[i..].chars().next().unwrap_or('\u{fffd}');
            out.push(ch);
            i += ch.len_utf8().max(1);
        }
    }
    changed.then_some(out)
}

fn is_mac(b: &[u8]) -> bool {
    let sep = b[2];
    if sep != b':' && sep != b'_' {
        return false;
    }
    (0..6).all(|k| b[k * 3].is_ascii_hexdigit() && b[k * 3 + 1].is_ascii_hexdigit())
        && (0..5).all(|k| b[k * 3 + 2] == sep)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn scrubs_secrets_and_media_titles() {
        let mut v = json!([{
            "info": {
                "user-name": "joe",
                "props": {
                    "media.class": "Audio/Sink",
                    "media.name": "Secret Song",
                    "application.process.host": "box",
                    "node.name": "alsa_output.pci"
                }
            }
        }]);
        redact_pw_dump(&mut v);
        let props = &v[0]["info"]["props"];
        assert_eq!(v[0]["info"]["user-name"], "REDACTED");
        assert_eq!(props["media.class"], "Audio/Sink");
        assert_eq!(props["media.name"], "REDACTED");
        assert_eq!(props["application.process.host"], "REDACTED");
        assert_eq!(props["node.name"], "alsa_output.pci");
    }

    #[test]
    fn macs_replaced_consistently_across_spellings() {
        let mut v = json!({
            "a": "bluez_output.A4_C1_38_0F_22_9B.1",
            "b": "A4:C1:38:0F:22:9B",
            "c": "other 11:22:33:44:55:66 x"
        });
        redact_pw_dump(&mut v);
        assert_eq!(v["a"], "bluez_output.00_00_00_00_00_01.1");
        assert_eq!(v["b"], "00:00:00:00:00:01");
        assert_eq!(v["c"], "other 00:00:00:00:00:02 x");
    }

    #[test]
    fn non_ascii_survives() {
        let mut macs = HashMap::new();
        assert_eq!(replace_macs("Café — no mac", &mut macs), None);
        assert_eq!(
            replace_macs("é11:22:33:44:55:66é", &mut macs).unwrap(),
            "é00:00:00:00:00:01é"
        );
    }
}
