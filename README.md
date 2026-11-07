# Ratewise

View and set Linux sound-card sample rates, formats and buffer sizes, for PipeWire
(with WirePlumber 0.4 or 0.5), PulseAudio or plain ALSA. It comes as a window and a
command line, in one self-contained AppImage.

- See what each device is running at right now, what it's configured to, and what
  it supports.
- Change the rate, bit depth, buffer size (quantum), period size, headroom, idle
  suspend and resampler quality, per device or for everything.
- For Bluetooth headphones and headsets on PipeWire: switch between high quality and
  headset (microphone) mode, pick the codec (LDAC, aptX, AAC, SBC, …), set LDAC's
  bitrate, and choose whether calls switch the headset automatically.
- Every change is shown as a plan with file diffs first, is backed up, and can be
  reverted. Ratewise only writes your own config files and never needs root.

## Install

Download `Ratewise-x86_64.AppImage` from
[Releases](https://github.com/sof006/ratewise/releases), make it executable and run
it:

```
chmod +x Ratewise-x86_64.AppImage
./Ratewise-x86_64.AppImage
```

It runs on x86_64 distributions with glibc 2.28 or newer (RHEL 8, Debian 10,
Ubuntu 20.04 onward), and doesn't need libfuse2.

## Command line

The same AppImage is the CLI:

```
./Ratewise-x86_64.AppImage list
./Ratewise-x86_64.AppImage plan rate 96k --device e30 --save
./Ratewise-x86_64.AppImage set quantum 256 --save --restart
./Ratewise-x86_64.AppImage set profile ldac --device headphones
./Ratewise-x86_64.AppImage revert
./Ratewise-x86_64.AppImage doctor
```

`ratewise --help` lists everything. `docs/SPEC.md` has the details.

## Troubleshooting

- `ratewise doctor` reports what Ratewise can see: sound stack, versions, tools and
  display.
- `RATEWISE_RENDERER=software` skips OpenGL if the window draws wrongly.
- `RATEWISE_LOG=debug` prints verbose logs.
- A crash writes a report to `~/.local/state/ratewise/crash-<time>.log`.

## Building

`cargo build` for development. `packaging/build-appimage.sh` builds the AppImage
(needs `cargo-zigbuild`), and `packaging/audit.sh` checks it.

## Licence

GPL-3.0-or-later. The embedded Inter font is under the SIL Open Font License 1.1
(`assets/fonts/Inter-LICENSE.txt`).
