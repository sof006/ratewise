//! `ratewise set`, `plan`, `revert`, `reset` and `show`.

use std::fmt::Write as _;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, bail};

use crate::backend::{self, Options, RecordingRunner, Runner, SystemRunner};
use crate::core::model::{Device, Snapshot, SoundServer};
use crate::core::paths::Paths;
use crate::core::plan::{
    self, Plan, PlanContext, PlanKind, SetRequest, render_checks, render_plan, restart_text,
};
use crate::core::select::select;
use crate::core::settings::{Change, Mode, SavedConfig};
use crate::probe::{Probes, stack};

/// Where changes go, and how commands run.
pub struct Env {
    pub paths: Paths,
    runner: Box<dyn Runner>,
    sandbox: bool,
    system_pulse: Option<String>,
    pulse_override: bool,
    tools: Vec<String>,
}

impl Env {
    /// `writes`: the command changes files. With `--fixture`, that's only allowed in a sandbox.
    pub fn new(fixture: bool, sandbox: Option<&Path>, writes: bool) -> anyhow::Result<Self> {
        if let Some(dir) = sandbox {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("couldn't create the sandbox {}", dir.display()))?;
            let dir = dir
                .canonicalize()
                .with_context(|| format!("couldn't open the sandbox {}", dir.display()))?;
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
            bail!(
                "--fixture reads a captured system, so changes would land in your real config \
                 files. Add --sandbox <dir> to try changes safely, or use `ratewise plan`."
            );
        }
        let var = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty());
        let Some(home) = var("HOME")
            .map(std::path::PathBuf::from)
            .filter(|h| h.is_absolute())
        else {
            bail!("HOME isn't set, so Ratewise doesn't know where your config folders are.");
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
}

struct Loaded {
    snapshot: Snapshot,
    saved: SavedConfig,
    files: std::collections::BTreeMap<std::path::PathBuf, plan::FileState>,
}

fn load(probes: &Probes, env: &Env, tolerate_bad_record: bool) -> anyhow::Result<Loaded> {
    let saved = match backend::load_saved(&env.paths) {
        Ok(s) => s,
        Err(_) if tolerate_bad_record => SavedConfig::default(),
        Err(e) => bail!(e),
    };
    Ok(Loaded {
        snapshot: probes.snapshot(),
        saved,
        files: backend::read_files(&env.paths),
    })
}

fn context<'a>(env: &'a Env, l: &'a Loaded) -> PlanContext<'a> {
    PlanContext {
        snapshot: &l.snapshot,
        paths: &env.paths,
        saved: &l.saved,
        files: &l.files,
        system_pulse_daemon_conf: env.system_pulse.as_deref(),
        tools: &env.tools,
        pulse_config_override: env.pulse_override,
    }
}

pub struct Request<'a> {
    pub change: Change,
    pub device: Option<&'a str>,
    pub mode: Option<Mode>,
    pub restart: bool,
    pub dry_run: bool,
}

pub fn set(probes: &Probes, env: &Env, req: Request) -> anyhow::Result<()> {
    let loaded = load(probes, env, false)?;
    let device: Option<&Device> = match req.device {
        Some(q) => Some(select(&loaded.snapshot.devices, q)?),
        None => None,
    };
    let ctx = context(env, &loaded);
    let plan = plan::plan_set(
        &ctx,
        &SetRequest {
            change: req.change,
            device,
            mode: req.mode,
        },
    )?;
    if req.dry_run {
        return super::emit(&render_plan(&plan, &env.paths));
    }
    execute(probes, env, &plan, req.restart, true)
}

pub fn reset(probes: &Probes, env: &Env, restart: bool, dry_run: bool) -> anyhow::Result<()> {
    let loaded = load(probes, env, true)?;
    let plan = plan::plan_reset(&context(env, &loaded))?;
    if dry_run {
        return super::emit(&render_plan(&plan, &env.paths));
    }
    execute(probes, env, &plan, restart, true)
}

pub fn revert(probes: &Probes, env: &Env, restart: bool, dry_run: bool) -> anyhow::Result<()> {
    let Some(backup) = backend::latest_backup(&env.paths).map_err(anyhow::Error::msg)? else {
        return super::emit(&format!(
            "There's nothing to revert: Ratewise hasn't made any backups yet (they go in {}).\n",
            env.paths.display(&env.paths.backups())
        ));
    };
    let loaded = load(probes, env, true)?;
    let plan = plan::plan_revert(&context(env, &loaded), &backup)?;
    if dry_run {
        return super::emit(&render_plan(&plan, &env.paths));
    }
    execute(probes, env, &plan, restart, false)?;
    backend::mark_reverted(&env.paths, &backup.id)
        .with_context(|| format!("reverted, but couldn't mark backup {} as used", backup.id))
}

fn execute(
    probes: &Probes,
    env: &Env,
    plan: &Plan,
    restart: bool,
    backup: bool,
) -> anyhow::Result<()> {
    let mut s = String::new();
    let _ = writeln!(s, "{}", plan.title);
    if plan.is_empty() {
        for n in &plan.notes {
            let _ = writeln!(s, "  {n}");
        }
        return super::emit(&s);
    }
    let now_unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let applied = backend::apply(
        plan,
        &env.paths,
        env.runner.as_ref(),
        &Options { backup, now_unix },
    )?;
    let p = &env.paths;
    for f in &applied.written {
        let _ = writeln!(s, "  Wrote {}", p.display(f));
    }
    for f in &applied.removed {
        let _ = writeln!(s, "  Removed {}", p.display(f));
    }
    for c in &applied.ran {
        if env.sandbox {
            let _ = writeln!(s, "  Would run (sandbox): {}", c.display());
        } else {
            let _ = writeln!(s, "  Ran {}", c.display());
        }
    }
    if let Some(id) = &applied.backup {
        let _ = writeln!(
            s,
            "  Backed up to {}. `ratewise revert` undoes this.",
            p.display(&p.backups().join(id))
        );
    }
    if !plan.notes.is_empty() {
        let _ = writeln!(s, "\nNotes");
        for n in &plan.notes {
            let _ = writeln!(s, "  - {n}");
        }
    }

    let mut restarted = false;
    if let Some(r) = &plan.restart {
        if restart && !r.commands.is_empty() {
            match backend::restart(r, env.runner.as_ref()) {
                Ok(()) if env.sandbox => {
                    let _ = writeln!(s, "\nRestart (sandbox, not run):");
                    for c in &r.commands {
                        let _ = writeln!(s, "  {}", c.display());
                    }
                }
                Ok(()) => {
                    let _ = writeln!(s, "\nRestarted {}.", r.services);
                    restarted = true;
                }
                Err(e) => {
                    let _ = writeln!(
                        s,
                        "\nThe settings are saved, but restarting {} didn't work: {e}. Restart \
                         them yourself, or log out and back in.",
                        r.services
                    );
                }
            }
        } else {
            let _ = writeln!(s, "\nAfterwards");
            s.push_str(&restart_text(r));
        }
    } else if restart {
        let _ = writeln!(s, "\nNothing needs restarting.");
    }
    if let Some(a) = &plan.afterwards {
        let _ = writeln!(s, "\n{a}");
    }

    let live = matches!(plan.kind, PlanKind::Set(Mode::Live)) || plan.kind == PlanKind::Reset;
    if !plan.expect.is_empty() && (live || restarted) {
        if env.sandbox {
            let _ = writeln!(
                s,
                "\nIn a sandbox nothing really changed, so there's nothing to check."
            );
        } else {
            let after = settle(probes, restarted);
            let _ = writeln!(s, "\nCheck");
            s.push_str(&render_checks(&plan::verify(&plan.expect, &after)));
        }
    }
    super::emit(&s)
}

/// Re-probes. After a restart, waits a few seconds for PipeWire to come back.
fn settle(probes: &Probes, restarted: bool) -> Snapshot {
    let mut snap = probes.snapshot();
    if restarted {
        for _ in 0..10 {
            if snap.stack.server == SoundServer::PipeWire && snap.clock.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(500));
            snap = probes.snapshot();
        }
    }
    snap
}

pub fn show(probes: &Probes, env: Option<&Env>, query: &str, json: bool) -> anyhow::Result<()> {
    let snap = probes.snapshot();
    let d = select(&snap.devices, query)?;
    let saved = env
        .and_then(|e| backend::load_saved(&e.paths).ok())
        .and_then(|s| s.devices.get(&d.id).cloned());
    if json {
        let v = serde_json::json!({
            "schema_version": crate::core::model::SCHEMA_VERSION,
            "device": d,
            "saved": saved,
        });
        return super::emit(&(serde_json::to_string_pretty(&v)? + "\n"));
    }
    let mut s = String::new();
    super::list::device(&mut s, d);
    if let Some(saved) = saved {
        // Empty entries are never saved, so there's always at least one row.
        let _ = writeln!(s, "  Saved by Ratewise");
        let mut rows: Vec<(&str, String)> = Vec::new();
        if let Some(f) = &saved.format {
            rows.push(("format", f.to_string()));
        }
        if let Some(r) = saved.rate {
            rows.push(("rate", format!("{} kHz", crate::core::model::khz(r))));
        }
        if let Some(n) = saved.period_size {
            rows.push(("period", format!("{n} frames")));
        }
        if let Some(n) = saved.headroom {
            rows.push(("headroom", format!("{n} frames")));
        }
        if saved.disable_suspend {
            rows.push(("suspend", "off (kept open when idle)".into()));
        }
        for (k, v) in rows {
            let _ = writeln!(s, "    {k:<9} {v}");
        }
    }
    super::emit(&s)
}
