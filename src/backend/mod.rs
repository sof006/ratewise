//! Carries out plans (spec §4). This is the only code that changes files or runs
//! commands that change the system. Before anything is written, it checks every
//! target is allowed and unchanged since the plan was made, and makes a backup.
//! If a step fails, the steps already done are undone.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::core::paths::Paths;
use crate::core::plan::{BackupView, Cmd, FileState, Plan, Restart, Step};
use crate::core::settings::SavedConfig;
use crate::probe::command;

const MANIFEST: &str = "manifest.json";
const MANIFEST_VERSION: u32 = 1;

/// Runs the commands in a plan.
pub trait Runner {
    /// `Err` holds a plain-English reason.
    fn run(&self, cmd: &Cmd) -> Result<(), String>;
}

/// The real system.
pub struct SystemRunner;

impl Runner for SystemRunner {
    fn run(&self, cmd: &Cmd) -> Result<(), String> {
        let args: Vec<&str> = cmd.args.iter().map(String::as_str).collect();
        match command::run(&cmd.program, &args, Duration::from_secs(20)) {
            Ok(out) if out.status.success() => Ok(()),
            Ok(out) => Err(out
                .stderr
                .lines()
                .chain(out.stdout.lines())
                .map(str::trim)
                .find(|l| !l.is_empty())
                .map(String::from)
                .unwrap_or_else(|| format!("it exited with {}", out.status))),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// Records commands instead of running them: sandbox mode and tests.
#[derive(Default)]
pub struct RecordingRunner {
    pub ran: Mutex<Vec<Cmd>>,
    /// A program name that should fail, to test rollback.
    pub fail: Option<String>,
}

impl RecordingRunner {
    pub fn commands(&self) -> Vec<Cmd> {
        self.ran.lock().map(|r| r.clone()).unwrap_or_default()
    }
}

impl Runner for RecordingRunner {
    fn run(&self, cmd: &Cmd) -> Result<(), String> {
        if self.fail.as_deref() == Some(cmd.program.as_str()) {
            return Err("simulated failure".into());
        }
        if let Ok(mut r) = self.ran.lock() {
            r.push(cmd.clone());
        }
        Ok(())
    }
}

// ---- reading -----------------------------------------------------------------------

pub fn read_state(path: &Path) -> FileState {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return FileState::Missing,
        Err(e) => return FileState::Unreadable(e.to_string()),
        Ok(m) if m.file_type().is_symlink() => return FileState::Symlink,
        Ok(m) if !m.is_file() => return FileState::Unreadable("it isn't a regular file".into()),
        Ok(_) => {}
    }
    match fs::read_to_string(path) {
        Ok(t) => FileState::Text(t),
        Err(e) if e.kind() == io::ErrorKind::InvalidData => {
            FileState::Unreadable("it isn't valid UTF-8 text".into())
        }
        Err(e) => FileState::Unreadable(e.to_string()),
    }
}

/// The current state of every file a plan may touch.
pub fn read_files(paths: &Paths) -> BTreeMap<PathBuf, FileState> {
    paths
        .managed()
        .into_iter()
        .map(|p| {
            let s = read_state(&p);
            (p, s)
        })
        .collect()
}

/// Ratewise's saved record. A missing file is an empty record.
pub fn load_saved(paths: &Paths) -> Result<SavedConfig, String> {
    match read_state(&paths.saved()) {
        FileState::Missing => Ok(SavedConfig::default()),
        FileState::Text(t) => serde_json::from_str(&t).map_err(|e| {
            format!(
                "{} is damaged ({e}). `ratewise reset` removes it and every file Ratewise wrote.",
                paths.display(&paths.saved())
            )
        }),
        FileState::Symlink => Err(format!(
            "{} is a symbolic link.",
            paths.display(&paths.saved())
        )),
        FileState::Unreadable(e) => Err(format!(
            "Couldn't read {}: {e}.",
            paths.display(&paths.saved())
        )),
    }
}

// ---- applying ----------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum ApplyError {
    #[error(
        "Ratewise won't write {0}: it's outside the folders it may change. Nothing was changed."
    )]
    NotAllowed(String),
    #[error(
        "{0} changed since the plan was made (another program may have written it). Nothing was \
         changed; run the command again."
    )]
    Changed(String),
    #[error("Couldn't make a backup in {path}: {source}. Nothing was changed.")]
    Backup { path: String, source: io::Error },
    #[error("Couldn't {action} {path}: {source}. {recovery}")]
    File {
        action: &'static str,
        path: String,
        source: io::Error,
        recovery: String,
    },
    #[error("`{cmd}` failed: {message}. {recovery}")]
    Command {
        cmd: String,
        message: String,
        recovery: String,
    },
}

pub struct Options {
    /// Make a backup first (not for a revert, which consumes one).
    pub backup: bool,
    pub now_unix: u64,
}

#[derive(Debug, Default)]
pub struct Applied {
    /// The backup id, for `ratewise revert`.
    pub backup: Option<String>,
    pub written: Vec<PathBuf>,
    pub removed: Vec<PathBuf>,
    pub ran: Vec<Cmd>,
}

pub fn apply(
    plan: &Plan,
    paths: &Paths,
    runner: &dyn Runner,
    opts: &Options,
) -> Result<Applied, ApplyError> {
    // 1. Every target is allowed, a regular file or absent, and unchanged since the plan.
    for step in &plan.steps {
        let (path, expected) = match step {
            Step::Write { path, old, .. } => (path, old.as_deref()),
            Step::Remove { path, old, .. } => (path, Some(old.as_str())),
            Step::Run { .. } => continue,
        };
        let shown = paths.display(path);
        if !paths.is_allowed(path) {
            return Err(ApplyError::NotAllowed(shown));
        }
        let now = match read_state(path) {
            FileState::Missing => None,
            FileState::Text(t) => Some(t),
            FileState::Symlink | FileState::Unreadable(_) => {
                return Err(ApplyError::Changed(shown));
            }
        };
        if now.as_deref() != expected {
            return Err(ApplyError::Changed(shown));
        }
    }

    let mut applied = Applied::default();
    if plan.steps.is_empty() {
        return Ok(applied);
    }
    if opts.backup {
        applied.backup = Some(make_backup(plan, paths, opts.now_unix)?);
    }

    // 2. Files.
    let mut done: Vec<&Step> = Vec::new();
    for step in &plan.steps {
        let result = match step {
            Step::Write { path, new, .. } => write_atomic(path, new).map(|()| ("write", path)),
            Step::Remove { path, .. } => remove(path).map(|()| ("remove", path)),
            Step::Run { .. } => continue,
        };
        match result {
            Ok((action, path)) => {
                if action == "write" {
                    applied.written.push(path.clone());
                } else {
                    applied.removed.push(path.clone());
                }
                done.push(step);
            }
            Err(source) => {
                let path = step.path().map(|p| paths.display(p)).unwrap_or_default();
                let recovery = roll_back_files(&done, &applied.backup, paths);
                return Err(ApplyError::File {
                    action: if matches!(step, Step::Remove { .. }) {
                        "remove"
                    } else {
                        "write"
                    },
                    path,
                    source,
                    recovery,
                });
            }
        }
    }

    // 3. Commands.
    let mut undo: Vec<&Cmd> = Vec::new();
    for step in &plan.steps {
        let Step::Run { cmd, undo: u, .. } = step else {
            continue;
        };
        if let Err(message) = runner.run(cmd) {
            let mut recovery = String::new();
            for u in undo.iter().rev() {
                if runner.run(u).is_err() {
                    recovery = format!("Couldn't undo `{}` either. ", u.display());
                }
            }
            recovery.push_str(&roll_back_files(&done, &applied.backup, paths));
            return Err(ApplyError::Command {
                cmd: cmd.display(),
                message,
                recovery,
            });
        }
        applied.ran.push(cmd.clone());
        if let Some(u) = u {
            undo.push(u);
        }
    }
    Ok(applied)
}

/// Puts back files from a failed apply, using the contents the plan recorded.
fn roll_back_files(done: &[&Step], backup: &Option<String>, paths: &Paths) -> String {
    if done.is_empty() {
        return "Nothing was changed.".into();
    }
    let mut failed = Vec::new();
    for step in done.iter().rev() {
        let r = match step {
            Step::Write {
                path,
                old: Some(old),
                ..
            }
            | Step::Remove { path, old, .. } => write_atomic(path, old),
            Step::Write {
                path, old: None, ..
            } => remove(path),
            Step::Run { .. } => Ok(()),
        };
        if r.is_err()
            && let Some(p) = step.path()
        {
            failed.push(paths.display(p));
        }
    }
    match (failed.is_empty(), backup) {
        (true, _) => "Ratewise put back the files it had already changed.".into(),
        (false, Some(id)) => format!(
            "These files couldn't be put back: {}. `ratewise revert` restores them from backup {id}.",
            failed.join(", ")
        ),
        (false, None) => format!("These files couldn't be put back: {}.", failed.join(", ")),
    }
}

/// Runs the restart commands of a plan, in order.
pub fn restart(r: &Restart, runner: &dyn Runner) -> Result<(), String> {
    for cmd in &r.commands {
        runner
            .run(cmd)
            .map_err(|e| format!("`{}` failed: {e}", cmd.display()))?;
    }
    Ok(())
}

fn write_atomic(path: &Path, contents: &str) -> io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no parent folder"))?;
    fs::create_dir_all(dir)?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy())
        .unwrap_or_default();
    let tmp = dir.join(format!(".{name}.ratewise-tmp"));
    let permissions = fs::metadata(path).ok().map(|m| m.permissions());
    let result = (|| {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(contents.as_bytes())?;
        f.sync_all()?;
        if let Some(p) = permissions {
            fs::set_permissions(&tmp, p)?;
        }
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

fn remove(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        r => r,
    }
}

// ---- backups -----------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub version: u32,
    pub created_unix: u64,
    pub description: String,
    pub files: Vec<BackedUpFile>,
    #[serde(default)]
    pub live_undo: Vec<Cmd>,
    #[serde(default)]
    pub reverted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackedUpFile {
    pub path: PathBuf,
    /// The copy's name inside the backup folder; `None` if the file didn't exist.
    pub copy: Option<String>,
}

fn make_backup(plan: &Plan, paths: &Paths, now: u64) -> Result<String, ApplyError> {
    let base = paths.backups();
    let stamp = timestamp(now);
    let (id, dir) = (1..)
        .map(|n| {
            if n == 1 {
                stamp.clone()
            } else {
                format!("{stamp}-{n}")
            }
        })
        .map(|id| {
            let dir = base.join(&id);
            (id, dir)
        })
        .find(|(_, dir)| !dir.exists())
        .expect("an unused name exists");
    let err = |source| ApplyError::Backup {
        path: paths.display(&dir),
        source,
    };
    fs::create_dir_all(dir.join("files")).map_err(err)?;
    let mut files = Vec::new();
    let mut live_undo = Vec::new();
    for (i, step) in plan.steps.iter().enumerate() {
        match step {
            Step::Write { path, old, .. } => {
                let copy = match old {
                    Some(text) => {
                        let name = format!("files/{i}");
                        fs::write(dir.join(&name), text).map_err(err)?;
                        Some(name)
                    }
                    None => None,
                };
                files.push(BackedUpFile {
                    path: path.clone(),
                    copy,
                });
            }
            Step::Remove { path, old, .. } => {
                let name = format!("files/{i}");
                fs::write(dir.join(&name), old).map_err(err)?;
                files.push(BackedUpFile {
                    path: path.clone(),
                    copy: Some(name),
                });
            }
            Step::Run { undo: Some(u), .. } => live_undo.push(u.clone()),
            Step::Run { .. } => {}
        }
    }
    // Undo live changes in reverse order.
    live_undo.reverse();
    let manifest = Manifest {
        version: MANIFEST_VERSION,
        created_unix: now,
        description: plan.title.clone(),
        files,
        live_undo,
        reverted: false,
    };
    let json = serde_json::to_string_pretty(&manifest).expect("manifest serializes") + "\n";
    write_atomic(&dir.join(MANIFEST), &json).map_err(err)?;
    Ok(id)
}

/// Backups, oldest first.
pub fn list_backups(paths: &Paths) -> Vec<(String, Manifest)> {
    let Ok(entries) = fs::read_dir(paths.backups()) else {
        return Vec::new();
    };
    let mut out: Vec<(String, Manifest)> = entries
        .flatten()
        .filter_map(|e| {
            let id = e.file_name().to_string_lossy().into_owned();
            let text = fs::read_to_string(e.path().join(MANIFEST)).ok()?;
            let m: Manifest = serde_json::from_str(&text).ok()?;
            Some((id, m))
        })
        .collect();
    out.sort_by(|a, b| (a.1.created_unix, &a.0).cmp(&(b.1.created_unix, &b.0)));
    out
}

/// The most recent backup that hasn't been reverted, ready for `plan_revert`.
pub fn latest_backup(paths: &Paths) -> Result<Option<BackupView>, String> {
    let Some((id, m)) = list_backups(paths)
        .into_iter()
        .rev()
        .find(|(_, m)| !m.reverted)
    else {
        return Ok(None);
    };
    let dir = paths.backups().join(&id);
    let mut files = Vec::new();
    for f in &m.files {
        let contents = match &f.copy {
            Some(name) => {
                if name.contains("..") || name.starts_with('/') {
                    return Err(format!(
                        "Backup {id} is damaged: it names an odd file ({name})."
                    ));
                }
                Some(fs::read_to_string(dir.join(name)).map_err(|e| {
                    format!("Backup {id} is incomplete: couldn't read {name} ({e}).")
                })?)
            }
            None => None,
        };
        files.push((f.path.clone(), contents));
    }
    Ok(Some(BackupView {
        id,
        description: m.description,
        files,
        live_undo: m.live_undo,
    }))
}

pub fn mark_reverted(paths: &Paths, id: &str) -> io::Result<()> {
    let path = paths.backups().join(id).join(MANIFEST);
    let mut m: Manifest = serde_json::from_str(&fs::read_to_string(&path)?)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    m.reverted = true;
    write_atomic(
        &path,
        &(serde_json::to_string_pretty(&m).expect("manifest serializes") + "\n"),
    )
}

/// `20261107-153012` (UTC), for backup folder names.
pub fn timestamp(unix: u64) -> String {
    let days = (unix / 86_400) as i64;
    let secs = unix % 86_400;
    // Howard Hinnant's civil-from-days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}{m:02}{d:02}-{:02}{:02}{:02}",
        secs / 3600,
        secs / 60 % 60,
        secs % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps() {
        assert_eq!(timestamp(0), "19700101-000000");
        assert_eq!(timestamp(951_782_400), "20000229-000000");
        assert_eq!(timestamp(1_794_053_001), "20261107-120321");
    }

    #[test]
    fn atomic_write_keeps_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("ratewise-atomic-{}", std::process::id()));
        let file = dir.join("sub/f.conf");
        write_atomic(&file, "one\n").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
        write_atomic(&file, "two\n").unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), "two\n");
        assert_eq!(
            fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(!dir.join("sub/.f.conf.ratewise-tmp").exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn read_state_kinds() {
        let dir = std::env::temp_dir().join(format!("ratewise-state-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("t"), "x").unwrap();
        std::os::unix::fs::symlink(dir.join("t"), dir.join("l")).unwrap();
        fs::write(dir.join("bin"), [0xff, 0xfe]).unwrap();
        assert_eq!(read_state(&dir.join("t")), FileState::Text("x".into()));
        assert_eq!(read_state(&dir.join("l")), FileState::Symlink);
        assert_eq!(read_state(&dir.join("none")), FileState::Missing);
        assert!(matches!(
            read_state(&dir.join("bin")),
            FileState::Unreadable(_)
        ));
        assert!(matches!(read_state(&dir), FileState::Unreadable(_)));
        fs::remove_dir_all(&dir).unwrap();
    }
}
