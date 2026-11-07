//! Direct kernel probe (spec §3.2), without libasound: open each
//! `/dev/snd/pcmC{c}D{d}{p|c}` and ask the driver with `SNDRV_PCM_IOCTL_HW_REFINE`
//! which formats, rates, channel counts and period/buffer sizes it accepts.
//!
//! `EBUSY` is normal: the sound server has the device open. Those devices keep
//! the PipeWire and /proc/asound view.

use std::io;
use std::path::{Path, PathBuf};

use rustix::fd::OwnedFd;
use rustix::fs::{Mode, OFlags};
use rustix::io::Errno;
use rustix::ioctl::{Updater, opcode};

use crate::core::facts::{KernelCaps, KernelOutcome, KernelPcmProbe};
use crate::core::model::{Direction, FrameRange, RateSet, STANDARD_RATES, SampleFormat};

// ---- Kernel ABI, from include/uapi/sound/asound.h --------------------------------

/// `SNDRV_MASK_MAX` (256 bits) as 32-bit words.
const MASK_WORDS: usize = 8;

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SndMask {
    pub bits: [u32; MASK_WORDS],
}

/// `struct snd_interval`. The C bitfields `openmin:1, openmax:1, integer:1, empty:1`
/// share one `unsigned int`, low bit first.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SndInterval {
    pub min: u32,
    pub max: u32,
    pub flags: u32,
}

const INTERVAL_OPENMIN: u32 = 1 << 0;
const INTERVAL_OPENMAX: u32 = 1 << 1;
const INTERVAL_INTEGER: u32 = 1 << 2;
const INTERVAL_EMPTY: u32 = 1 << 3;

/// `struct snd_pcm_hw_params`. Recent kernels split `reserved[64]` into
/// `sync[16]` + `reserved[48]`; the size and layout are unchanged.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SndPcmHwParams {
    pub flags: u32,
    pub masks: [SndMask; 3],
    pub mres: [SndMask; 5],
    pub intervals: [SndInterval; 12],
    pub ires: [SndInterval; 9],
    pub rmask: u32,
    pub cmask: u32,
    pub info: u32,
    pub msbits: u32,
    pub rate_num: u32,
    pub rate_den: u32,
    pub fifo_size: std::ffi::c_ulong,
    pub reserved: [u8; 64],
}

// Mask indices (SNDRV_PCM_HW_PARAM_*, from FIRST_MASK = 0).
const MASK_FORMAT: usize = 1;
// Interval indices (SNDRV_PCM_HW_PARAM_* minus FIRST_INTERVAL = 8).
const IV_CHANNELS: usize = 10 - 8;
const IV_RATE: usize = 11 - 8;
const IV_PERIOD_SIZE: usize = 13 - 8;
const IV_BUFFER_SIZE: usize = 17 - 8;

/// `SNDRV_PCM_IOCTL_HW_REFINE = _IOWR('A', 0x10, struct snd_pcm_hw_params)`.
pub const HW_REFINE: rustix::ioctl::Opcode = opcode::read_write::<SndPcmHwParams>(b'A', 0x10);

/// `SNDRV_PCM_FORMAT_*` names by bit index (gaps are reserved values).
const FORMAT_NAMES: [&str; 53] = [
    "S8",
    "U8",
    "S16_LE",
    "S16_BE",
    "U16_LE",
    "U16_BE",
    "S24_LE",
    "S24_BE",
    "U24_LE",
    "U24_BE",
    "S32_LE",
    "S32_BE",
    "U32_LE",
    "U32_BE",
    "FLOAT_LE",
    "FLOAT_BE",
    "FLOAT64_LE",
    "FLOAT64_BE",
    "IEC958_SUBFRAME_LE",
    "IEC958_SUBFRAME_BE",
    "MU_LAW",
    "A_LAW",
    "IMA_ADPCM",
    "MPEG",
    "GSM",
    "S20_LE",
    "S20_BE",
    "U20_LE",
    "U20_BE",
    "",
    "",
    "SPECIAL",
    "S24_3LE",
    "S24_3BE",
    "U24_3LE",
    "U24_3BE",
    "S20_3LE",
    "S20_3BE",
    "U20_3LE",
    "U20_3BE",
    "S18_3LE",
    "S18_3BE",
    "U18_3LE",
    "U18_3BE",
    "G723_24",
    "G723_24_1B",
    "G723_40",
    "G723_40_1B",
    "DSD_U8",
    "DSD_U16_LE",
    "DSD_U32_LE",
    "DSD_U16_BE",
    "DSD_U32_BE",
];

impl SndPcmHwParams {
    /// Everything allowed, as alsa-lib's `snd_pcm_hw_params_any` starts from.
    pub fn any() -> Self {
        let any = SndInterval {
            min: 0,
            max: u32::MAX,
            flags: 0,
        };
        let empty_mask = SndMask {
            bits: [0; MASK_WORDS],
        };
        Self {
            flags: 0,
            masks: [SndMask {
                bits: [u32::MAX; MASK_WORDS],
            }; 3],
            mres: [empty_mask; 5],
            intervals: [any; 12],
            ires: [SndInterval {
                min: 0,
                max: 0,
                flags: 0,
            }; 9],
            rmask: u32::MAX,
            cmask: 0,
            info: u32::MAX,
            msbits: 0,
            rate_num: 0,
            rate_den: 0,
            fifo_size: 0,
            reserved: [0; 64],
        }
    }

    /// Pin interval `idx` to exactly `value`.
    fn set_exact(&mut self, idx: usize, value: u32) {
        self.intervals[idx] = SndInterval {
            min: value,
            max: value,
            flags: INTERVAL_INTEGER,
        };
    }

    /// Inclusive integer bounds of interval `idx`, or `None` if empty.
    pub fn bounds(&self, idx: usize) -> Option<(u32, u32)> {
        let i = self.intervals[idx];
        if i.flags & INTERVAL_EMPTY != 0 {
            return None;
        }
        let lo = if i.flags & INTERVAL_OPENMIN != 0 {
            i.min.saturating_add(1)
        } else {
            i.min
        };
        let hi = if i.flags & INTERVAL_OPENMAX != 0 {
            i.max.saturating_sub(1)
        } else {
            i.max
        };
        (lo <= hi).then_some((lo, hi))
    }

    fn formats(&self) -> Vec<SampleFormat> {
        let mask = &self.masks[MASK_FORMAT];
        FORMAT_NAMES
            .iter()
            .enumerate()
            .filter(|(i, name)| !name.is_empty() && mask.bits[i / 32] & (1 << (i % 32)) != 0)
            .map(|(_, name)| SampleFormat::from_alsa(name))
            .collect()
    }
}

// ---- Probing ----------------------------------------------------------------------

/// Something that answers `HW_REFINE`: a real PCM device, or a simulation in tests.
pub trait HwRefine {
    fn refine(&mut self, params: &mut SndPcmHwParams) -> io::Result<()>;
}

/// An open PCM device node.
pub struct PcmDevice {
    fd: OwnedFd,
}

impl PcmDevice {
    /// Opens without blocking, so a device held by the sound server fails fast with `EBUSY`.
    pub fn open(path: &Path) -> Result<Self, Errno> {
        let fd = rustix::fs::open(
            path,
            OFlags::RDWR | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )?;
        Ok(Self { fd })
    }
}

impl HwRefine for PcmDevice {
    fn refine(&mut self, params: &mut SndPcmHwParams) -> io::Result<()> {
        // SAFETY: HW_REFINE is _IOWR with a `struct snd_pcm_hw_params`, whose layout
        // `SndPcmHwParams` reproduces (checked by the layout tests below), and the fd
        // is an open ALSA PCM device.
        unsafe {
            rustix::ioctl::ioctl(
                &self.fd,
                Updater::<{ HW_REFINE }, SndPcmHwParams>::new(params),
            )
        }
        .map_err(io::Error::from)
    }
}

/// Asks the driver what it can do. Rates are expanded to the standard rates
/// the driver accepts one by one, so gaps in a range show up.
pub fn probe_caps(dev: &mut dyn HwRefine) -> io::Result<KernelCaps> {
    let mut p = SndPcmHwParams::any();
    dev.refine(&mut p)?;

    let accepts = |dev: &mut dyn HwRefine, idx: usize, value: u32| {
        let mut q = SndPcmHwParams::any();
        q.set_exact(idx, value);
        dev.refine(&mut q).is_ok() && q.bounds(idx).is_some()
    };

    let rates = match p.bounds(IV_RATE) {
        Some((lo, hi)) => {
            let accepted: Vec<u32> = STANDARD_RATES
                .iter()
                .copied()
                .filter(|&r| (lo..=hi).contains(&r) && accepts(dev, IV_RATE, r))
                .collect();
            if accepted.is_empty() {
                RateSet::range(lo, hi)
            } else {
                RateSet::discrete(accepted)
            }
        }
        None => RateSet::discrete([]),
    };

    let channels = match p.bounds(IV_CHANNELS) {
        Some((lo, hi)) if hi - lo <= 32 => (lo..=hi)
            .filter(|&c| accepts(dev, IV_CHANNELS, c))
            .collect(),
        Some((lo, hi)) => vec![lo, hi],
        None => vec![],
    };

    let range = |idx| p.bounds(idx).map(|(min, max)| FrameRange { min, max });
    Ok(KernelCaps {
        formats: p.formats(),
        rates,
        channels,
        period_frames: range(IV_PERIOD_SIZE),
        buffer_frames: range(IV_BUFFER_SIZE),
    })
}

/// `pcmC0D3p` → `(0, 3, Playback)`.
pub fn parse_node_name(name: &str) -> Option<(u32, u32, Direction)> {
    let rest = name.strip_prefix("pcmC")?;
    let (card, rest) = rest.split_once('D')?;
    let (device, direction) = if let Some(d) = rest.strip_suffix('p') {
        (d, Direction::Playback)
    } else {
        (rest.strip_suffix('c')?, Direction::Capture)
    };
    Some((card.parse().ok()?, device.parse().ok()?, direction))
}

/// Names of PCM device nodes (`pcmC0D0p`, ...) in `dir`, sorted.
pub fn pcm_device_nodes(dir: &Path) -> io::Result<Vec<String>> {
    let mut names: Vec<String> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| parse_node_name(n).is_some())
        .collect();
    names.sort();
    Ok(names)
}

// ---- Sources ----------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum KernelError {
    #[error(
        "{0} doesn't exist, so Ratewise can't ask the hardware directly. In a container, pass the sound devices through (for example `--device /dev/snd`)."
    )]
    NoDevSnd(String),
    #[error("couldn't list {path}: {source}")]
    Unreadable {
        path: String,
        #[source]
        source: io::Error,
    },
    #[error("this fixture has no kernel.json, so there are no direct hardware readings")]
    NotInFixture,
    #[error("kernel.json in this fixture isn't valid: {0}")]
    BadFixture(String),
}

/// Where kernel probe results come from.
pub trait KernelSource: Send + Sync {
    fn probe(&self) -> Result<Vec<KernelPcmProbe>, KernelError>;
}

/// The real `/dev/snd`.
pub struct DevSnd {
    pub dir: PathBuf,
}

impl Default for DevSnd {
    fn default() -> Self {
        Self {
            dir: PathBuf::from("/dev/snd"),
        }
    }
}

impl KernelSource for DevSnd {
    fn probe(&self) -> Result<Vec<KernelPcmProbe>, KernelError> {
        let names = pcm_device_nodes(&self.dir).map_err(|e| match e.kind() {
            io::ErrorKind::NotFound => KernelError::NoDevSnd(self.dir.display().to_string()),
            _ => KernelError::Unreadable {
                path: self.dir.display().to_string(),
                source: e,
            },
        })?;
        Ok(names
            .iter()
            .filter_map(|name| {
                let (card, device, direction) = parse_node_name(name)?;
                let result = probe_node(&self.dir.join(name));
                tracing::debug!(%name, ?result, "kernel probe");
                Some(KernelPcmProbe {
                    card,
                    device,
                    direction,
                    result,
                })
            })
            .collect())
    }
}

fn probe_node(path: &Path) -> KernelOutcome {
    match PcmDevice::open(path) {
        Ok(mut dev) => match probe_caps(&mut dev) {
            Ok(caps) => KernelOutcome::Ok(caps),
            Err(e) => KernelOutcome::Failed {
                message: format!("the driver rejected the capability query ({e})"),
            },
        },
        Err(Errno::BUSY) => KernelOutcome::Busy,
        Err(Errno::ACCESS | Errno::PERM) => KernelOutcome::PermissionDenied,
        Err(e) => KernelOutcome::Failed {
            message: format!("couldn't open the device ({})", io::Error::from(e)),
        },
    }
}

/// `kernel.json` in a fixture directory.
pub struct KernelFile {
    pub path: PathBuf,
}

impl KernelSource for KernelFile {
    fn probe(&self) -> Result<Vec<KernelPcmProbe>, KernelError> {
        let text = std::fs::read_to_string(&self.path).map_err(|e| match e.kind() {
            io::ErrorKind::NotFound => KernelError::NotInFixture,
            _ => KernelError::BadFixture(e.to_string()),
        })?;
        serde_json::from_str(&text).map_err(|e| KernelError::BadFixture(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{offset_of, size_of};

    #[test]
    fn struct_layout_matches_the_kernel() {
        assert_eq!(size_of::<SndMask>(), 32);
        assert_eq!(size_of::<SndInterval>(), 12);
        assert_eq!(offset_of!(SndPcmHwParams, masks), 4);
        assert_eq!(offset_of!(SndPcmHwParams, mres), 100);
        assert_eq!(offset_of!(SndPcmHwParams, intervals), 260);
        assert_eq!(offset_of!(SndPcmHwParams, ires), 404);
        assert_eq!(offset_of!(SndPcmHwParams, rmask), 512);
        assert_eq!(offset_of!(SndPcmHwParams, rate_den), 532);
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn struct_size_and_ioctl_number_64bit() {
        assert_eq!(offset_of!(SndPcmHwParams, fifo_size), 536);
        assert_eq!(size_of::<SndPcmHwParams>(), 608);
        // The value in every 64-bit Linux build of alsa-lib (x86_64, aarch64).
        assert_eq!(HW_REFINE as u64, 0xC260_4110);
    }

    #[cfg(target_pointer_width = "32")]
    #[test]
    fn struct_size_and_ioctl_number_32bit() {
        assert_eq!(size_of::<SndPcmHwParams>(), 604);
        assert_eq!(HW_REFINE as u64, 0xC25C_4110);
    }

    #[test]
    fn node_names() {
        assert_eq!(
            parse_node_name("pcmC0D3p"),
            Some((0, 3, Direction::Playback))
        );
        assert_eq!(
            parse_node_name("pcmC12D0c"),
            Some((12, 0, Direction::Capture))
        );
        assert_eq!(parse_node_name("controlC0"), None);
        assert_eq!(parse_node_name("pcmC0D0x"), None);
        assert_eq!(parse_node_name("pcmCD0p"), None);
    }

    /// A pretend driver: intersects requests with what it supports, like the kernel.
    struct Simulated {
        formats: Vec<usize>,
        rates: Vec<u32>,
        channels: (u32, u32),
        period: (u32, u32),
        calls: usize,
    }

    impl HwRefine for Simulated {
        fn refine(&mut self, p: &mut SndPcmHwParams) -> io::Result<()> {
            self.calls += 1;
            let einval = || io::Error::from_raw_os_error(22);
            let mut mask = [0u32; MASK_WORDS];
            for &f in &self.formats {
                if p.masks[MASK_FORMAT].bits[f / 32] & (1 << (f % 32)) != 0 {
                    mask[f / 32] |= 1 << (f % 32);
                }
            }
            if mask.iter().all(|&w| w == 0) {
                return Err(einval());
            }
            p.masks[MASK_FORMAT].bits = mask;

            let r = p.intervals[IV_RATE];
            let ok: Vec<u32> = self
                .rates
                .iter()
                .copied()
                .filter(|x| (r.min..=r.max).contains(x))
                .collect();
            let (Some(&lo), Some(&hi)) = (ok.first(), ok.last()) else {
                return Err(einval());
            };
            p.intervals[IV_RATE] = SndInterval {
                min: lo,
                max: hi,
                flags: INTERVAL_INTEGER,
            };

            let c = p.intervals[IV_CHANNELS];
            let (lo, hi) = (c.min.max(self.channels.0), c.max.min(self.channels.1));
            if lo > hi {
                return Err(einval());
            }
            p.intervals[IV_CHANNELS] = SndInterval {
                min: lo,
                max: hi,
                flags: INTERVAL_INTEGER,
            };
            p.intervals[IV_PERIOD_SIZE] = SndInterval {
                min: self.period.0,
                max: self.period.1,
                flags: INTERVAL_INTEGER,
            };
            p.intervals[IV_BUFFER_SIZE] = SndInterval {
                min: self.period.0 * 2,
                max: self.period.1 * 4,
                flags: INTERVAL_INTEGER | INTERVAL_OPENMAX,
            };
            Ok(())
        }
    }

    #[test]
    fn probes_a_simulated_hda_codec() {
        // S16_LE (2), S32_LE (10), DSD_U32_BE (52 — lives in the second mask word).
        let mut dev = Simulated {
            formats: vec![2, 10, 52],
            rates: vec![44_100, 48_000, 96_000, 192_000],
            channels: (2, 8),
            period: (32, 4096),
            calls: 0,
        };
        let caps = probe_caps(&mut dev).unwrap();
        assert_eq!(
            caps.formats,
            vec![
                SampleFormat::S16Le,
                SampleFormat::S32Le,
                SampleFormat::Other("DSD_U32_BE".into())
            ]
        );
        // 88.2 kHz is inside 44.1–192 kHz but the driver refuses it.
        assert_eq!(
            caps.rates,
            RateSet::discrete([44_100, 48_000, 96_000, 192_000])
        );
        assert_eq!(caps.channels, (2..=8).collect::<Vec<_>>());
        assert_eq!(caps.period_frames, Some(FrameRange { min: 32, max: 4096 }));
        // Open max bound: 16384 is excluded.
        assert_eq!(
            caps.buffer_frames,
            Some(FrameRange {
                min: 64,
                max: 16383
            })
        );
    }

    #[test]
    fn refused_capability_query_is_an_error() {
        let mut dev = Simulated {
            formats: vec![],
            rates: vec![48_000],
            channels: (2, 2),
            period: (64, 64),
            calls: 0,
        };
        assert!(probe_caps(&mut dev).is_err());
        assert_eq!(dev.calls, 1);
    }

    #[test]
    fn missing_dev_snd_is_explained() {
        let src = DevSnd {
            dir: PathBuf::from("/nonexistent/ratewise/dev/snd"),
        };
        assert!(matches!(src.probe(), Err(KernelError::NoDevSnd(_))));
    }

    #[test]
    fn fixture_round_trip() {
        let probes = vec![
            KernelPcmProbe {
                card: 0,
                device: 0,
                direction: Direction::Playback,
                result: KernelOutcome::Busy,
            },
            KernelPcmProbe {
                card: 0,
                device: 0,
                direction: Direction::Capture,
                result: KernelOutcome::Ok(KernelCaps {
                    formats: vec![SampleFormat::S16Le],
                    rates: RateSet::discrete([48_000]),
                    channels: vec![2],
                    period_frames: None,
                    buffer_frames: None,
                }),
            },
        ];
        let json = serde_json::to_string(&probes).unwrap();
        assert!(json.contains(r#""status":"busy""#));
        let back: Vec<KernelPcmProbe> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, probes);
    }
}
