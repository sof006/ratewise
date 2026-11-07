//! Read-only discovery. Every probe is optional: a missing tool, device or
//! sound server produces an explained note, never a panic.

pub mod command;
pub mod kernel;
pub mod pipewire;
pub mod procfs;
pub mod stack;

use std::path::{Path, PathBuf};

use crate::core::facts::{EnvFacts, KernelPcmProbe, ProcAsound, PwDump};
use crate::core::merge::{Facts, merge};
use crate::core::model::{ProbeNote, Snapshot, Source, StackReport};
use kernel::{DevSnd, KernelError, KernelFile, KernelSource};
use pipewire::{PipeWireError, PwDumpCommand, PwDumpFile, PwDumpSource, parse_pw_dump};
use procfs::{DirTree, FileTree, ProcfsError};
use stack::{EnvSource, FixtureEnv, LiveEnv};

/// The set of data sources to probe: the live system, or a captured fixture.
pub struct Probes {
    procfs: Box<dyn FileTree>,
    pipewire: Box<dyn PwDumpSource>,
    kernel: Box<dyn KernelSource>,
    env: Box<dyn EnvSource>,
    fixture: Option<PathBuf>,
}

/// Every source's raw result, for `doctor` and `snapshot`.
pub struct Gathered {
    pub procfs: Result<ProcAsound, ProcfsError>,
    pub pipewire: Result<PwDump, PipeWireError>,
    pub kernel: Result<Vec<KernelPcmProbe>, KernelError>,
    pub env: EnvFacts,
    pub stack: StackReport,
}

impl Probes {
    /// The live system: `/proc/asound`, `/dev/snd`, and the installed tools.
    pub fn system() -> Self {
        Self {
            procfs: Box::new(DirTree::new("/proc/asound")),
            pipewire: Box::new(PwDumpCommand::default()),
            kernel: Box::new(DevSnd::default()),
            env: Box::new(LiveEnv),
            fixture: None,
        }
    }

    /// A fixture directory (spec §8): `proc-asound/`, and optionally `pw-dump.json`,
    /// `kernel.json`, `versions.txt` and `processes.txt`.
    pub fn fixture(dir: &Path) -> Self {
        Self {
            procfs: Box::new(DirTree::new(dir.join("proc-asound"))),
            pipewire: Box::new(PwDumpFile {
                path: dir.join("pw-dump.json"),
            }),
            kernel: Box::new(KernelFile {
                path: dir.join("kernel.json"),
            }),
            env: Box::new(FixtureEnv {
                dir: dir.to_path_buf(),
            }),
            fixture: Some(dir.to_path_buf()),
        }
    }

    /// Custom sources, for tests.
    pub fn from_parts(
        procfs: Box<dyn FileTree>,
        pipewire: Box<dyn PwDumpSource>,
        kernel: Box<dyn KernelSource>,
        env: Box<dyn EnvSource>,
    ) -> Self {
        Self {
            procfs,
            pipewire,
            kernel,
            env,
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

    pub fn kernel(&self) -> Result<Vec<KernelPcmProbe>, KernelError> {
        self.kernel.probe()
    }

    /// Runs every probe once.
    pub fn gather(&self) -> Gathered {
        let procfs = self.procfs();
        let pipewire = self.pipewire();
        let kernel = self.kernel();
        let env = self.env.env_facts();
        let have_cards = procfs.as_ref().is_ok_and(|p| !p.cards.is_empty());
        let stack = crate::core::stack::detect(pipewire.as_ref().ok(), &env, have_cards);
        Gathered {
            procfs,
            pipewire,
            kernel,
            env,
            stack,
        }
    }

    /// Probe everything and merge. Failures become notes.
    pub fn snapshot(&self) -> Snapshot {
        snapshot_from(self.gather())
    }
}

pub fn snapshot_from(g: Gathered) -> Snapshot {
    let mut notes = Vec::new();
    let mut note = |source, e: &dyn std::fmt::Display| {
        tracing::info!(%source, error = %e, "probe unavailable");
        notes.push(ProbeNote {
            source,
            message: e.to_string(),
        });
    };
    if let Err(e) = &g.procfs {
        note(Source::Procfs, e);
    }
    if let Err(e) = &g.pipewire {
        note(Source::PipeWire, e);
    }
    if let Err(e) = &g.kernel {
        note(Source::Kernel, e);
    }
    merge(
        Facts {
            procfs: g.procfs.as_ref().ok(),
            pipewire: g.pipewire.as_ref().ok(),
            kernel: g.kernel.as_deref().ok(),
            stack: g.stack,
        },
        notes,
    )
}
