//! Generates the config files Ratewise owns, from its saved record. Every
//! function returns `None` when there's nothing to write, meaning the file
//! shouldn't exist.

use std::collections::BTreeMap;
use std::fmt::Write;

use crate::core::model::Direction;
use crate::core::settings::{DeviceSaved, GlobalSaved};

const HEADER: &str = "Written by Ratewise. It regenerates this file on every `ratewise set --save`,\n\
                      so edits here are lost. `ratewise reset` removes it.";

fn header(comment: &str) -> String {
    HEADER
        .lines()
        .map(|l| format!("{comment} {}\n", l.trim()))
        .collect()
}

/// A quoted string for SPA-JSON or Lua (both accept these escapes).
fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// One line of text safe to put in a comment.
fn comment_text(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

fn rates(rs: &[u32]) -> String {
    let items: Vec<String> = rs.iter().map(u32::to_string).collect();
    format!("[ {} ]", items.join(" "))
}

/// `~/.config/pipewire/pipewire.conf.d/60-ratewise.conf`: the graph clock.
pub fn pipewire(g: &GlobalSaved) -> Option<String> {
    let mut props = Vec::new();
    if let Some(r) = g.rate {
        props.push(format!("default.clock.rate = {r}"));
    }
    if let Some(rs) = &g.allowed_rates {
        props.push(format!("default.clock.allowed-rates = {}", rates(rs)));
    }
    for (key, v) in [
        ("quantum", g.quantum),
        ("min-quantum", g.min_quantum),
        ("max-quantum", g.max_quantum),
    ] {
        if let Some(v) = v {
            props.push(format!("default.clock.{key} = {v}"));
        }
    }
    if props.is_empty() {
        return None;
    }
    let mut s = header("#");
    s.push_str("context.properties = {\n");
    for p in props {
        let _ = writeln!(s, "    {p}");
    }
    s.push_str("}\n");
    Some(s)
}

/// `client.conf.d` and `pipewire-pulse.conf.d`: properties for new streams.
pub fn stream_properties(g: &GlobalSaved) -> Option<String> {
    let q = g.resample_quality?;
    Some(format!(
        "{}stream.properties = {{\n    resample.quality = {q}\n}}\n",
        header("#")
    ))
}

/// (key, value) pairs a WirePlumber rule sets on one node.
type Props = Vec<(&'static str, String)>;

/// The properties a WirePlumber rule sets on one node.
fn node_props(d: &DeviceSaved) -> Props {
    let mut p = Vec::new();
    if let Some(name) = d.format.as_ref().and_then(|f| f.pipewire_name()) {
        p.push(("audio.format", quote(name)));
    }
    if let Some(r) = d.rate {
        p.push(("audio.rate", r.to_string()));
    }
    if let Some(n) = d.period_size {
        p.push(("api.alsa.period-size", n.to_string()));
    }
    if let Some(n) = d.headroom {
        p.push(("api.alsa.headroom", n.to_string()));
    }
    if d.disable_suspend {
        p.push(("session.suspend-timeout-seconds", "0".into()));
    }
    p
}

fn rules(devices: &BTreeMap<String, DeviceSaved>) -> Vec<(&str, &DeviceSaved, Props)> {
    devices
        .values()
        .filter_map(|d| {
            let node = d.pipewire_node.as_deref()?;
            let props = node_props(d);
            (!props.is_empty()).then_some((node, d, props))
        })
        .collect()
}

/// WirePlumber 0.5+: `~/.config/wireplumber/wireplumber.conf.d/60-ratewise.conf`.
pub fn wireplumber_05(devices: &BTreeMap<String, DeviceSaved>) -> Option<String> {
    let rules = rules(devices);
    if rules.is_empty() {
        return None;
    }
    let mut s = header("#");
    s.push_str("monitor.alsa.rules = [\n");
    for (node, d, props) in rules {
        let _ = writeln!(s, "  {{");
        let _ = writeln!(s, "    # {}", comment_text(&d.description));
        let _ = writeln!(s, "    matches = [");
        let _ = writeln!(s, "      {{ node.name = {} }}", quote(node));
        let _ = writeln!(s, "    ]");
        let _ = writeln!(s, "    actions = {{");
        let _ = writeln!(s, "      update-props = {{");
        for (k, v) in props {
            let _ = writeln!(s, "        {k} = {v}");
        }
        let _ = writeln!(s, "      }}");
        let _ = writeln!(s, "    }}");
        let _ = writeln!(s, "  }}");
    }
    s.push_str("]\n");
    Some(s)
}

/// WirePlumber 0.4: `~/.config/wireplumber/main.lua.d/60-ratewise.lua`. It loads
/// after `50-alsa-config.lua`, so these rules are applied last and win.
pub fn wireplumber_04(devices: &BTreeMap<String, DeviceSaved>) -> Option<String> {
    let rules = rules(devices);
    if rules.is_empty() {
        return None;
    }
    let mut s = header("--");
    // Never fail if 50-alsa-config.lua was replaced: a Lua error stops the ALSA monitor.
    s.push_str("alsa_monitor = alsa_monitor or {}\n");
    s.push_str("alsa_monitor.rules = alsa_monitor.rules or {}\n");
    for (node, d, props) in rules {
        let _ = writeln!(s, "\n-- {}", comment_text(&d.description));
        let _ = writeln!(s, "table.insert(alsa_monitor.rules, {{");
        let _ = writeln!(s, "  matches = {{");
        let _ = writeln!(s, "    {{");
        let _ = writeln!(s, "      {{ \"node.name\", \"equals\", {} }},", quote(node));
        let _ = writeln!(s, "    }},");
        let _ = writeln!(s, "  }},");
        let _ = writeln!(s, "  apply_properties = {{");
        for (k, v) in props {
            let _ = writeln!(s, "    [\"{k}\"] = {v},");
        }
        let _ = writeln!(s, "  }},");
        let _ = writeln!(s, "}})");
    }
    Some(s)
}

/// PulseAudio: `~/.config/pulse/daemon.conf.d/60-ratewise.conf`. Files in the
/// `.d` folder are read after `daemon.conf`, so these values win.
pub fn pulse(g: &GlobalSaved) -> Option<String> {
    let mut lines = Vec::new();
    let default = match &g.allowed_rates {
        Some(rs) => g.rate.or_else(|| rs.first().copied()),
        None => g.rate,
    };
    if let Some(r) = default {
        lines.push(format!("default-sample-rate = {r}"));
    }
    if let Some(rs) = &g.allowed_rates {
        // PulseAudio switches between two rates. The same value as the default disables switching.
        let alternate = rs.iter().copied().find(|&r| Some(r) != default).or(default);
        if let Some(a) = alternate {
            lines.push(format!("alternate-sample-rate = {a}"));
        }
        lines.push("avoid-resampling = yes".into());
    }
    if let Some(f) = g.format.as_ref().and_then(|f| f.pulse_name()) {
        lines.push(format!("default-sample-format = {f}"));
    }
    if lines.is_empty() {
        return None;
    }
    let mut s = header("#");
    for l in lines {
        let _ = writeln!(s, "{l}");
    }
    Some(s)
}

/// The header Ratewise adds when it creates `~/.config/pulse/daemon.conf`.
pub fn pulse_daemon_conf_copy(system: Option<&str>) -> String {
    let mut s = String::from(
        "# Created by Ratewise as a copy of /etc/pulse/daemon.conf. PulseAudio reads\n\
         # ~/.config/pulse/daemon.conf.d/ only when this file exists. You can edit it.\n",
    );
    match system {
        Some(text) => {
            s.push_str(text);
            if !text.ends_with('\n') && !text.is_empty() {
                s.push('\n');
            }
        }
        None => s.push_str(
            "# /etc/pulse/daemon.conf wasn't found, so PulseAudio's built-in defaults apply.\n",
        ),
    }
    s
}

/// The contents of Ratewise's block in `~/.asoundrc` (dmix for playback, dsnoop for capture).
///
/// Keys as alsa-lib's `pcm/dmix.conf` and `pcm/dsnoop.conf` look them up: dsnoop takes
/// its rate and format from `defaults.pcm.dmix.*` too, and the two plugins name their
/// per-card period size differently.
pub fn alsa_block(g: &GlobalSaved, devices: &BTreeMap<String, DeviceSaved>) -> Option<String> {
    let mut s = String::new();
    if let Some(r) = g.rate {
        let _ = writeln!(s, "defaults.pcm.dmix.rate {r}");
    }
    if let Some(f) = &g.format {
        let _ = writeln!(s, "defaults.pcm.dmix.format {}", quote(f.alsa_name()));
    }
    for d in devices.values() {
        let (Some(card), Some(n)) = (&d.alsa_card_id, d.period_size) else {
            continue;
        };
        match d.direction {
            Direction::Capture => {
                let _ = writeln!(s, "cards.{card}.pcm.dsnoop.period_size {n}");
            }
            _ => {
                let _ = writeln!(s, "defaults.dmix.{card}.period_size {n}");
            }
        }
    }
    (!s.is_empty()).then_some(s)
}

/// ALSA card ids that are safe to use unquoted in a config key.
pub fn alsa_key_safe(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

pub const BLOCK_START: &str = "# >>> ratewise >>>";
pub const BLOCK_END: &str = "# <<< ratewise <<<";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("it has a `{BLOCK_START}` line without a matching `{BLOCK_END}`")]
pub struct DamagedBlock;

/// Puts `body` into Ratewise's marked block in `existing`, replacing any old
/// block and keeping everything else byte for byte. With `body = None` the
/// block is removed. Returns `None` when the result would be empty.
pub fn splice_block(
    existing: Option<&str>,
    body: Option<&str>,
) -> Result<Option<String>, DamagedBlock> {
    let text = existing.unwrap_or("");
    let block = body.map(|b| {
        format!(
            "{BLOCK_START} Managed by Ratewise; `ratewise reset` removes this block.\n{b}{BLOCK_END}\n"
        )
    });
    let out = match find_block(text)? {
        Some((start, end)) => {
            format!(
                "{}{}{}",
                &text[..start],
                block.as_deref().unwrap_or(""),
                &text[end..]
            )
        }
        None => match block {
            Some(b) if text.is_empty() || text.ends_with('\n') => format!("{text}{b}"),
            Some(b) => format!("{text}\n{b}"),
            None => text.to_string(),
        },
    };
    Ok((!out.is_empty()).then_some(out))
}

/// Byte range of the block, from the start of its first line to the end of its last.
fn find_block(text: &str) -> Result<Option<(usize, usize)>, DamagedBlock> {
    let mut start = None;
    let mut pos = 0;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_end();
        match start {
            None if trimmed.starts_with(BLOCK_START) => start = Some(pos),
            Some(s) if trimmed.starts_with(BLOCK_END) => return Ok(Some((s, pos + line.len()))),
            _ => {}
        }
        pos += line.len();
    }
    match start {
        Some(_) => Err(DamagedBlock),
        None => Ok(None),
    }
}

/// Lines outside Ratewise's block that set one of `keys` after the block, which
/// would override it.
pub fn overrides_after_block(text: &str, keys: &[&str]) -> Vec<String> {
    let Ok(Some((_, end))) = find_block(text) else {
        return Vec::new();
    };
    text[end..]
        .lines()
        .map(str::trim)
        .filter(|l| keys.iter().any(|k| l.starts_with(k)))
        .map(String::from)
        .collect()
}

/// FNV-1a, to recognise a file Ratewise wrote. Not for security.
pub fn fingerprint(text: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting_escapes_what_it_must() {
        assert_eq!(quote(r#"a"b\c"#), r#""a\"b\\c""#);
        assert_eq!(quote("x\u{7}y"), r#""xy""#);
    }

    #[test]
    fn empty_settings_mean_no_file() {
        let g = GlobalSaved::default();
        assert_eq!(pipewire(&g), None);
        assert_eq!(stream_properties(&g), None);
        assert_eq!(pulse(&g), None);
        assert_eq!(alsa_block(&g, &BTreeMap::new()), None);
        assert_eq!(wireplumber_05(&BTreeMap::new()), None);
        assert_eq!(wireplumber_04(&BTreeMap::new()), None);
    }

    #[test]
    fn pulse_alternate_rate() {
        let g = |rate, allowed: &[u32]| GlobalSaved {
            rate,
            allowed_rates: Some(allowed.to_vec()),
            ..Default::default()
        };
        let p = pulse(&g(Some(48_000), &[44_100, 48_000])).unwrap();
        assert!(p.contains("default-sample-rate = 48000\nalternate-sample-rate = 44100\navoid-resampling = yes\n"), "{p}");
        let p = pulse(&g(None, &[44_100, 48_000])).unwrap();
        assert!(
            p.contains("default-sample-rate = 44100\nalternate-sample-rate = 48000\n"),
            "{p}"
        );
        let p = pulse(&g(None, &[48_000])).unwrap();
        assert!(
            p.contains("default-sample-rate = 48000\nalternate-sample-rate = 48000\n"),
            "{p}"
        );
    }

    #[test]
    fn block_is_added_replaced_and_removed_byte_for_byte() {
        let user = "pcm.!default {\n  type hw\n  card 1\n}\n# user comment";
        let one = splice_block(Some(user), Some("defaults.pcm.dmix.rate 96000\n"))
            .unwrap()
            .unwrap();
        assert!(one.starts_with(user));
        assert!(one.contains("\n# >>> ratewise >>>"));
        let two = splice_block(Some(&one), Some("defaults.pcm.dmix.rate 48000\n"))
            .unwrap()
            .unwrap();
        assert_eq!(two.matches(BLOCK_START).count(), 1);
        assert!(two.contains("rate 48000") && !two.contains("rate 96000"));
        let back = splice_block(Some(&two), None).unwrap().unwrap();
        assert_eq!(
            back,
            format!("{user}\n"),
            "only the newline Ratewise added remains"
        );

        let only = splice_block(None, Some("x 1\n")).unwrap().unwrap();
        assert_eq!(splice_block(Some(&only), None).unwrap(), None);
        assert_eq!(splice_block(None, None).unwrap(), None);
        assert_eq!(
            splice_block(Some(user), None).unwrap().as_deref(),
            Some(user)
        );
    }

    #[test]
    fn block_in_the_middle_keeps_both_sides() {
        let text = format!("before\n{BLOCK_START} x\nold\n{BLOCK_END}\nafter\n");
        let out = splice_block(Some(&text), Some("new\n")).unwrap().unwrap();
        assert!(out.starts_with("before\n# >>> ratewise >>>"));
        assert!(out.ends_with("new\n# <<< ratewise <<<\nafter\n"));
        assert_eq!(
            overrides_after_block(
                "x\n# >>> ratewise >>>\n# <<< ratewise <<<\n defaults.pcm.dmix.rate 44100\n",
                &["defaults.pcm.dmix.rate"]
            ),
            vec!["defaults.pcm.dmix.rate 44100"]
        );
    }

    #[test]
    fn damaged_block_is_refused() {
        let text = format!("a\n{BLOCK_START}\nb\n");
        assert_eq!(splice_block(Some(&text), Some("c\n")), Err(DamagedBlock));
    }

    #[test]
    fn card_ids_for_config_keys() {
        assert!(alsa_key_safe("PCH"));
        assert!(alsa_key_safe("Lite_2"));
        assert!(!alsa_key_safe("My card"));
        assert!(!alsa_key_safe(""));
    }

    #[test]
    fn fingerprint_is_stable() {
        assert_eq!(fingerprint(""), "cbf29ce484222325");
        assert_ne!(fingerprint("a"), fingerprint("b"));
    }
}
