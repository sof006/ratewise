//! Ratewise: view and set Linux sound-card sample rates, formats and buffer sizes.
//!
//! Layout follows `docs/SPEC.md` §2:
//! - [`core`]: pure logic, no I/O.
//! - [`probe`]: read-only discovery (PipeWire, kernel ioctls, /proc/asound, stack detection).
//! - [`backend`]: applies changes per sound stack.
//! - [`app`]: what the CLI and GUI share: reading state for a plan, and carrying it out.
//! - [`cli`]: command-line interface.
//! - `gui`: the Slint window (feature `gui`, on by default).

pub mod app;
pub mod backend;
pub mod cli;
pub mod core;
pub mod crash;
#[cfg(feature = "gui")]
pub mod gui;
pub mod probe;
