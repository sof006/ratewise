//! Device, capability and state model (filled in during milestone 1).

use serde::{Deserialize, Serialize};

/// Where a probed fact came from, so the UI can show provenance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Source {
    PipeWire,
    Kernel,
    Procfs,
}

/// A value together with the source it was read from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sourced<T> {
    pub value: T,
    pub source: Source,
}

impl<T> Sourced<T> {
    pub fn new(value: T, source: Source) -> Self {
        Self { value, source }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sourced_serializes_with_provenance() {
        let s = Sourced::new(48_000u32, Source::Procfs);
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(json, r#"{"value":48000,"source":"Procfs"}"#);
    }
}
