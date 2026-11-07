//! The binary itself: exit codes and behaviour in hostile environments.

mod common;

use std::process::{Command, Stdio};

use common::fixture;

fn ratewise() -> Command {
    Command::new(env!("CARGO_BIN_EXE_ratewise"))
}

#[test]
fn help_and_version_exit_zero() {
    let out = ratewise().arg("--help").output().unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("doctor"));
    assert!(ratewise().arg("--version").status().unwrap().success());
}

#[test]
fn doctor_exits_zero_on_every_fixture() {
    for name in [
        "arch-desktop",
        "pw-1.2-desktop",
        "pw-0.3-laptop",
        "alsa-only",
        "no-sound",
    ] {
        let out = ratewise()
            .args(["doctor", "--fixture"])
            .arg(fixture(name))
            .output()
            .unwrap();
        assert!(out.status.success(), "{name}: {out:?}");
        assert!(String::from_utf8_lossy(&out.stdout).contains("system report"));
    }
}

#[test]
fn doctor_and_list_survive_an_empty_environment() {
    // No PATH (so no tools), no XDG_RUNTIME_DIR, no display, no HOME.
    for args in [&["doctor"][..], &["list"], &["list", "--json"]] {
        let out = ratewise().args(args).env_clear().output().unwrap();
        assert!(out.status.success(), "{args:?}: {out:?}");
    }
}

#[test]
fn list_json_is_valid_json() {
    let out = ratewise()
        .args(["list", "--json", "--fixture"])
        .arg(fixture("pw-1.2-desktop"))
        .output()
        .unwrap();
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["schema_version"], 1);
}

#[test]
fn closed_stdout_is_not_a_crash() {
    let mut child = ratewise()
        .args(["list", "--fixture"])
        .arg(fixture("pw-1.2-desktop"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdout.take());
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn unfinished_commands_explain_themselves() {
    let out = ratewise().arg("revert").output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("isn't available yet"));
}

#[test]
fn capture_fixture_writes_a_new_dir_and_refuses_a_full_one() {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("capture-test");
    let _ = std::fs::remove_dir_all(&dir);
    let out = ratewise()
        .arg("capture-fixture")
        .arg(&dir)
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    assert!(dir.join("versions.txt").is_file());

    let again = ratewise()
        .arg("capture-fixture")
        .arg(&dir)
        .output()
        .unwrap();
    assert!(!again.status.success());
    assert!(String::from_utf8_lossy(&again.stderr).contains("isn't empty"));
}
