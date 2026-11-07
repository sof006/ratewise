//! Read-only discovery. Every probe is optional: a missing tool, device or
//! sound server produces an explained note, never a panic.

pub mod command;
pub mod kernel;
pub mod pipewire;
pub mod procfs;
pub mod stack;

use std::path::{Path, PathBuf};

use crate::core::facts::{ProcAsound, PwDump};
use crate::core::merge::merge;
use crate::core::model::{ProbeNote, Snapshot, Source};
use pipewire::{PipeWireError, PwDumpCommand, PwDumpFile, PwDumpSource, parse_pw_dump};
use procfs::{DirTree, FileTree, ProcfsError};

/// The set of data sources to probe: the live system, or a captured fixture.
pub struct Probes {
    procfs: Box<dyn FileTree>,
    pipewire: Box<dyn PwDumpSource>,
    fixture: Option<PathBuf>,
}

impl Probes {
    /// The live system: `/proc/asound` and the installed `pw-dump`.
    pub fn system() -> Self {
        Self {
            procfs: Box::new(DirTree::new("/proc/asound")),
            pipewire: Box::new(PwDumpCommand::default()),
            fixture: None,
        }
    }

    /// A fixture directory with `proc-asound/` and optionally `pw-dump.json` (spec §8).
    pub fn fixture(dir: &Path) -> Self {
        Self {
            procfs: Box::new(DirTree::new(dir.join("proc-asound"))),
            pipewire: Box::new(PwDumpFile {
                path: dir.join("pw-dump.json"),
            }),
            fixture: Some(dir.to_path_buf()),
        }
    }

    /// Custom sources, for tests.
    pub fn from_parts(procfs: Box<dyn FileTree>, pipewire: Box<dyn PwDumpSource>) -> Self {
        Self {
            procfs,
            pipewire,
            fixture: None,
        }
    }

    pub fn fixture_dir(&self) -> Option<&Path> {
        self.fixture.as_deref()
    }

    pub fn procfs(&self) -> Result<ProcAsound, ProcfsError> {
        procfs::probe(self.procfs.as_ref())
    }

    pub fn pipewire(&self) -> Result<PwDump, PipeWireError> {
        let text = self.pipewire.pw_dump()?;
        Ok(parse_pw_dump(&text)?)
    }

    /// Probe everything and merge. Failures become notes.
    pub fn snapshot(&self) -> Snapshot {
        let mut notes = Vec::new();
        let procfs = self
            .procfs()
            .map_err(|e| {
                tracing::info!(error = %e, "procfs probe unavailable");
                notes.push(ProbeNote {
                    source: Source::Procfs,
                    message: e.to_string(),
                });
            })
            .ok();
        let pw = self
            .pipewire()
            .map_err(|e| {
                tracing::info!(error = %e, "PipeWire probe unavailable");
                notes.push(ProbeNote {
                    source: Source::PipeWire,
                    message: e.to_string(),
                });
            })
            .ok();
        merge(procfs.as_ref(), pw.as_ref(), notes)
    }
}
