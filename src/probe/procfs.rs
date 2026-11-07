//! Parses `/proc/asound` (spec §3.3): cards, PCM info, USB stream files and the
//! running `hw_params`.
//!
//! The parsers are pure functions over text and never panic: unexpected input
//! is an error. File access goes through [`FileTree`], so fixtures and the real
//! `/proc/asound` share one code path.

use std::io;
use std::path::PathBuf;

use crate::core::facts::{ProcAsound, ProcCard, ProcPcm, UsbAltset, UsbStream, UsbStreamDirection};
use crate::core::model::{Direction, RateSet, Running, SampleFormat, StreamParams};

/// Read-only access to a directory tree, addressed with `/`-separated relative paths.
pub trait FileTree: Send + Sync {
    fn read_to_string(&self, rel: &str) -> io::Result<String>;
    fn list_dir(&self, rel: &str) -> io::Result<Vec<String>>;
    /// Where this tree lives, for messages.
    fn describe(&self) -> String;
}

/// A real directory: `/proc/asound`, or `<fixture>/proc-asound`.
pub struct DirTree {
    root: PathBuf,
}

impl DirTree {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
}

impl FileTree for DirTree {
    fn read_to_string(&self, rel: &str) -> io::Result<String> {
        std::fs::read_to_string(self.root.join(rel))
    }

    fn list_dir(&self, rel: &str) -> io::Result<Vec<String>> {
        let mut names = Vec::new();
        for entry in std::fs::read_dir(self.root.join(rel))? {
            names.push(entry?.file_name().to_string_lossy().into_owned());
        }
        names.sort();
        Ok(names)
    }

    fn describe(&self) -> String {
        self.root.display().to_string()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProcfsError {
    #[error(
        "{path} doesn't exist, so the kernel isn't showing any ALSA sound cards. This is normal inside containers and WSL. On a desktop it means the sound drivers aren't loaded."
    )]
    NotPresent { path: String },
    #[error("couldn't read {path}: {source}")]
    Unreadable {
        path: String,
        #[source]
        source: io::Error,
    },
    #[error(
        "{path} isn't in the format Ratewise expects ({reason}). Please report this with `ratewise capture-fixture`."
    )]
    Parse { path: String, reason: String },
}

/// A parser rejected its input.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ParseError(pub String);

/// Read and parse the whole tree. Only a missing or unparseable `cards` file is fatal;
/// anything unreadable below it is skipped and logged.
pub fn probe(tree: &dyn FileTree) -> Result<ProcAsound, ProcfsError> {
    let root = tree.describe();
    let cards_text = tree.read_to_string("cards").map_err(|e| match e.kind() {
        io::ErrorKind::NotFound => ProcfsError::NotPresent { path: root.clone() },
        _ => ProcfsError::Unreadable {
            path: format!("{root}/cards"),
            source: e,
        },
    })?;
    let lines = parse_cards(&cards_text).map_err(|e| ProcfsError::Parse {
        path: format!("{root}/cards"),
        reason: e.0,
    })?;

    let mut cards = Vec::new();
    for line in lines {
        cards.push(probe_card(tree, line));
    }
    Ok(ProcAsound { cards })
}

fn probe_card(tree: &dyn FileTree, line: CardLine) -> ProcCard {
    let dir = format!("card{}", line.number);
    // `cardN/id` has the full id; the `cards` column is padded and may be truncated.
    let id = tree
        .read_to_string(&format!("{dir}/id"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or(line.id);
    let entries = tree.list_dir(&dir).unwrap_or_else(|e| {
        tracing::debug!(%dir, error = %e, "can't list card directory");
        Vec::new()
    });

    let mut pcms = Vec::new();
    let mut usb_streams = Vec::new();
    for entry in &entries {
        if let Some((device, direction)) = parse_pcm_dir_name(entry) {
            pcms.push(probe_pcm(
                tree,
                &format!("{dir}/{entry}"),
                device,
                direction,
            ));
        } else if let Some(n) = entry
            .strip_prefix("stream")
            .and_then(|n| n.parse::<u32>().ok())
        {
            let path = format!("{dir}/{entry}");
            match tree.read_to_string(&path).map(|t| parse_usb_stream(&t)) {
                Ok(Ok(mut stream)) => {
                    stream.device = n;
                    usb_streams.push(stream);
                }
                Ok(Err(e)) => tracing::warn!(%path, error = %e, "unparseable USB stream file"),
                Err(e) => tracing::debug!(%path, error = %e, "can't read USB stream file"),
            }
        }
    }
    pcms.sort_by_key(|p| (p.device, p.direction));
    usb_streams.sort_by_key(|s| s.device);

    ProcCard {
        number: line.number,
        id,
        driver: line.driver,
        name: line.name,
        long_name: line.long_name,
        pcms,
        usb_streams,
    }
}

fn probe_pcm(tree: &dyn FileTree, dir: &str, device: u32, direction: Direction) -> ProcPcm {
    let info = tree
        .read_to_string(&format!("{dir}/info"))
        .ok()
        .and_then(|t| parse_pcm_info(&t).ok());
    let (id, name) = match info {
        Some(i) => (i.id, i.name),
        None => (format!("PCM {device}"), format!("PCM {device}")),
    };

    // A PCM can have several subdevices; report the first active one.
    let mut running = None;
    let subs = tree.list_dir(dir).unwrap_or_default();
    for sub in subs.iter().filter(|s| s.starts_with("sub")) {
        let path = format!("{dir}/{sub}/hw_params");
        match tree.read_to_string(&path).map(|t| parse_hw_params(&t)) {
            Ok(Ok(r @ Running::Active(_))) => {
                running = Some(r);
                break;
            }
            Ok(Ok(Running::Idle)) => running = running.or(Some(Running::Idle)),
            Ok(Err(e)) => tracing::warn!(%path, error = %e, "unparseable hw_params"),
            Err(e) => tracing::debug!(%path, error = %e, "can't read hw_params"),
        }
    }

    ProcPcm {
        device,
        direction,
        id,
        name,
        running,
    }
}

/// One card from `/proc/asound/cards`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardLine {
    pub number: u32,
    pub id: String,
    pub driver: String,
    pub name: String,
    pub long_name: String,
}

/// Parses `/proc/asound/cards`:
/// ```text
///  0 [PCH            ]: HDA-Intel - HDA Intel PCH
///                       HDA Intel PCH at 0xf7f10000 irq 32
/// ```
pub fn parse_cards(text: &str) -> Result<Vec<CardLine>, ParseError> {
    let mut cards: Vec<CardLine> = Vec::new();
    let mut saw_no_cards = false;
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if t.starts_with("---") {
            // "--- no soundcards ---"
            saw_no_cards = true;
            continue;
        }
        match parse_card_line(t) {
            Some(card) => cards.push(card),
            None => match cards.last_mut() {
                Some(last) if last.long_name.is_empty() => last.long_name = t.to_string(),
                _ => tracing::debug!(line = t, "ignoring unexpected line in cards"),
            },
        }
    }
    if cards.is_empty() && !saw_no_cards && !text.trim().is_empty() {
        return Err(ParseError("no card entries found".into()));
    }
    Ok(cards)
}

fn parse_card_line(t: &str) -> Option<CardLine> {
    let (num, rest) = t.split_once(char::is_whitespace)?;
    let number = num.parse().ok()?;
    let rest = rest.trim_start().strip_prefix('[')?;
    let (id, rest) = rest.split_once(']')?;
    let rest = rest.trim_start().strip_prefix(':')?.trim();
    let (driver, name) = rest.split_once(" - ").unwrap_or((rest, ""));
    Some(CardLine {
        number,
        id: id.trim().to_string(),
        driver: driver.trim().to_string(),
        name: name.trim().to_string(),
        long_name: String::new(),
    })
}

/// `pcm0p` → `(0, Playback)`, `pcm12c` → `(12, Capture)`.
pub fn parse_pcm_dir_name(name: &str) -> Option<(u32, Direction)> {
    let rest = name.strip_prefix("pcm")?;
    let (num, direction) = if let Some(n) = rest.strip_suffix('p') {
        (n, Direction::Playback)
    } else {
        (rest.strip_suffix('c')?, Direction::Capture)
    };
    Some((num.parse().ok()?, direction))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PcmInfo {
    pub id: String,
    pub name: String,
    pub direction: Option<Direction>,
}

/// Parses `cardN/pcmXp/info` (`key: value` lines).
pub fn parse_pcm_info(text: &str) -> Result<PcmInfo, ParseError> {
    let mut id = None;
    let mut name = None;
    let mut direction = None;
    for (k, v) in key_values(text) {
        match k {
            "id" => id = Some(v.to_string()),
            "name" => name = Some(v.to_string()),
            "stream" => {
                direction = match v {
                    "PLAYBACK" => Some(Direction::Playback),
                    "CAPTURE" => Some(Direction::Capture),
                    _ => None,
                }
            }
            _ => {}
        }
    }
    match (id, name) {
        (Some(id), Some(name)) => Ok(PcmInfo {
            id,
            name,
            direction,
        }),
        (Some(id), None) => Ok(PcmInfo {
            name: id.clone(),
            id,
            direction,
        }),
        (None, Some(name)) => Ok(PcmInfo {
            id: name.clone(),
            name,
            direction,
        }),
        (None, None) => Err(ParseError("no id or name".into())),
    }
}

/// Parses `sub*/hw_params`: either `closed`, or
/// ```text
/// access: MMAP_INTERLEAVED
/// format: S32_LE
/// subformat: STD
/// channels: 2
/// rate: 48000 (48000/1)
/// period_size: 1024
/// buffer_size: 4096
/// ```
pub fn parse_hw_params(text: &str) -> Result<Running, ParseError> {
    let t = text.trim();
    if t == "closed" {
        return Ok(Running::Idle);
    }
    let mut rate = None;
    let mut params = StreamParams {
        rate: 0,
        format: None,
        channels: None,
        period_frames: None,
        buffer_frames: None,
    };
    for (k, v) in key_values(t) {
        let first_number = || {
            v.split_whitespace()
                .next()
                .and_then(|n| n.parse::<u32>().ok())
        };
        match k {
            "rate" => rate = first_number(),
            "format" => params.format = Some(SampleFormat::from_alsa(v)),
            "channels" => params.channels = first_number(),
            "period_size" => params.period_frames = first_number(),
            "buffer_size" => params.buffer_frames = first_number(),
            _ => {}
        }
    }
    params.rate = rate.ok_or_else(|| ParseError("no rate".into()))?;
    Ok(Running::Active(params))
}

/// Parses a USB `streamN` file (one per PCM device).
pub fn parse_usb_stream(text: &str) -> Result<UsbStream, ParseError> {
    let mut stream = UsbStream::default();
    let mut current: Option<(Direction, UsbStreamDirection)> = None;
    let mut altset: Option<UsbAltset> = None;

    fn finish(
        stream: &mut UsbStream,
        current: &mut Option<(Direction, UsbStreamDirection)>,
        altset: &mut Option<UsbAltset>,
    ) {
        if let Some((_, dir)) = current.as_mut()
            && let Some(a) = altset.take()
        {
            dir.altsets.push(a);
        }
        if let Some((d, dir)) = current.take() {
            match d {
                Direction::Capture => stream.capture = Some(dir),
                _ => stream.playback = Some(dir),
            }
        }
    }

    for line in text.lines() {
        let t = line.trim();
        match t {
            "Playback:" | "Capture:" => {
                finish(&mut stream, &mut current, &mut altset);
                let d = if t == "Playback:" {
                    Direction::Playback
                } else {
                    Direction::Capture
                };
                current = Some((d, UsbStreamDirection::default()));
                continue;
            }
            _ => {}
        }
        let Some((_, dir)) = current.as_mut() else {
            continue;
        };
        if let Some(v) = t.strip_prefix("Status:") {
            dir.running = Some(v.trim() == "Running");
        } else if let Some(v) = t.strip_prefix("Momentary freq =") {
            dir.momentary_freq = v.split_whitespace().next().and_then(|n| n.parse().ok());
        } else if let Some(v) = t.strip_prefix("Interface ").filter(|_| !t.contains('=')) {
            if let Some(a) = altset.take() {
                dir.altsets.push(a);
            }
            altset = Some(UsbAltset {
                interface: v.trim().parse().unwrap_or(0),
                ..Default::default()
            });
        } else if let Some(a) = altset.as_mut() {
            if let Some(v) = t.strip_prefix("Altset ").filter(|_| !t.contains('=')) {
                a.altset = v.trim().parse().unwrap_or(0);
            } else if let Some(v) = t.strip_prefix("Format:") {
                a.formats = v.split_whitespace().map(SampleFormat::from_alsa).collect();
            } else if let Some(v) = t.strip_prefix("Channels:") {
                a.channels = v.trim().parse().ok();
            } else if let Some(v) = t.strip_prefix("Rates:") {
                a.rates = parse_rates(v);
            } else if let Some(v) = t.strip_prefix("Bits:") {
                a.bits = v.trim().parse().ok();
            }
        }
    }
    finish(&mut stream, &mut current, &mut altset);

    if stream.playback.is_none() && stream.capture.is_none() {
        return Err(ParseError("no Playback or Capture section".into()));
    }
    Ok(stream)
}

/// `44100, 48000, 96000` or `8000 - 48000 (continuous)`.
fn parse_rates(v: &str) -> Option<RateSet> {
    let v = v.trim();
    if let Some((lo, hi)) = v.split_once(" - ") {
        let lo = lo.trim().parse().ok()?;
        let hi = hi.split_whitespace().next()?.parse().ok()?;
        return Some(RateSet::range(lo, hi));
    }
    let rates: Vec<u32> = v.split(',').filter_map(|r| r.trim().parse().ok()).collect();
    (!rates.is_empty()).then(|| RateSet::discrete(rates))
}

fn key_values(text: &str) -> impl Iterator<Item = (&str, &str)> {
    text.lines().filter_map(|l| {
        let (k, v) = l.split_once(':')?;
        Some((k.trim(), v.trim()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cards_file() {
        let text = " 0 [PCH            ]: HDA-Intel - HDA Intel PCH
                      HDA Intel PCH at 0xf7f10000 irq 32
 1 [NVidia         ]: HDA-Intel - HDA NVidia
                      HDA NVidia at 0xf7080000 irq 17
10 [D50s           ]: USB-Audio - D50s
                      Topping D50s at usb-0000:00:14.0-3, high speed
";
        let cards = parse_cards(text).unwrap();
        assert_eq!(cards.len(), 3);
        assert_eq!(cards[0].id, "PCH");
        assert_eq!(cards[0].driver, "HDA-Intel");
        assert_eq!(cards[0].name, "HDA Intel PCH");
        assert_eq!(cards[0].long_name, "HDA Intel PCH at 0xf7f10000 irq 32");
        assert_eq!(cards[2].number, 10);
        assert_eq!(
            cards[2].long_name,
            "Topping D50s at usb-0000:00:14.0-3, high speed"
        );
    }

    #[test]
    fn no_soundcards() {
        assert_eq!(parse_cards("--- no soundcards ---\n").unwrap(), vec![]);
        assert_eq!(parse_cards("").unwrap(), vec![]);
        assert!(parse_cards("hello world\n").is_err());
    }

    #[test]
    fn pcm_dir_names() {
        assert_eq!(parse_pcm_dir_name("pcm0p"), Some((0, Direction::Playback)));
        assert_eq!(parse_pcm_dir_name("pcm12c"), Some((12, Direction::Capture)));
        assert_eq!(parse_pcm_dir_name("pcm"), None);
        assert_eq!(parse_pcm_dir_name("pcmp"), None);
        assert_eq!(parse_pcm_dir_name("pcm0x"), None);
    }

    #[test]
    fn hw_params_closed_and_running() {
        assert_eq!(parse_hw_params("closed\n").unwrap(), Running::Idle);
        let r = parse_hw_params(
            "access: MMAP_INTERLEAVED\nformat: S32_LE\nsubformat: STD\nchannels: 2\nrate: 96000 (96000/1)\nperiod_size: 1024\nbuffer_size: 8192\n",
        )
        .unwrap();
        let Running::Active(p) = r else {
            panic!("expected active")
        };
        assert_eq!(p.rate, 96_000);
        assert_eq!(p.format, Some(SampleFormat::S32Le));
        assert_eq!(p.channels, Some(2));
        assert_eq!(p.period_frames, Some(1024));
        assert_eq!(p.buffer_frames, Some(8192));
        assert!(parse_hw_params("garbage").is_err());
    }

    #[test]
    fn usb_stream_with_two_altsets() {
        let text = "Topping D50s at usb-0000:00:14.0-3, high speed : USB Audio

Playback:
  Status: Running
    Interface = 1
    Altset = 2
    Packet Size = 104
    Momentary freq = 96000 Hz (0xc.0000)
    Feedback Format = 16.16
  Interface 1
    Altset 1
    Format: S24_3LE
    Channels: 2
    Endpoint: 0x01 (1 OUT) (ASYNC)
    Rates: 44100, 48000, 88200, 96000
    Data packet interval: 125 us
    Bits: 24
  Interface 1
    Altset 2
    Format: S32_LE
    Channels: 2
    Endpoint: 0x01 (1 OUT) (ASYNC)
    Rates: 44100, 48000, 88200, 96000, 176400, 192000
    Bits: 32
";
        let s = parse_usb_stream(text).unwrap();
        assert!(s.capture.is_none());
        let p = s.playback.unwrap();
        assert_eq!(p.running, Some(true));
        assert_eq!(p.momentary_freq, Some(96_000));
        assert_eq!(p.altsets.len(), 2);
        assert_eq!(p.altsets[0].formats, vec![SampleFormat::S24_3Le]);
        assert_eq!(p.altsets[0].bits, Some(24));
        assert_eq!(p.altsets[1].altset, 2);
        assert_eq!(
            p.altsets[1].rates,
            Some(RateSet::discrete([
                44_100, 48_000, 88_200, 96_000, 176_400, 192_000
            ]))
        );
    }

    #[test]
    fn usb_stream_continuous_capture() {
        let text = "USB PnP Sound Device at usb-0000:00:14.0-2, full speed : USB Audio

Capture:
  Status: Stop
  Interface 1
    Altset 1
    Format: S16_LE
    Channels: 1
    Endpoint: 0x82 (2 IN) (ADAPTIVE)
    Rates: 8000 - 48000 (continuous)
";
        let s = parse_usb_stream(text).unwrap();
        let c = s.capture.unwrap();
        assert_eq!(c.running, Some(false));
        assert_eq!(c.altsets[0].rates, Some(RateSet::range(8_000, 48_000)));
        assert!(parse_usb_stream("nothing useful").is_err());
    }

    #[test]
    fn pcm_info() {
        let i = parse_pcm_info(
            "card: 0\ndevice: 3\nsubdevice: 0\nstream: PLAYBACK\nid: HDMI 0\nname: HDMI 0\n",
        )
        .unwrap();
        assert_eq!(i.id, "HDMI 0");
        assert_eq!(i.direction, Some(Direction::Playback));
        assert!(parse_pcm_info("").is_err());
    }
}
