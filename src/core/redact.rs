//! Scrubs personal details from captured data (`pw-dump`, `wpctl status`) before it
//! is saved as a fixture, since fixtures get committed and attached to bug reports.
//!
//! - Values of identity keys (user/host names, machine and session ids, serials)
//!   and `media.*` titles are replaced outright.
//! - The user and host names are also scrubbed wherever else they appear as a
//!   whole word, e.g. client `core.name` values (`pipewire-<user>-<pid>`) and
//!   `wpctl status` lines (`<user>@<host>`).
//! - Bluetooth MAC addresses get consistent placeholders, so the same device
//!   keeps the same placeholder everywhere it appears.

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

/// Keys whose values are a user or host name worth scrubbing everywhere.
const IDENTITY_KEYS: &[&str] = &[
    "user-name",
    "host-name",
    "application.process.user",
    "application.process.host",
];

/// Names too generic to scrub as words without damaging the data.
const TOO_COMMON: &[&str] = &[
    "localhost",
    "localdomain",
    "alsa",
    "audio",
    "pipewire",
    "pulse",
    "sink",
    "source",
    "input",
    "output",
    "default",
    "user",
    "usb",
    "pci",
    "hdmi",
];

/// `media.*` keys that describe the stream type rather than its content.
const SAFE_MEDIA_KEYS: &[&str] = &["media.class", "media.role", "media.type", "media.category"];

/// Collects names to scrub, then redacts JSON and text with them.
#[derive(Debug, Default)]
pub struct Redactor {
    words: Vec<String>,
    macs: HashMap<String, String>,
}

impl Redactor {
    pub fn new() -> Self {
        Self::default()
    }

    /// Scrub `name` wherever it appears as a whole word. A dotted hostname also
    /// adds its short form (`box.localdomain` → `box`).
    pub fn add_word(&mut self, name: &str) {
        let name = name.trim();
        let short = name.split('.').next().unwrap_or(name);
        for w in [name, short] {
            let common = TOO_COMMON.contains(&w.to_ascii_lowercase().as_str());
            if !w.is_empty() && w != REDACTED && !common && !self.words.iter().any(|x| x == w) {
                self.words.push(w.to_string());
            }
        }
        // Longest first, so `box.localdomain` is replaced before `box`.
        self.words.sort_by_key(|w| std::cmp::Reverse(w.len()));
    }

    /// Learn user and host names from identity keys in a pw-dump.
    pub fn learn(&mut self, value: &Value) {
        match value {
            Value::Object(map) => {
                for (key, v) in map {
                    match v {
                        Value::String(s) if IDENTITY_KEYS.contains(&key.as_str()) => {
                            self.add_word(s)
                        }
                        other => self.learn(other),
                    }
                }
            }
            Value::Array(items) => items.iter().for_each(|v| self.learn(v)),
            _ => {}
        }
    }

    pub fn redact_json(&mut self, value: &mut Value) {
        match value {
            Value::Object(map) => {
                for (key, v) in map.iter_mut() {
                    if is_secret_key(key) && !v.is_null() {
                        *v = Value::String(REDACTED.into());
                    } else {
                        self.redact_json(v);
                    }
                }
            }
            Value::Array(items) => items.iter_mut().for_each(|v| self.redact_json(v)),
            Value::String(s) => {
                let redacted = self.redact_text(s);
                if redacted != *s {
                    *s = redacted;
                }
            }
            _ => {}
        }
    }

    pub fn redact_text(&mut self, text: &str) -> String {
        let mut out = replace_macs(text, &mut self.macs).unwrap_or_else(|| text.to_string());
        for w in &self.words {
            if let Some(replaced) = replace_word(&out, w) {
                out = replaced;
            }
        }
        out
    }
}

/// Learn names from the dump itself, then redact it.
pub fn redact_pw_dump(value: &mut Value) {
    let mut r = Redactor::new();
    r.learn(value);
    r.redact_json(value);
}

fn is_secret_key(key: &str) -> bool {
    SECRET_KEYS.contains(&key) || (key.starts_with("media.") && !SAFE_MEDIA_KEYS.contains(&key))
}

/// Replaces `word` where it isn't part of a longer alphanumeric run.
fn replace_word(s: &str, word: &str) -> Option<String> {
    let is_word_char = |c: Option<char>| c.is_some_and(char::is_alphanumeric);
    let mut out = String::with_capacity(s.len());
    let mut last = 0;
    let mut changed = false;
    for (i, _) in s.match_indices(word) {
        if i < last {
            continue;
        }
        let before = s[..i].chars().next_back();
        let after = s[i + word.len()..].chars().next();
        if !is_word_char(before) && !is_word_char(after) {
            out.push_str(&s[last..i]);
            out.push_str(REDACTED);
            last = i + word.len();
            changed = true;
        }
    }
    changed.then(|| {
        out.push_str(&s[last..]);
        out
    })
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
    fn learned_names_are_scrubbed_everywhere() {
        // Real leak from a PipeWire 1.6 capture: client core.name embeds the user.
        let mut v = json!([
            {"info": {"user-name": "joe", "host-name": "box.localdomain"}},
            {"info": {"props": {"core.name": "pipewire-joe-1088", "x": "joey", "y": "box"}}}
        ]);
        redact_pw_dump(&mut v);
        let props = &v[1]["info"]["props"];
        assert_eq!(props["core.name"], "pipewire-REDACTED-1088");
        assert_eq!(props["x"], "joey", "only whole words");
        assert_eq!(props["y"], "REDACTED", "short hostname too");
    }

    #[test]
    fn text_redaction_for_wpctl_status() {
        let mut r = Redactor::new();
        r.add_word("joe");
        r.add_word("box.localdomain");
        assert_eq!(
            r.redact_text(" 32. kwin  [1.6.10, joe@box, pid:1088]\n bluez 11:22:33:44:55:66"),
            " 32. kwin  [1.6.10, REDACTED@REDACTED, pid:1088]\n bluez 00:00:00:00:00:01"
        );
    }

    #[test]
    fn common_words_are_never_scrubbed() {
        let mut r = Redactor::new();
        r.add_word("localhost");
        r.add_word("audio");
        r.add_word("");
        assert_eq!(r.redact_text("audio on localhost"), "audio on localhost");
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
        assert_eq!(replace_word("éjoeé", "joe"), None);
        assert_eq!(replace_word("é-joe-é", "joe").unwrap(), "é-REDACTED-é");
    }
}
