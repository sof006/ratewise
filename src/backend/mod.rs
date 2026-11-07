//! Applies plans to a specific sound stack (spec §4, milestone 3).
//! Writes stay inside the per-user paths listed in CLAUDE.md, with a backup first.

pub mod alsa;
pub mod pipewire;
pub mod pulseaudio;
pub mod wireplumber;
