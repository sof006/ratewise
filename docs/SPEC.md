# Ratewise — Linux audio sample-rate & format manager

*Working title. Rename freely.*

A single-file AppImage that detects every sound device on a Linux system, shows what each one can actually do (sample rates, bit depths/formats, channels, buffer sizes), shows what it is running at right now, and lets the user change those settings live or persistently, without editing config files by hand.

---

## 1. Goals and non-goals

### Goals
- Works on any mainstream glibc-based distro from roughly 2020 onward, on x86_64 (aarch64 is a stretch goal).
- **Double-click and it opens. Always.** This means no missing libraries, no FUSE errors and no GPU-driver crashes. It also must not crash when there's no sound server, no PipeWire tools or no `/dev/snd` access. Section 6 is the contract for this.
- Detects the active audio stack (PipeWire, PulseAudio, bare ALSA, standalone JACK) and drives the right one.
- Shows real hardware capabilities, not guesses, and clearly labels where each piece of information came from.
- Shows the **actual running** hardware rate and format, separately from the configured value.
- Changes can be **Live** (this session only, instant) or **Saved** (persistent across reboots). **Revert** always works.
- Per-user only. It never needs root and never writes outside the user's config dirs.
- Has a professional, calm UI that a non-expert can use, with enough depth for an audio nerd.
- A CLI mode exists for scripting and remote use.

### Non-goals
- Volume mixer, EQ, per-app routing, effects (pavucontrol/Helvum/EasyEffects already do these).
- System-wide (`/etc`) configuration.
- Configuring a standalone JACK server. Show its status read-only and point the user to their JACK tool.
- musl distros (Alpine, Void-musl). Document them as unsupported. NixOS is supported via `appimage-run`/binfmt, so document how.

---

## 2. Architecture

```
ratewise (one Rust binary)
├── core/            pure logic, no I/O; fully unit-tested
│   ├── model        Device, Capability, CurrentState, Setting, Change
│   └── plan         turns a desired Change into file edits + live commands
├── probe/           read-only discovery
│   ├── pipewire     parse `pw-dump` JSON
│   ├── kernel       ALSA PCM ioctls on /dev/snd/* (no libasound)
│   ├── procfs       /proc/asound/** parsing
│   └── stack        detect which sound server is running + versions
├── backend/         applies changes (trait AudioBackend)
│   ├── pipewire     pw-metadata (live) + conf.d drop-ins (persistent)
│   ├── wireplumber  per-device rules — 0.4 (Lua) AND 0.5 (SPA-JSON)
│   ├── pulseaudio   ~/.config/pulse/daemon.conf
│   └── alsa         ~/.asoundrc (dmix/dsnoop defaults)
├── cli/             clap subcommands
└── ui/              Slint GUI
```

### Hard rules
- **Link no C audio libraries.** No `libasound` and no `libpipewire`. They have to match the host's daemon or config and can't be bundled safely. Talk to the kernel with ioctls, and talk to PipeWire through its own CLI tools (`pw-dump`, `pw-metadata`, `wpctl`), which always match the running daemon.
- Every external command is optional. If a tool is missing, the feature is shown as unavailable with a reason, and nothing panics.
- `core` has no I/O. Probes and backends are traits, with fixture-driven fakes for tests.

### Suggested crates
`slint`, `serde`/`serde_json`, `clap`, `rustix` (ioctls), `thiserror`/`anyhow`, `directories`, `similar` (config diffs), `tracing` + `tracing-subscriber`. No async runtime is needed: run probes on a worker thread and post results to the UI.

---

## 3. Discovery (probing)

Merge three sources per device, and record which source each fact came from (it's shown in the UI as a small "source" hint).

### 3.1 PipeWire (primary when running)
- Run `pw-dump`. Take nodes with `media.class` in `Audio/Sink`, `Audio/Source`, `Audio/Duplex`, plus their parent `Device`.
- From `EnumFormat` params, read rates (lists *and* ranges), formats and channels. From props, read `api.alsa.path`, `alsa.card`, `alsa.device`, `device.bus` (usb/pci) and the description.
- Read the current clock from the `settings` metadata: `clock.rate`, `clock.allowed-rates`, `clock.quantum`, `clock.min-quantum`, `clock.max-quantum`, `clock.force-rate`, `clock.force-quantum`.
- Treat field names as version-dependent. Parse defensively and keep fixture tests for several PipeWire versions.

### 3.2 Kernel direct (precise hardware truth)
- For each `/dev/snd/pcmC{c}D{d}{p|c}`, open with `O_RDWR | O_NONBLOCK` and use `SNDRV_PCM_IOCTL_HW_REFINE` to get the rate interval, format mask, channel interval and period/buffer ranges.
- Expand rate ranges into the standard list (8000, 11025, 16000, 22050, 32000, 44100, 48000, 88200, 96000, 176400, 192000, 352800, 384000, 705600, 768000) by refining with each candidate rate and keeping the ones the kernel accepts.
- Define the `snd_pcm_hw_params` struct and ioctl numbers from the kernel uapi headers. Add tests asserting struct size/layout per target arch.
- **EBUSY is normal** (PipeWire holds the device). In that case, fall back to 3.1/3.3 and offer a "Probe precisely" button. That button temporarily suspends the PipeWire node (`pw-cli`/`wpctl`), probes, and resumes it, after a confirmation dialog warning that audio will drop for a moment.

### 3.3 /proc/asound
- `cards`, `cardN/id`, `cardN/stream*` (USB devices list exact rates/formats per altset, which is excellent data), and `cardN/pcm*/sub*/hw_params`. The last one is the **actual running** rate/format/period, or `closed`.
- Refresh `hw_params` every ~1 s while the window is focused so the "Running now" value stays live.

### 3.4 Stack detection
Report: which server owns audio (PipeWire with pipewire-pulse, PulseAudio, bare ALSA, JACK), their versions, the WirePlumber version (**0.4 vs 0.5 changes the config format**), whether a systemd user session exists, and which CLI tools are present. Show this on an "About this system" panel and in `ratewise doctor`.

---

## 4. Applying changes

Every change goes through `core::plan`, which produces a `Plan` made up of a list of live commands and file writes, each with a human-readable description. The UI shows the plan, including a **diff** of any file it will touch, before anything happens.

### Settings exposed
| Setting | Scope | Notes |
|---|---|---|
| Default sample rate | global | |
| Allowed rates | global | Lets PipeWire switch rate to match content (bit-perfect-ish). Only offer rates the hardware supports. |
| Force rate | global, live | Debug/testing. |
| Quantum / buffer (default, min, max, force) | global | Show the latency in ms next to the frames value. |
| Per-device format/bit depth | device | e.g. S24_LE, S32_LE |
| Per-device rate | device | |
| Per-device period size / headroom | device | Common fix for USB DAC crackle |
| Resampler quality | global | |
| Disable suspend-on-idle | device | Fixes pops at playback start |
| Bluetooth mode / codec (profile) | device, live | High quality (A2DP/BAP) or headset (HFP/HSP, with microphone), and the codec within it. WirePlumber remembers it, so it's never saved. |
| Headset auto-switch | global | Whether WirePlumber switches a Bluetooth headset to headset mode when a program opens its microphone. Live on WirePlumber 0.5 only. |
| LDAC quality | device, saved | `hq` / `sq` / `mq` / `auto`, for devices that offer LDAC. Applied when the device connects. |
| LE Audio latency | device, saved | `low` / `reliable` / balanced (unset), for devices with `bap-*` profiles. PipeWire 1.6+. Applied when the device connects. |

Grey out any value the device can't do, with a tooltip explaining why.

### Backends
- **PipeWire live:** `pw-metadata -n settings 0 <key> <value>`.
- **PipeWire persistent:** write `~/.config/pipewire/pipewire.conf.d/60-ratewise.conf` (`context.properties { default.clock.rate = …, default.clock.allowed-rates = [ … ], … }`).
- **WirePlumber per-device rules:**
  - 0.5+: `~/.config/wireplumber/wireplumber.conf.d/60-ratewise.conf` using `monitor.alsa.rules` SPA-JSON.
  - 0.4: `~/.config/wireplumber/main.lua.d/60-ratewise.lua` using `alsa_monitor.rules`.
  - Match devices on a stable key (`node.name` or `device.name`), never on a numeric ID.
- **Bluetooth (PipeWire only):**
  - Profiles come from the card's `EnumProfile`/`Profile` params in `pw-dump`. Names are `<base>` or `<base>-<codec>`. Since PipeWire 0.3.78 the best codec's profile has the bare name (`a2dp-sink`) and its codec appears only in the description ("…, codec LDAC"); 0.3.21–0.3.77 also list a codec-less `a2dp-sink` next to the per-codec ones. Profile indexes change between versions, so Ratewise looks the index up from the current snapshot every time and never stores it.
  - Switching: `pw-cli set-param <device.name> Profile '{ index: N, save: true }'`. `save: true` makes WirePlumber remember it per device (`~/.local/state/wireplumber/default-profile`). `wpctl set-profile` only sends `save` from WirePlumber 0.5.11 and takes numeric ids only, so it isn't used. pw-cli finds objects by name from PipeWire 0.3.44; before that Ratewise passes the numeric id.
  - Headset auto-switch: WirePlumber 0.5's `bluetooth.autoswitch-to-headset-profile` setting, live with `wpctl settings <key> true|false` (`--reset` for the default), saved in `wireplumber.settings = { … }` in the same `60-ratewise.conf`. A value stored with `wpctl settings --save` overrides config files; Ratewise reads it from the `persistent-sm-settings` metadata and says how to remove it. WirePlumber 0.4: `bluetooth_policy.policy["media-role.use-headset-profile"]` in `~/.config/wireplumber/policy.lua.d/60-ratewise.lua`, saved only.
  - LDAC quality: `bluez5.a2dp.ldac.quality` on the card, via `monitor.bluez.rules` matching `device.name` (0.5), or `bluez_monitor.rules` in `~/.config/wireplumber/bluetooth.lua.d/60-ratewise.lua` (0.4). Needs a WirePlumber restart and the device to reconnect.
  - LE Audio latency: `bluez5.bap.force-target-latency` = `low-latency` / `high-reliability` on the card, through the same rules. It restricts PipeWire's LC3 QoS choice to BAP Table 5.2's low-latency (`_1`) or high-reliability (`_2`) presets. Unset, PipeWire picks from both, preferring high reliability at 44.1/48 kHz since early 2026. The property is new in PipeWire 1.6.0 and marked experimental, so Ratewise refuses it on older PipeWire. A device counts as LE Audio when it has a profile named `bap-*` (`bap-sink`, `bap-source`, `bap-duplex`). PipeWire also reads `bluez5.bap.preset` (one named preset such as `48_5_1`; ignored when the device can't do it), `.rtn`, `.latency`, `.delay` and `.framing`; Ratewise doesn't set these yet. PipeWire's own docs call the last one `bluez5.framing`, but the code reads `bluez5.bap.framing`. GMAP's gaming presets (`48_x_gr`) aren't implemented in PipeWire.
  - Rate, format, period size and the other ALSA settings don't apply to Bluetooth devices: the codec sets them. A card whose profile is off has no nodes; it's still listed so it can be switched on. PulseAudio-only Bluetooth is shown but not configured.
- **Resampler quality (PipeWire):** `stream.properties { resample.quality = N }` in `~/.config/pipewire/client.conf.d/` and `pipewire-pulse.conf.d/` (plus `client-rt.conf.d/` on PipeWire 0.3, whose ALSA-plugin clients read it). Programs pick it up when they start playing.
- **PulseAudio:** a drop-in, `~/.config/pulse/daemon.conf.d/60-ratewise.conf` (`default-sample-rate`, `alternate-sample-rate`, `avoid-resampling = yes`, `default-sample-format`). PulseAudio has a `.d` mechanism, but derives the folder from whichever `daemon.conf` it opened, so it reads `~/.config/pulse/daemon.conf.d/` only when `~/.config/pulse/daemon.conf` exists. If it doesn't, Ratewise creates it as a copy of `/etc/pulse/daemon.conf` (the user file replaces the system one), records a fingerprint, and `reset` removes it only if nobody edited it. Quantum, resampler and per-device settings aren't offered. `$PULSE_CONFIG` set → refuse.
- **Bare ALSA:** a marked block in `~/.asoundrc` with `defaults.pcm.dmix.rate` and `.format` (dsnoop reads these too; there are no `defaults.pcm.dsnoop.*` keys), and per-card period sizes as alsa-lib's `pcm/dmix.conf` and `pcm/dsnoop.conf` look them up: `defaults.dmix.<card_id>.period_size` for playback, `cards.<card_id>.pcm.dsnoop.period_size` for capture.

### Safety
- Ratewise owns **only its own files** (`60-ratewise.*`, plus a `~/.config/pulse/daemon.conf` it created). The only user-authored file it edits is `~/.asoundrc`, which has no drop-in mechanism: there it edits only a block between `# >>> ratewise >>>` and `# <<< ratewise <<<`, keeps any other content byte-for-byte, and warns when later lines override its keys. A half-open block is refused, not repaired.
- It never writes through a symlink, and refuses at apply time if any target changed since the plan was made. Writes are atomic (temp file + rename, keeping permissions); a failure puts back files already written and undoes commands already run.
- Back up to `~/.local/state/ratewise/backups/<YYYYMMDD-HHMMSS>/` (UTC; `manifest.json` plus `files/<n>`) before any write, including the commands that undo live changes. **Revert** restores the newest backup not yet reverted, so repeating it walks back through history. "Reset to defaults" removes Ratewise's files and its block, and releases forced live settings.
- What Ratewise has saved lives in `~/.local/state/ratewise/saved.json`; generated files are regenerated from it on every save, so edits to them are lost (their header says so).
- After a persistent change, offer **Restart audio services**:
  - With a systemd user session: `systemctl --user restart pipewire pipewire-pulse wireplumber` (or `pulseaudio -k`).
  - Without one (OpenRC/runit): fall back to killing and relaunching the processes, or tell the user to log out and back in.
  - Warn first that playing audio will stop.
- After applying, re-probe and show a **verification** result: requested vs. actually running. If they differ, explain the likely reason (e.g. "the device is idle, and the rate is set when playback starts").

---

## 5. UI

The toolkit is **Slint**, using its own renderer.

### Layout
- **Left sidebar:** device list grouped as Outputs / Inputs. Each entry shows an icon (USB / built-in / HDMI / Bluetooth / microphone), the name, and a second line with the running rate ("48 kHz · 24-bit", in the signal colour while playing, or "Idle"). The bottom of the sidebar shows the sound-server status (e.g. "PipeWire 1.2 · WirePlumber 0.5") next to the refresh and theme buttons.
- **Main panel, per device:**
  - **Overview:** a large "Playing now" readout (rate / depth / channels, then format / period / buffer with latency), a grid of the standard rates (the 44.1 and 48 kHz families in rows, multiples in columns, low rates only when supported) with supported ones filled and the running one marked, and the "Configured" values beside a capability summary.
  - **Sample rate & format:** segmented selectors containing only valid options. Bluetooth devices get a **Bluetooth** tab here instead: mode, codec (only ones both sides support), LDAC quality when offered, LE Audio latency when the device offers LE Audio, and headset auto-switch.
  - **Buffer & latency:** a quantum slider with live ms readout and plain-English hints ("lower = less latency, more risk of crackles").
  - **Advanced:** period size, headroom, suspend, resampler, raw properties table (copyable).
- **Global settings** page for clock/allowed rates/quantum.
- **Bottom action bar:** `Revert last change` when nothing is pending; `Discard` · `Apply live` · `Save permanently` when something is. Both open a review sheet showing the plan, with file diffs coloured by line.
- **About this system / Doctor** page showing stack, versions, tools found, and which renderer is in use.

### Feel
- Follow the system light/dark theme where it can be detected, and offer a manual toggle.
- Use an embedded UI font (Inter or Noto Sans, OFL) so text looks identical everywhere and doesn't depend on fontconfig. Ratewise embeds Inter 4.1's static Regular, SemiBold and Bold from `assets/fonts/` (licence alongside; ship it in the AppImage too). Monospace text (the doctor report, diffs) uses the system's monospace font, found with `fc-match`.
- Keep it consistent: 8 px spacing grid, warm neutral surfaces with ink (not a brand colour) marking what's selected, and colour only where it means something (playing, warnings, errors, diffs). Every control is drawn by Ratewise, not taken from a system style, so it looks the same everywhere. No clutter. Empty and error states need real wording, e.g. "No sound devices found. This usually means you don't have access to /dev/snd. Are you in the `audio` group, or running inside a container?"
- It must be fully keyboard-navigable, and must stay usable at 1280×720 and on HiDPI.

### Behaviour
- Picks are collected as pending edits; `Apply live` or `Save permanently` turns them all into one plan (one backup, one restart). Picking what's already in effect drops the edit.
- Options a device or stack can't take are shown disabled, and hovering one says why.
- The review sheet offers to restart the affected services, ticked by default and labelled as interrupting sound.
- Probing and applying run on a worker thread. "Running now" refreshes every 3 s without opening devices; Refresh re-probes everything.
- Slint has no API that reports which renderer it ended up using, so the Doctor page states the policy (OpenGL with automatic software fallback, or software when forced).

---

## 6. Packaging: the "always opens" contract

### 6.1 Build
- Target a glibc floor of **2.28** (covers roughly RHEL 8 / Debian 10 / Ubuntu 20.04 onward). Use `cargo zigbuild --target x86_64-unknown-linux-gnu.2.28`, or build inside an old container (e.g. `debian:bullseye`). CI must fail if the binary references any glibc symbol newer than the floor (check with `objdump -T`).
- Release profile: LTO, `strip = true`, `panic = "unwind"` (so the panic handler below can run).

### 6.2 AppImage
- Use the **static type2 runtime** (the current `appimagetool` default). It does not need `libfuse2`, which is the #1 cause of "double-clicked and nothing happened" on Ubuntu 22.04+ and Fedora. Check this against the current appimagetool release.
- Bundle every shared library the binary or Slint needs **except** the host-only allowlist:
  - **Allowed from host:** glibc family (`libc`, `libm`, `libdl`, `libpthread`, `librt`, `ld-linux*`), `libgcc_s`, GPU driver stack (`libGL`, `libEGL`, `libGLX`, `libGLdispatch`, `libvulkan`, `libdrm`, `libgbm`).
  - **Bundled:** everything else, typically `libxkbcommon`, `libxkbcommon-x11`, `libxcb*`, `libX11*`, `libXcursor`, `libXi`, `libXrandr`, `libwayland-client`, `libwayland-cursor`, `libwayland-egl`, plus `libfontconfig`/`libfreetype` if Slint pulls them in.
  - Pay attention to libraries Slint/winit **dlopen** at runtime. `ldd` won't show them, so bundle them explicitly and point the loader at them from `AppRun`.
  - **As built:** the bundled copies come from Debian 10 (glibc 2.28), fetched by `packaging/fetch-libs.sh` and listed in `packaging/bundled-libs.txt`. A distro's own copies need newer glibc (Ubuntu 24.04's need 2.38), so they can't be used. `AppRun` uses a bundled copy **only when the host lacks that library**. A desktop's own X11/xcb/Wayland libraries are the ones its GPU driver (from the host) was built against, and forcing older copies under a newer Mesa is the usual way AppImages break. Minimal systems that lack them still get the bundled ones. `RATEWISE_LIBS=bundled` forces every bundled copy and `RATEWISE_LIBS=host` none, for troubleshooting.
- Include a `.desktop` file, an icon (SVG + 256 px PNG), and AppStream metainfo.
- **Audit script** (`packaging/audit.sh`, run in CI): extract the AppImage, then run `ldd` on every ELF inside and fail if any dependency is unresolved or resolves to a host library outside the allowlist.

### 6.3 Runtime resilience (in the app)
- **Renderer fallback:** try the GPU renderer, and if context creation fails, switch automatically to Slint's software renderer. Allow forcing it with `RATEWISE_RENDERER=software`. OpenGL can also fail only once the window is shown (openSUSE under Xvfb: "glutin: initialization failed"), after Slint's own fallback point, so when the window fails to open or run, Ratewise reruns itself with `RATEWISE_RENDERER=software`.
- **Backend fallback:** Wayland → X11. If there's no display at all, print a clear message suggesting the CLI and exit non-zero. winit doesn't fall back by itself, so when the window fails with both `WAYLAND_DISPLAY` and `DISPLAY` set, Ratewise reruns itself without `WAYLAND_DISPLAY`. (Slint always uses X11 under WSL, so test Wayland on real Linux or with the WSL markers hidden.)
- Install a **panic handler** that writes a log to `~/.local/state/ratewise/crash-<ts>.log`. If a display is available, it also shows a minimal error window (or a `zenity`/`kdialog` popup if one is present).
- Every probe and backend failure is non-fatal. The app must start and render with: no PipeWire, no PulseAudio, no `/dev/snd` access, no `pw-*` tools, a read-only home directory, or a missing `XDG_RUNTIME_DIR`.

### 6.4 Launch test matrix (CI)
Run `packaging/test-launch.sh`. For each container below, run the AppImage with `--appimage-extract-and-run` (containers have no FUSE) under Xvfb. The test passes if `ratewise --self-test` renders one frame offscreen to PNG and exits 0, and `ratewise doctor` exits 0.

- ubuntu:20.04, 22.04, 24.04
- debian:11, 12
- fedora:latest
- opensuse/tumbleweed
- archlinux:latest

Use **minimal images with no audio packages installed**. That's the point: it must still open. Repeat one run on a Wayland headless compositor (e.g. `weston --backend=headless`) to exercise the Wayland path.

---

## 7. CLI

```
ratewise list [--json]              devices, capabilities, running state
ratewise show <device> [--json]
ratewise set rate 96000 [--device X] [--live|--save]
ratewise set allowed-rates 44100,48000,88200,96000 --save
ratewise set quantum 256 [--live|--save] [--restart]
ratewise set {min-quantum,max-quantum,force-rate,force-quantum,format,period-size,headroom,suspend,resample-quality} …
ratewise set profile {high-quality,headset,off,<codec>,<profile name>} --device X     Bluetooth, live
ratewise set headset-autoswitch {on,off,default} [--live|--save]
ratewise set ldac-quality {hq,sq,mq,auto,default} --device X --save
ratewise set le-audio-latency {low,reliable,balanced,default} --device X --save
ratewise plan …                     print the plan + diffs, change nothing
ratewise revert | reset [--restart] [--dry-run]
ratewise doctor                     stack/version/tools/renderer report
ratewise capture-fixture <dir>      dump pw-dump, /proc/asound, versions for bug reports + tests
ratewise                            open the window
ratewise --self-test                render one frame offscreen to PNG (in the temp dir), exit 0 if it isn't blank
```

Device selectors accept the stable name, the ALSA `hw:CARD=…` id, or a fuzzy description match. `--json` output is a stable, documented schema.

Without `--live` or `--save`, `set` changes things live when it can and otherwise explains that `--save` is needed. Values accept friendly spellings (`96k`, `44.1kHz`, `default`; `off` for force values and suspend). `--restart` restarts the affected services after a save, and asks for nothing first, so the GUI must confirm. Hidden flags for development: `--fixture <dir>` reads a captured system, and `--sandbox <dir>` sends every write into `<dir>/home` and only records commands; `--fixture` refuses to write without `--sandbox`. The window takes the same two flags; with `--fixture` alone it shows the captured system read-only.

Environment variables: `RATEWISE_LOG=debug` for verbose logs, `RATEWISE_RENDERER=software` to skip OpenGL, `RATEWISE_THEME=light|dark` to override the system theme. For design review, the hidden `--screenshot <png>` renders the window to a file and exits; `RATEWISE_PAGE` picks the page (`device:<id>`, `global`, `doctor`, `review:<setting>=<value>`, or `apply:<setting>=<value>`, which also confirms it, so use `--sandbox`), `RATEWISE_TAB=0..3` picks the device tab, and `RATEWISE_SIZE=960x620` sets the window size.

---

## 8. Testing

- **Unit:** `core::plan` for every backend × setting, with golden-file tests for generated configs (WirePlumber 0.4 and 0.5 both covered).
- **Fixtures:** `tests/fixtures/<name>/` contains `pw-dump.json`, a `/proc/asound` tree and `versions.txt`. Cover at least an onboard HDA codec, a USB DAC with 24/32-bit and high rates, an HDMI output, a Bluetooth sink, a PulseAudio-only system and an ALSA-only system. Collect real ones with `ratewise capture-fixture`.
- **Parser robustness:** fuzz/proptest the `pw-dump` and `/proc` parsers. Garbage input must produce an error, never a panic.
- **Packaging:** audit script + launch matrix (section 6).

---

## 9. Milestones

1. Scaffold, `core` model, `procfs` + `pipewire` probes against fixtures, `ratewise list`/`doctor`.
2. Kernel ioctl probe and stack detection; merge sources with provenance.
3. Plan/apply/revert for PipeWire + WirePlumber (0.4 and 0.5), then PulseAudio and ALSA. CLI `set`/`plan`/`revert`.
4. Slint GUI over the same core.
5. Packaging: zigbuild, AppImage, audit script, launch matrix in CI (GitHub Actions).
6. **Real-hardware check (Joe):** a manual checklist on real machines covering a USB DAC at 96/192 kHz, onboard audio, HDMI, a Bluetooth headset, and a PulseAudio-only VM. Confirm "Running now" changes as expected during playback.

**Stretch:** aarch64 AppImage; AppImageUpdate (zsync) support; a native PKGBUILD/.deb alongside the AppImage.
