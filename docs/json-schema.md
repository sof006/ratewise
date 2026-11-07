# `ratewise list --json` schema

Stable output for scripts. `schema_version` changes only when a field is removed, renamed or changes type; new fields may be added at any time. Every key is always present, with `null` when unknown.

```jsonc
{
  "schema_version": 1,
  "devices": [ Device, ... ],          // outputs first, then inputs; ordered by card/device
  "clock": Sourced<Clock> | null,      // PipeWire's global clock settings
  "pipewire_version": "1.2.7" | null,
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
| `capabilities` | object | `rates`, `formats`, `channels`; each `Sourced<…>` or null. |
| `running` | `Sourced<Running>` \| null | What the hardware is doing now. |

## Value types

- **Source**: `"PipeWire"`, `"Kernel"` or `"Procfs"`: where a fact was read.
- **Sourced\<T\>**: `{ "value": T, "source": Source }`.
- **Rates**: `{ "kind": "discrete", "rates": [44100, 48000] }` or `{ "kind": "range", "min": 8000, "max": 48000 }`. A range with `max` ≥ 10,000,000 means PipeWire will convert any rate and says nothing about the hardware.
- **Formats**: ALSA names, e.g. `"S16_LE"`, `"S24_LE"` (24-bit in 32-bit container), `"S24_3LE"` (packed 24-bit), `"S32_LE"`, `"FLOAT_LE"`. Unknown formats are passed through verbatim.
- **Running**: `{ "state": "idle" }` or `{ "state": "active", "rate": 48000, "format": "S32_LE" | null, "channels": 2 | null, "period_frames": 1024 | null, "buffer_frames": 4096 | null }`.
- **Clock**: `rate`, `allowed_rates` (array), `quantum`, `min_quantum`, `max_quantum`, `force_rate`, `force_quantum`; each may be null. A force value of `0` means "not forced".
