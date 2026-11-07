//! Ratewise: view and set Linux sound-card sample rates, formats and buffer sizes.
//!
//! Layout follows `docs/SPEC.md` §2:
//! - [`core`]: pure logic, no I/O.
//! - [`probe`]: read-only discovery (PipeWire, kernel ioctls, /proc/asound, stack detection).
//! - [`backend`]: applies changes per sound stack.
//! - [`cli`]: command-line interface.

pub mod backend;
pub mod cli;
pub mod core;
pub mod probe;
