//! LE Audio's LC3 presets as PipeWire 1.6 knows them (BAP v1.0.1 Table 5.2, from
//! `spa/plugins/bluez5/bap-codec-lc3.c`), and the limits on the link settings PipeWire
//! lets you set by hand. Pure data.

use crate::core::model::khz;

/// One unicast QoS preset, e.g. `48_5_1`: 48 kHz, configuration 5, low latency.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lc3Preset {
    pub name: &'static str,
    pub rate: u32,
    /// The nominal frame duration: 7500 or 10000 µs.
    pub frame_us: u32,
    /// Bytes per frame and channel.
    pub octets: u16,
    pub retransmissions: u8,
    /// Maximum transport latency.
    pub latency_ms: u16,
    /// The high-reliability (`_2`) presets; the others are low-latency (`_1`).
    pub reliable: bool,
}

const fn p(
    name: &'static str,
    rate: u32,
    octets: u16,
    retransmissions: u8,
    latency_ms: u16,
    reliable: bool,
) -> Lc3Preset {
    // Odd configurations use 7.5 ms frames, even ones 10 ms.
    let b = name.as_bytes();
    let mut i = 0;
    while b[i] != b'_' {
        i += 1;
    }
    let frame_us = if (b[i + 1] - b'0') % 2 == 1 {
        7500
    } else {
        10_000
    };
    Lc3Preset {
        name,
        rate,
        frame_us,
        octets,
        retransmissions,
        latency_ms,
        reliable,
    }
}

/// PipeWire's unicast presets, in its order.
pub const PRESETS: [Lc3Preset; 32] = [
    p("8_1_1", 8000, 26, 2, 8, false),
    p("8_2_1", 8000, 30, 2, 10, false),
    p("16_1_1", 16_000, 30, 2, 8, false),
    p("16_2_1", 16_000, 40, 2, 10, false),
    p("24_1_1", 24_000, 45, 2, 8, false),
    p("24_2_1", 24_000, 60, 2, 10, false),
    p("32_1_1", 32_000, 60, 2, 8, false),
    p("32_2_1", 32_000, 80, 2, 10, false),
    p("441_1_1", 44_100, 97, 5, 24, false),
    p("441_2_1", 44_100, 130, 5, 31, false),
    p("48_1_1", 48_000, 75, 5, 15, false),
    p("48_2_1", 48_000, 100, 5, 20, false),
    p("48_3_1", 48_000, 90, 5, 15, false),
    p("48_4_1", 48_000, 120, 5, 20, false),
    p("48_5_1", 48_000, 117, 5, 15, false),
    p("48_6_1", 48_000, 155, 5, 20, false),
    p("8_1_2", 8000, 26, 13, 75, true),
    p("8_2_2", 8000, 30, 13, 95, true),
    p("16_1_2", 16_000, 30, 13, 75, true),
    p("16_2_2", 16_000, 40, 13, 95, true),
    p("24_1_2", 24_000, 45, 13, 75, true),
    p("24_2_2", 24_000, 60, 13, 95, true),
    p("32_1_2", 32_000, 60, 13, 75, true),
    p("32_2_2", 32_000, 80, 13, 95, true),
    p("441_1_2", 44_100, 97, 13, 80, true),
    p("441_2_2", 44_100, 130, 13, 85, true),
    p("48_1_2", 48_000, 75, 13, 75, true),
    p("48_2_2", 48_000, 100, 13, 95, true),
    p("48_3_2", 48_000, 90, 13, 75, true),
    p("48_4_2", 48_000, 120, 13, 100, true),
    p("48_5_2", 48_000, 117, 13, 75, true),
    p("48_6_2", 48_000, 155, 13, 100, true),
];

/// The rates presets exist for, highest first.
pub const RATES: [u32; 6] = [48_000, 44_100, 32_000, 24_000, 16_000, 8000];

/// `bluez5.bap.latency` must lie in BAP's Max_Transport_Latency range.
pub const TRANSPORT_LATENCY_MS: std::ops::RangeInclusive<u16> = 5..=4000;
/// `bluez5.bap.delay` (Presentation_Delay) is 24 bits of µs.
pub const MAX_PRESENTATION_DELAY_US: u32 = 0xFF_FFFF;
/// What PipeWire asks for when it isn't set.
pub const DEFAULT_PRESENTATION_DELAY_US: u32 = 40_000;

impl Lc3Preset {
    /// The preset's number within its rate: `5` for `48_5_1`.
    pub fn config(&self) -> &'static str {
        self.name.split('_').nth(1).unwrap_or("")
    }

    /// `bluez5.bap.force-target-latency`'s value that keeps this preset.
    pub fn target(&self) -> &'static str {
        if self.reliable {
            "high-reliability"
        } else {
            "low-latency"
        }
    }

    /// 44.1 kHz frames don't fit the 7.5 and 10 ms intervals, so those presets are framed.
    pub fn framed(&self) -> bool {
        self.rate == 44_100
    }

    /// Per channel. 44.1 kHz frames last 8.163 or 10.884 ms.
    pub fn kbps(&self) -> f64 {
        let mut frame_us = f64::from(self.frame_us);
        if self.rate == 44_100 {
            frame_us *= 48_000.0 / 44_100.0;
        }
        f64::from(self.octets) * 8.0 * 1000.0 / frame_us
    }

    /// "48 kHz, 7.5 ms frames, 124.8 kbps per channel, low latency: up to 15 ms on the
    /// link, 5 retransmissions"
    pub fn describe(&self) -> String {
        format!(
            "{} kHz, {} frames, {} kbps per channel, {}: up to {} ms on the link, {} \
             retransmissions",
            khz(self.rate),
            ms_text(self.frame_us),
            one_decimal(self.kbps()),
            if self.reliable {
                "high reliability"
            } else {
                "low latency"
            },
            self.latency_ms,
            self.retransmissions
        )
    }
}

/// A preset by name. Takes `48_5_1`, `48-5-1` and `44.1_1_1` for `441_1_1`.
pub fn find(name: &str) -> Option<&'static Lc3Preset> {
    let n = name
        .trim()
        .to_ascii_lowercase()
        .replace('-', "_")
        .replacen("44.1_", "441_", 1);
    PRESETS.iter().find(|p| p.name == n)
}

/// 40000 → "40 ms", 7500 → "7.5 ms".
pub fn ms_text(us: u32) -> String {
    format!("{} ms", one_decimal(f64::from(us) / 1000.0))
}

/// The input form of a delay in µs: 40000 → "40", 7500 → "7.5".
pub fn ms_number(us: u32) -> String {
    let ms = f64::from(us) / 1000.0;
    let s = format!("{ms:.3}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn one_decimal(x: f64) -> String {
    let s = format!("{x:.1}");
    s.strip_suffix(".0").map(str::to_string).unwrap_or(s)
}

/// "40", "40ms", "7.5 ms", "40000us" → µs.
pub fn parse_delay(s: &str) -> Option<u32> {
    let t = s.trim().to_ascii_lowercase();
    let (num, scale) = if let Some(n) = t.strip_suffix("us").or_else(|| t.strip_suffix("µs")) {
        (n, 1.0)
    } else {
        (t.strip_suffix("ms").unwrap_or(&t), 1000.0)
    };
    let v: f64 = num.trim().parse().ok()?;
    let us = (v * scale).round();
    (v.is_finite() && us >= 0.0 && us <= f64::from(u32::MAX)).then_some(us as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_match_bap() {
        let p = find("48_5_1").unwrap();
        assert_eq!((p.frame_us, p.octets, p.config()), (7500, 117, "5"));
        assert_eq!(one_decimal(p.kbps()), "124.8");
        assert_eq!(one_decimal(find("48-6-2").unwrap().kbps()), "124");
        assert_eq!(find("48_2_1").unwrap().frame_us, 10_000);
        // BAP Table 3.11: 441_1 is 95.06 kbps.
        assert_eq!(one_decimal(find("44.1_1_1").unwrap().kbps()), "95.1");
        assert!(find("441_2_2").unwrap().framed());
        assert!(find("48_7_1").is_none());
        assert_eq!(
            p.describe(),
            "48 kHz, 7.5 ms frames, 124.8 kbps per channel, low latency: up to 15 ms on the \
             link, 5 retransmissions"
        );
    }

    #[test]
    fn delays_parse_in_ms() {
        assert_eq!(parse_delay("40"), Some(40_000));
        assert_eq!(parse_delay("7.5 ms"), Some(7500));
        assert_eq!(parse_delay("25000us"), Some(25_000));
        assert_eq!(parse_delay("-1"), None);
        assert_eq!(parse_delay("soon"), None);
        assert_eq!(ms_number(7500), "7.5");
        assert_eq!(ms_number(40_000), "40");
        assert_eq!(ms_text(40_000), "40 ms");
    }
}
