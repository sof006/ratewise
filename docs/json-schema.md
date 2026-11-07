# `ratewise list --json` schema

Stable output for scripts. `schema_version` changes only when a field is removed, renamed or changes type; new fields may be added at any time. Every key is always present, with `null` when unknown.

```jsonc
{
  "schema_version": 1,
  "devices": [ Device, ... ],          // outputs first, then inputs; ordered by card/device
  "clock": Sourced<Clock> | null,      // PipeWire's global clock settings
  "pipewire_version": "1.2.7" | null,
  "stack": Stack,                      // which sound server and session manager are running
  "notes": [ { "source": Source, "message": "plain-English explanation" } ]
}
```

## Device

| Field | Type | Notes |
|---|---|---|
| `id` | string | Stable selector. The PipeWire `node.name` when PipeWire knows the device, otherwise `alsa:<card id>:<device>:<playback\|capture>`. |
| `description` | string | Human-readable name. |
| `direction` | `"playback"` \| `"capture"` \| `"duplex"` | |
| `kind` | `"usb"` \| `"builtin"` \| `"hdmi"` \| `"bluetooth"` \| `"unknown"` | |
| `alsa` | object \| null | `card`, `card_id`, `device`, `card_driver`, `pcm_name`. `hw:CARD=<card_id>,DEV=<device>` addresses it. |
| `pipewire` | object \| null | `object_id` (not stable across restarts), `node_name`, `media_class`, `state`, `is_default`. |
| `capabilities` | object | `rates`, `formats`, `channels`, `period_frames`, `buffer_frames`; each `Sourced<…>` or null. Period and buffer ranges come only from the kernel. |
| `running` | `Sourced<Running>` \| null | What the hardware is doing now. |
| `kernel_probe` | KernelProbe \| null | How the direct `/dev/snd` query went. null when it wasn't attempted (no ALSA device, or no kernel readings at all). |

Capabilities prefer the most direct source: the kernel's answer, then `/proc/asound` USB stream files, then PipeWire. A device that's in use can't be queried directly, so its capabilities come from the other sources.

## Stack

| Field | Type | Notes |
|---|---|---|
| `server` | `"pipewire"` \| `"pulseaudio"` \| `"jack"` \| `"alsa"` \| `"none"` | What owns audio. `"alsa"` means no sound server; programs open devices directly. |
| `pipewire_version` | string \| null | |
| `pipewire_pulse` | bool | PipeWire's PulseAudio replacement is running. |
| `session_manager` | object \| null | `{ "name": "wireplumber", "version": "0.5.18" \| null, "config": "lua-0.4" \| "spa-json-0.5" \| null }` or `{ "name": "pipewire-media-session" }`. |
| `pulseaudio_version` | string \| null | Only for a real PulseAudio daemon, not pipewire-pulse. |
| `jack_running` | bool | A standalone JACK server (jackd or jackdbus). |
| `systemd_user_session` | bool \| null | Whether `systemctl --user` can restart audio services. |

## Value types

- **Source**: `"PipeWire"`, `"Kernel"` or `"Procfs"`: where a fact was read. `"Kernel"` means the driver answered an `HW_REFINE` query on `/dev/snd`.
- **FrameRange**: `{ "min": 32, "max": 524288 }`, in frames.
- **KernelProbe**: `{ "status": "ok" }`, `{ "status": "busy" }` (another program has the device open, which is normal for a device in use), `{ "status": "permission_denied" }`, or `{ "status": "failed", "message": "…" }`.
- **Sourced\<T\>**: `{ "value": T, "source": Source }`.
- **Rates**: `{ "kind": "discrete", "rates": [44100, 48000] }` or `{ "kind": "range", "min": 8000, "max": 48000 }`. A range with `max` ≥ 10,000,000 means PipeWire will convert any rate and says nothing about the hardware.
- **Formats**: ALSA names, e.g. `"S16_LE"`, `"S24_LE"` (24-bit in 32-bit container), `"S24_3LE"` (packed 24-bit), `"S32_LE"`, `"FLOAT_LE"`. Unknown formats are passed through verbatim.
- **Running**: `{ "state": "idle" }` or `{ "state": "active", "rate": 48000, "format": "S32_LE" | null, "channels": 2 | null, "period_frames": 1024 | null, "buffer_frames": 4096 | null }`.
- **Clock**: `rate`, `allowed_rates` (array), `quantum`, `min_quantum`, `max_quantum`, `force_rate`, `force_quantum`; each may be null. A force value of `0` means "not forced".
