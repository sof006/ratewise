//! Read-only discovery. Every probe is optional: a missing tool, device or
//! sound server produces an explained "unavailable" result, never a panic.

pub mod kernel;
pub mod pipewire;
pub mod procfs;
pub mod stack;
