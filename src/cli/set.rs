//! `ratewise set`, `plan`, `revert`, `reset` and `show`.

use std::fmt::Write as _;

use crate::app::{Session, render_outcome};
use crate::backend;
use crate::core::model::Device;
use crate::core::plan::{self, Plan, SetRequest, render_plan};
use crate::core::select::select;
use crate::core::settings::{Change, Mode};

pub struct Request<'a> {
    pub change: Change,
    pub device: Option<&'a str>,
    pub mode: Option<Mode>,
    pub restart: bool,
    pub dry_run: bool,
}

pub fn set(session: &mut Session, req: Request) -> anyhow::Result<()> {
    let inputs = session.inputs_for_set()?;
    let device: Option<&Device> = match req.device {
        Some(q) => Some(select(&inputs.snapshot.devices, q)?),
        None => None,
    };
    let plan = plan::plan_set(
        &inputs.ctx(),
        &SetRequest {
            change: req.change,
            device,
            mode: req.mode,
        },
    )?;
    if req.dry_run {
        return super::emit(&render_plan(&plan, &inputs.paths));
    }
    execute(session, &plan, req.restart, None)
}

pub fn reset(session: &mut Session, restart: bool, dry_run: bool) -> anyhow::Result<()> {
    let inputs = session.inputs();
    let plan = plan::plan_reset(&inputs.ctx())?;
    if dry_run {
        return super::emit(&render_plan(&plan, &inputs.paths));
    }
    execute(session, &plan, restart, None)
}

pub fn revert(session: &mut Session, restart: bool, dry_run: bool) -> anyhow::Result<()> {
    let paths = &session.env.paths;
    let Some(backup) = backend::latest_backup(paths).map_err(anyhow::Error::msg)? else {
        return super::emit(&format!(
            "There's nothing to revert: Ratewise hasn't made any backups yet (they go in {}).\n",
            paths.display(&paths.backups())
        ));
    };
    let inputs = session.inputs();
    let plan = plan::plan_revert(&inputs.ctx(), &backup)?;
    if dry_run {
        return super::emit(&render_plan(&plan, &inputs.paths));
    }
    execute(session, &plan, restart, Some(&backup.id))
}

fn execute(
    session: &Session,
    plan: &Plan,
    restart: bool,
    revert_id: Option<&str>,
) -> anyhow::Result<()> {
    if plan.is_empty() {
        let mut s = String::new();
        let _ = writeln!(s, "{}", plan.title);
        for n in &plan.notes {
            let _ = writeln!(s, "  {n}");
        }
        return super::emit(&s);
    }
    let outcome = session.execute(plan, restart, revert_id)?;
    super::emit(&render_outcome(
        plan,
        &outcome,
        &session.env.paths,
        session.env.sandbox,
    ))
}

/// `paths`: where Ratewise's record is, if known (it isn't without `$HOME`).
pub fn show(
    probes: &crate::probe::Probes,
    paths: Option<&crate::core::paths::Paths>,
    query: &str,
    json: bool,
) -> anyhow::Result<()> {
    let snap = probes.snapshot();
    let d = select(&snap.devices, query)?;
    let saved = paths
        .and_then(|p| backend::load_saved(p).ok())
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
    if let Some(bt) = &d.bluetooth {
        let _ = writeln!(
            s,
            "  Bluetooth profiles (switch with `ratewise set profile <name> --device {}`)",
            d.id
        );
        for p in &bt.profiles {
            let mark = if bt.profile.as_deref() == Some(p.name.as_str()) {
                "*"
            } else {
                " "
            };
            let unavailable = if p.available { "" } else { "  (not available)" };
            let _ = writeln!(
                s,
                "  {mark} {:<26} {}{unavailable}",
                p.name,
                crate::core::bluetooth::profile_label(p)
            );
        }
    }
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
