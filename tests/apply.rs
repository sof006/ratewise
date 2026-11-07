//! The backend carrying plans out in a temporary home: backups, revert, refusals
//! and rollback. Commands go to a `RecordingRunner`, so nothing touches the system.

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::fixture;
use ratewise::backend::{self, ApplyError, Options, RecordingRunner};
use ratewise::core::model::Snapshot;
use ratewise::core::paths::Paths;
use ratewise::core::plan::{self, Cmd, Plan, PlanContext, PlanKind, SetRequest, Step};
use ratewise::core::settings::{Change, Mode, Value};
use ratewise::probe::Probes;

const NOW: u64 = 1_794_053_001; // 2026-11-07 12:03:21 UTC

fn home(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("apply")
        .join(name);
    let _ = make_writable(&dir);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[cfg(unix)]
fn make_writable(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    for e in walk(dir) {
        if e.is_dir() {
            fs::set_permissions(&e, fs::Permissions::from_mode(0o755))?;
        }
    }
    Ok(())
}
#[cfg(not(unix))]
fn make_writable(_: &Path) -> std::io::Result<()> {
    Ok(())
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = vec![dir.to_path_buf()];
    if let Ok(entries) = fs::read_dir(dir) {
        for e in entries.flatten() {
            if e.path().is_dir() {
                out.extend(walk(&e.path()));
            }
        }
    }
    out
}

fn snapshot(name: &str) -> Snapshot {
    Probes::fixture(&fixture(name)).snapshot()
}

/// Plans `change` against the files currently in `paths`.
fn plan_set(snap: &Snapshot, paths: &Paths, change: Change, device: Option<&str>) -> Plan {
    let files = backend::read_files(paths);
    let saved = backend::load_saved(paths).unwrap();
    let tools: Vec<String> = ["pw-metadata", "systemctl"].map(String::from).to_vec();
    let ctx = PlanContext {
        snapshot: snap,
        paths,
        saved: &saved,
        files: &files,
        system_pulse_daemon_conf: None,
        tools: &tools,
        pulse_config_override: false,
    };
    let d = device.map(|q| ratewise::core::select::select(&snap.devices, q).unwrap());
    plan::plan_set(
        &ctx,
        &SetRequest {
            change,
            device: d,
            mode: Some(Mode::Save),
        },
    )
    .unwrap()
}

fn revert_plan(snap: &Snapshot, paths: &Paths) -> (String, Plan) {
    let backup = backend::latest_backup(paths).unwrap().expect("a backup");
    let files = backend::read_files(paths);
    let saved = backend::load_saved(paths).unwrap_or_default();
    let ctx = PlanContext {
        snapshot: snap,
        paths,
        saved: &saved,
        files: &files,
        system_pulse_daemon_conf: None,
        tools: &[],
        pulse_config_override: false,
    };
    (backup.id.clone(), plan::plan_revert(&ctx, &backup).unwrap())
}

fn opts(backup: bool) -> Options {
    Options {
        backup,
        now_unix: NOW,
    }
}

#[test]
fn backup_and_revert_round_trip_keeps_user_content() {
    let dir = home("round-trip");
    let paths = Paths::new(dir.clone(), None, None);
    let user =
        "# mine\r\npcm.!default { type plug; slave.pcm \"hw:1,0\" }  \n\n# trailing, no newline";
    fs::write(paths.asoundrc(), user).unwrap();
    let snap = snapshot("alsa-only");
    let runner = RecordingRunner::default();

    let first = plan_set(&snap, &paths, Change::Rate(Value::Set(48_000)), None);
    let applied = backend::apply(&first, &paths, &runner, &opts(true)).unwrap();
    assert_eq!(applied.backup.as_deref(), Some("20261107-120321"));
    let rc = fs::read_to_string(paths.asoundrc()).unwrap();
    assert!(rc.starts_with(user), "{rc}");
    assert!(rc.contains("defaults.pcm.dmix.rate 48000"));
    assert!(paths.saved().is_file());

    // A second change in the same second gets its own backup.
    let second = plan_set(
        &snap,
        &paths,
        Change::PeriodSize(Value::Set(512)),
        Some("alsa:Device:0:capture"),
    );
    let applied = backend::apply(&second, &paths, &runner, &opts(true)).unwrap();
    assert_eq!(applied.backup.as_deref(), Some("20261107-120321-2"));

    // Revert undoes the latest change only, then the one before.
    let (id, undo) = revert_plan(&snap, &paths);
    assert_eq!(id, "20261107-120321-2");
    assert_eq!(undo.kind, PlanKind::Revert);
    backend::apply(&undo, &paths, &runner, &opts(false)).unwrap();
    backend::mark_reverted(&paths, &id).unwrap();
    let rc = fs::read_to_string(paths.asoundrc()).unwrap();
    assert!(
        rc.contains("dmix.rate 48000") && !rc.contains("period_size"),
        "{rc}"
    );

    let (id, undo) = revert_plan(&snap, &paths);
    assert_eq!(id, "20261107-120321");
    backend::apply(&undo, &paths, &runner, &opts(false)).unwrap();
    backend::mark_reverted(&paths, &id).unwrap();
    assert_eq!(
        fs::read_to_string(paths.asoundrc()).unwrap(),
        user,
        "byte for byte"
    );
    assert!(!paths.saved().exists());
    assert!(backend::latest_backup(&paths).unwrap().is_none());
    assert!(runner.commands().is_empty());
}

#[test]
fn pipewire_files_land_where_pipewire_reads_them() {
    let dir = home("pipewire");
    let paths = Paths::new(dir.clone(), None, None);
    let snap = snapshot("pw-1.2-desktop");
    let runner = RecordingRunner::default();
    let p = plan_set(&snap, &paths, Change::Rate(Value::Set(96_000)), None);
    backend::apply(&p, &paths, &runner, &opts(true)).unwrap();
    let p = plan_set(
        &snap,
        &paths,
        Change::Suspend(Value::Set(false)),
        Some("alsa_output.usb-Topping_D50s-00.analog-stereo"),
    );
    backend::apply(&p, &paths, &runner, &opts(true)).unwrap();
    for f in [
        ".config/pipewire/pipewire.conf.d/60-ratewise.conf",
        ".config/wireplumber/wireplumber.conf.d/60-ratewise.conf",
        ".local/state/ratewise/saved.json",
    ] {
        assert!(dir.join(f).is_file(), "{f}");
    }
    // No temporary files left behind.
    for d in walk(&dir) {
        for e in fs::read_dir(&d).unwrap().flatten() {
            assert!(!e.file_name().to_string_lossy().contains("ratewise-tmp"));
        }
    }
    let wp = fs::read_to_string(paths.wireplumber_05()).unwrap();
    assert!(wp.contains("session.suspend-timeout-seconds = 0"), "{wp}");
}

#[test]
fn a_file_changed_since_the_plan_stops_everything() {
    let dir = home("changed");
    let paths = Paths::new(dir.clone(), None, None);
    let snap = snapshot("alsa-only");
    let p = plan_set(&snap, &paths, Change::Rate(Value::Set(48_000)), None);
    fs::write(paths.asoundrc(), "# written meanwhile\n").unwrap();
    let err = backend::apply(&p, &paths, &RecordingRunner::default(), &opts(true)).unwrap_err();
    assert!(matches!(err, ApplyError::Changed(_)), "{err}");
    assert!(err.to_string().contains("Nothing was changed"));
    assert_eq!(
        fs::read_to_string(paths.asoundrc()).unwrap(),
        "# written meanwhile\n"
    );
    assert!(!paths.backups().exists(), "no backup for a refused plan");
}

#[cfg(unix)]
#[test]
fn a_symlink_appearing_after_the_plan_is_refused() {
    let dir = home("symlink");
    let paths = Paths::new(dir.clone(), None, None);
    let snap = snapshot("alsa-only");
    let p = plan_set(&snap, &paths, Change::Rate(Value::Set(48_000)), None);
    let target = dir.join("elsewhere");
    std::os::unix::fs::symlink(&target, paths.asoundrc()).unwrap();
    let err = backend::apply(&p, &paths, &RecordingRunner::default(), &opts(true)).unwrap_err();
    assert!(matches!(err, ApplyError::Changed(_)), "{err}");
    assert!(!target.exists());
}

#[test]
fn paths_outside_the_allowed_folders_are_refused() {
    let dir = home("not-allowed");
    let paths = Paths::new(dir.clone(), None, None);
    let plan = Plan {
        kind: PlanKind::Revert,
        title: "Bad".into(),
        steps: vec![Step::Write {
            path: dir.join(".bashrc"),
            old: None,
            new: "x".into(),
            why: String::new(),
            show_diff: false,
        }],
        notes: vec![],
        restart: None,
        afterwards: None,
        expect: vec![],
    };
    let err = backend::apply(&plan, &paths, &RecordingRunner::default(), &opts(true)).unwrap_err();
    assert!(matches!(err, ApplyError::NotAllowed(_)), "{err}");
    assert!(!dir.join(".bashrc").exists());
}

#[cfg(unix)]
#[test]
fn a_write_failure_puts_earlier_files_back() {
    use std::os::unix::fs::PermissionsExt;
    let dir = home("read-only");
    let paths = Paths::new(dir.clone(), None, None);
    let snap = snapshot("pw-1.2-desktop");
    // Resampler quality writes client.conf.d, then pipewire-pulse.conf.d.
    let p = plan_set(&snap, &paths, Change::ResampleQuality(Value::Set(10)), None);
    let blocked = paths
        .pipewire_pulse_dropin()
        .parent()
        .unwrap()
        .to_path_buf();
    fs::create_dir_all(&blocked).unwrap();
    fs::set_permissions(&blocked, fs::Permissions::from_mode(0o555)).unwrap();
    if fs::write(blocked.join("probe"), "").is_ok() {
        eprintln!("skipped: running as root, so permissions don't block writes");
        return;
    }
    let err = backend::apply(&p, &paths, &RecordingRunner::default(), &opts(true)).unwrap_err();
    fs::set_permissions(&blocked, fs::Permissions::from_mode(0o755)).unwrap();
    let msg = err.to_string();
    assert!(matches!(err, ApplyError::File { .. }), "{msg}");
    assert!(msg.contains("put back the files"), "{msg}");
    assert!(!paths.client_dropin().exists(), "rolled back");
    assert!(!paths.saved().exists());
}

#[test]
fn a_failed_command_undoes_earlier_commands_and_files() {
    let dir = home("failed-command");
    let paths = Paths::new(dir.clone(), None, None);
    let file = paths.pipewire_dropin();
    let set = Cmd::new(
        "pw-metadata",
        &["-n", "settings", "0", "clock.quantum", "256"],
    );
    let unset = Cmd::new(
        "pw-metadata",
        &["-n", "settings", "0", "clock.quantum", "1024"],
    );
    let plan = Plan {
        kind: PlanKind::Set(Mode::Live),
        title: "Test".into(),
        steps: vec![
            Step::Write {
                path: file.clone(),
                old: None,
                new: "x\n".into(),
                why: String::new(),
                show_diff: true,
            },
            Step::Run {
                cmd: set.clone(),
                why: String::new(),
                undo: Some(unset.clone()),
            },
            Step::Run {
                cmd: Cmd::new("broken", &[]),
                why: String::new(),
                undo: None,
            },
        ],
        notes: vec![],
        restart: None,
        afterwards: None,
        expect: vec![],
    };
    let runner = RecordingRunner {
        fail: Some("broken".into()),
        ..Default::default()
    };
    let err = backend::apply(&plan, &paths, &runner, &opts(true)).unwrap_err();
    assert!(matches!(err, ApplyError::Command { .. }), "{err}");
    assert!(
        err.to_string()
            .contains("`broken` failed: simulated failure"),
        "{err}"
    );
    assert_eq!(runner.commands(), vec![set, unset]);
    assert!(!file.exists());
}

#[test]
fn damaged_records_and_backups_are_reported_not_fatal() {
    let dir = home("damaged");
    let paths = Paths::new(dir.clone(), None, None);
    fs::create_dir_all(&paths.state).unwrap();
    fs::write(paths.saved(), "{ not json").unwrap();
    let err = backend::load_saved(&paths).unwrap_err();
    assert!(err.contains("`ratewise reset`"), "{err}");

    let b = paths.backups().join("20261107-120000");
    fs::create_dir_all(&b).unwrap();
    fs::write(
        b.join("manifest.json"),
        r#"{"version":1,"created_unix":1,"description":"x","files":[{"path":"/x","copy":"../../etc/passwd"}]}"#,
    )
    .unwrap();
    let err = backend::latest_backup(&paths).unwrap_err();
    assert!(err.contains("damaged"), "{err}");
}
