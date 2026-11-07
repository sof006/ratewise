# Fixtures

One directory per captured system (spec §8):

- `pw-dump.json`: output of `pw-dump` (absent = no PipeWire)
- `proc-asound/`: a copy of `/proc/asound` (absent = no ALSA, e.g. a container)
- `versions.txt`: tool and OS versions. `synthetic: true` marks hand-written fixtures.

Collect real ones with `ratewise capture-fixture <new dir>` (it redacts user/host names, media titles, serials and Bluetooth addresses). Point any command at a fixture with the hidden `--fixture <dir>` flag or `RATEWISE_FIXTURE=<dir>`.

| Fixture | Covers | Real? |
|---|---|---|
| `arch-desktop` | PipeWire 1.6.10 / WirePlumber 0.5.18 on CachyOS: Topping E30 II Lite (UCM HiFi profile, S32_LE + DSD, playing at 48 kHz), AMD HDMI (3 PCMs unused), SteelSeries Arctis Nova 7 (24-bit out, mono mic) | **real** |
| `pw-1.2-desktop` | PipeWire 1.2 / WirePlumber 0.5: onboard HDA, NVidia HDMI (one PCM unused by the profile), USB DAC with 24/32-bit up to 768 kHz, Bluetooth A2DP sink, a virtual sink and a stream that must be ignored | synthetic |
| `pw-0.3-laptop` | PipeWire 0.3.48 / WirePlumber 0.4: values as strings, metadata values as JSON strings | synthetic |
| `alsa-only` | No sound server; USB mic with a continuous rate range | synthetic |
| `pulseaudio-only` | PulseAudio, no PipeWire | synthetic |
| `no-sound` | Container: no `/proc/asound`, no PipeWire | synthetic |

Synthetic fixtures follow the kernel and pw-dump formats as documented, but real captures should replace them.
