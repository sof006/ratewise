# Fixtures

One directory per captured system (spec §8):

- `pw-dump.json`: output of `pw-dump` (absent = no PipeWire)
- `proc-asound/`: a copy of `/proc/asound` (absent = no ALSA, e.g. a container)
- `kernel.json`: what `/dev/snd` answered, one entry per PCM device node (absent = not captured). Each `result` is `ok` with formats, rates, channels, period and buffer ranges, or `busy`, `permission_denied`, or `failed` with a message.
- `processes.txt`: the sound-server processes the user was running, one per line (absent = unknown)
- `versions.txt`: tool and OS versions, plus `pulse-server`, `pulse-server-version` and `systemd-user-session: yes|no` when known. `synthetic: true` marks hand-written fixtures.

Collect real ones with `ratewise capture-fixture <new dir>` (it redacts user/host names, media titles, serials and Bluetooth addresses). Point any command at a fixture with the hidden `--fixture <dir>` flag or `RATEWISE_FIXTURE=<dir>`.

| Fixture | Covers | Real? |
|---|---|---|
| `arch-desktop` | PipeWire 1.6.10 / WirePlumber 0.5.18 on CachyOS: Topping E30 II Lite (UCM HiFi profile, S32_LE + DSD, playing at 48 kHz), AMD HDMI (3 PCMs unused), SteelSeries Arctis Nova 7 (24-bit out, mono mic) | **real** (captured before M2, so no `kernel.json` or `processes.txt`) |
| `arch-desktop-2` | Same machine after M2: adds `kernel.json` (E30 busy while playing; connected HDMI limited to 32–48 kHz stereo by the monitor; unconnected HDMI at driver defaults) and `processes.txt` | **real** |
| `pw-1.2-desktop` | PipeWire 1.2 / WirePlumber 0.5: onboard HDA, NVidia HDMI (one PCM unused by the profile), USB DAC with 24/32-bit up to 768 kHz, Bluetooth A2DP sink, a virtual sink and a stream that must be ignored | synthetic |
| `pw-0.3-laptop` | PipeWire 0.3.48 / WirePlumber 0.4: values as strings, metadata values as JSON strings; user not in the `audio` group, so the kernel probe is refused | synthetic |
| `alsa-only` | No sound server; USB mic with a continuous rate range; the running HDA output is busy | synthetic |
| `pulseaudio-only` | PulseAudio, no PipeWire | synthetic |
| `no-sound` | Container: no `/proc/asound`, no PipeWire | synthetic |

Synthetic fixtures follow the kernel and pw-dump formats as documented, but real captures should replace them.
