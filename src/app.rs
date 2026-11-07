//! What the CLI and the GUI share: where changes go, reading the current state for a
//! plan, and carrying a plan out (backup, apply, restart, verify).
//!
//! This is I/O glue between `probe`, `core::plan` and `backend`; the decisions stay in
//! `core`.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::backend::{self, Applied, Options, RecordingRunner, Runner, SystemRunner};
use crate::core::facts::KernelPcmProbe;
use crate::core::model::{Snapshot, SoundServer};
use crate::core::paths::Paths;
use crate::core::plan::{
    self, BackupView, Check, Cmd, FileState, Plan, PlanContext, PlanKind, render_checks,
    restart_text,
};
use crate::core::settings::{Mode, SavedConfig};
use crate::probe::{Probes, stack};

/// A plain-English reason something couldn't be done, saying what to do next.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct AppError(pub String);

impl AppError {
    fn new(msg: impl Into<String>) -> Self {
        Self(msg.into())
    }
}

/// Where changes go, and how commands run.
pub struct Env {
    pub paths: Paths,
    runner: Box<dyn Runner>,
    /// Writes go into a sandbox folder and commands are only recorded.
    pub sandbox: bool,
    system_pulse: Option<String>,
    pulse_override: bool,
    tools: Vec<String>,
}

impl Env {
    /// `writes`: the caller will change files. With a fixture, that's only allowed in a sandbox.
    pub fn new(fixture: bool, sandbox: Option<&Path>, writes: bool) -> Result<Self, AppError> {
        if let Some(dir) = sandbox {
            std::fs::create_dir_all(dir).map_err(|e| {
                AppError(format!(
                    "Couldn't create the sandbox {}: {e}.",
                    dir.display()
                ))
            })?;
            let dir = dir.canonicalize().map_err(|e| {
                AppError(format!("Couldn't open the sandbox {}: {e}.", dir.display()))
            })?;
            let mut paths = Paths::new(dir.join("home"), None, None);
            paths.system_pulse_daemon_conf = dir.join("etc/pulse/daemon.conf");
            let system_pulse = std::fs::read_to_string(&paths.system_pulse_daemon_conf).ok();
            return Ok(Self {
                paths,
                runner: Box::new(RecordingRunner::default()),
                sandbox: true,
                system_pulse,
                pulse_override: false,
                tools: stack::TOOLS.iter().map(|t| t.to_string()).collect(),
            });
        }
        if fixture && writes {
            return Err(AppError::new(
                "--fixture reads a captured system, so changes would land in your real config \
                 files. Add --sandbox <dir> to try changes safely, or use `ratewise plan`.",
            ));
        }
        let var = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty());
        let Some(home) = var("HOME").map(PathBuf::from).filter(|h| h.is_absolute()) else {
            return Err(AppError::new(
                "HOME isn't set, so Ratewise doesn't know where your config folders are.",
            ));
        };
        let paths = Paths::new(
            home,
            var("XDG_CONFIG_HOME").map(Into::into),
            var("XDG_STATE_HOME").map(Into::into),
        );
        let system_pulse = std::fs::read_to_string(&paths.system_pulse_daemon_conf).ok();
        Ok(Self {
            paths,
            runner: Box::new(SystemRunner),
            sandbox: false,
            system_pulse,
            pulse_override: var("PULSE_CONFIG").is_some(),
            tools: stack::find_tools()
                .into_iter()
                .filter(|t| t.path.is_some())
                .map(|t| t.name.to_string())
                .collect(),
        })
    }

    /// For showing devices when [`Env::new`] failed (e.g. no HOME). Nothing is read
    /// from or written to these paths, and commands are only recorded.
    pub fn unavailable() -> Self {
        Self {
            paths: Paths::new(PathBuf::from("/nonexistent"), None, None),
            runner: Box::new(RecordingRunner::default()),
            sandbox: false,
            system_pulse: None,
            pulse_override: false,
            tools: Vec::new(),
        }
    }
}

/// Everything a plan reads, owned, so it can cross threads.
#[derive(Debug, Clone)]
pub struct Inputs {
    pub snapshot: Snapshot,
    pub paths: Paths,
    pub saved: SavedConfig,
    /// Why the saved record couldn't be read, when it couldn't.
    pub saved_error: Option<String>,
    pub files: BTreeMap<PathBuf, FileState>,
    pub system_pulse: Option<String>,
    pub tools: Vec<String>,
    pub pulse_override: bool,
    pub sandbox: bool,
    /// The backup `revert` would restore.
    pub latest_backup: Option<BackupView>,
}

impl Inputs {
    pub fn ctx(&self) -> PlanContext<'_> {
        PlanContext {
            snapshot: &self.snapshot,
            paths: &self.paths,
            saved: &self.saved,
            files: &self.files,
            system_pulse_daemon_conf: self.system_pulse.as_deref(),
            tools: &self.tools,
            pulse_config_override: self.pulse_override,
        }
    }
}

/// How a plan was carried out.
#[derive(Debug)]
pub struct Outcome {
    pub applied: Applied,
    pub restart: RestartOutcome,
    /// Requested vs. running, when it makes sense to check now.
    pub checks: Option<Vec<Check>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestartOutcome {
    /// The plan needs no restart.
    NotNeeded,
    /// A restart is needed; the user didn't ask for it.
    Pending,
    Done(String),
    /// Sandbox: these would have run.
    Sandbox(Vec<Cmd>),
    Failed(String),
}

/// A probe source plus where changes go.
pub struct Session {
    pub probes: Probes,
    pub env: Env,
    /// Kernel readings from the last full probe. Capabilities don't change while running,
    /// and probing opens idle devices, so light refreshes reuse them.
    kernel_cache: Option<Vec<KernelPcmProbe>>,
}

impl Session {
    pub fn new(probes: Probes, env: Env) -> Self {
        Self {
            probes,
            env,
            kernel_cache: None,
        }
    }

    /// Probes everything (including the kernel) and reads Ratewise's files.
    pub fn inputs(&mut self) -> Inputs {
        let g = self.probes.gather();
        if let Ok(k) = &g.kernel {
            self.kernel_cache = Some(k.clone());
        }
        self.inputs_from(crate::probe::snapshot_from(g))
    }

    /// Like [`Self::inputs`], but reuses the last kernel readings: cheap enough to poll.
    pub fn inputs_light(&mut self) -> Inputs {
        let snap = match &self.kernel_cache {
            Some(k) => crate::probe::snapshot_from(self.probes.gather_with_kernel(Ok(k.clone()))),
            None => return self.inputs(),
        };
        self.inputs_from(snap)
    }

    fn inputs_from(&self, snapshot: Snapshot) -> Inputs {
        let paths = self.env.paths.clone();
        let (saved, saved_error) = match backend::load_saved(&paths) {
            Ok(s) => (s, None),
            Err(e) => (SavedConfig::default(), Some(e)),
        };
        let latest_backup = backend::latest_backup(&paths).ok().flatten();
        Inputs {
            snapshot,
            files: backend::read_files(&paths),
            paths,
            saved,
            saved_error,
            system_pulse: self.env.system_pulse.clone(),
            tools: self.env.tools.clone(),
            pulse_override: self.env.pulse_override,
            sandbox: self.env.sandbox,
            latest_backup,
        }
    }

    /// Inputs for a plan that needs a readable record (`set`; `reset` and `revert` don't).
    pub fn inputs_for_set(&mut self) -> Result<Inputs, AppError> {
        let i = self.inputs();
        match &i.saved_error {
            Some(e) => Err(AppError(e.clone())),
            None => Ok(i),
        }
    }

    /// Applies `plan`; with `restart`, restarts the services it needs. `revert_id`: the
    /// backup this plan restores, marked as used afterwards.
    pub fn execute(
        &self,
        plan: &Plan,
        restart: bool,
        revert_id: Option<&str>,
    ) -> Result<Outcome, AppError> {
        let now_unix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let applied = backend::apply(
            plan,
            &self.env.paths,
            self.env.runner.as_ref(),
            &Options {
                backup: revert_id.is_none(),
                now_unix,
            },
        )
        .map_err(|e| AppError(e.to_string()))?;
        if let Some(id) = revert_id {
            backend::mark_reverted(&self.env.paths, id).map_err(|e| {
                AppError(format!(
                    "Reverted, but couldn't mark backup {id} as used: {e}."
                ))
            })?;
        }
        let restart = match &plan.restart {
            None => RestartOutcome::NotNeeded,
            Some(r) if !restart || r.commands.is_empty() => RestartOutcome::Pending,
            Some(r) => match backend::restart(r, self.env.runner.as_ref()) {
                Ok(()) if self.env.sandbox => RestartOutcome::Sandbox(r.commands.clone()),
                Ok(()) => RestartOutcome::Done(r.services.clone()),
                Err(e) => RestartOutcome::Failed(e),
            },
        };
        let live = matches!(plan.kind, PlanKind::Set(Mode::Live)) || plan.kind == PlanKind::Reset;
        let restarted = matches!(restart, RestartOutcome::Done(_));
        let checks = (!plan.expect.is_empty() && (live || restarted) && !self.env.sandbox)
            .then(|| plan::verify(&plan.expect, &self.settle(restarted)));
        Ok(Outcome {
            applied,
            restart,
            checks,
        })
    }

    /// Re-probes. After a restart, waits a few seconds for PipeWire to come back.
    fn settle(&self, restarted: bool) -> Snapshot {
        let mut snap = self.probes.snapshot();
        if restarted {
            for _ in 0..10 {
                if snap.stack.server == SoundServer::PipeWire && snap.clock.is_some() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(500));
                snap = self.probes.snapshot();
            }
        }
        snap
    }
}

/// What happened, as `ratewise set` prints it.
pub fn render_outcome(plan: &Plan, o: &Outcome, paths: &Paths, sandbox: bool) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "{}", plan.title);
    let a = &o.applied;
    for f in &a.written {
        let _ = writeln!(s, "  Wrote {}", paths.display(f));
    }
    for f in &a.removed {
        let _ = writeln!(s, "  Removed {}", paths.display(f));
    }
    for c in &a.ran {
        if sandbox {
            let _ = writeln!(s, "  Would run (sandbox): {}", c.display());
        } else {
            let _ = writeln!(s, "  Ran {}", c.display());
        }
    }
    if let Some(id) = &a.backup {
        let _ = writeln!(
            s,
            "  Backed up to {}. `ratewise revert` undoes this.",
            paths.display(&paths.backups().join(id))
        );
    }
    if !plan.notes.is_empty() {
        let _ = writeln!(s, "\nNotes");
        for n in &plan.notes {
            let _ = writeln!(s, "  - {n}");
        }
    }
    match (&o.restart, &plan.restart) {
        (RestartOutcome::Sandbox(cmds), _) => {
            let _ = writeln!(s, "\nRestart (sandbox, not run):");
            for c in cmds {
                let _ = writeln!(s, "  {}", c.display());
            }
        }
        (RestartOutcome::Done(services), _) => {
            let _ = writeln!(s, "\nRestarted {services}.");
        }
        (RestartOutcome::Failed(e), Some(r)) => {
            let _ = writeln!(
                s,
                "\nThe settings are saved, but restarting {} didn't work: {e}. Restart them \
                 yourself, or log out and back in.",
                r.services
            );
        }
        (RestartOutcome::Pending, Some(r)) => {
            let _ = writeln!(s, "\nAfterwards");
            s.push_str(&restart_text(r));
        }
        _ => {}
    }
    if let Some(after) = &plan.afterwards {
        let _ = writeln!(s, "\n{after}");
    }
    match &o.checks {
        Some(checks) => {
            let _ = writeln!(s, "\nCheck");
            s.push_str(&render_checks(checks));
        }
        None if sandbox && !plan.expect.is_empty() => {
            let _ = writeln!(
                s,
                "\nIn a sandbox nothing really changed, so there's nothing to check."
            );
        }
        None => {}
    }
    s
}

/// For a sandboxed GUI: the folder that stands in for home.
pub fn sandbox_note(paths: &Paths) -> String {
    format!(
        "Sandbox: changes go to {} and commands are only recorded.",
        paths.home.display()
    )
}
