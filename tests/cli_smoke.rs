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

fn sandbox(name: &str) -> std::path::PathBuf {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// Runs ratewise against a fixture, in a sandbox. Returns (success, stdout, stderr).
fn in_sandbox(dir: &std::path::Path, fx: &str, args: &[&str]) -> (bool, String, String) {
    let out = ratewise()
        .args(args)
        .arg("--fixture")
        .arg(fixture(fx))
        .arg("--sandbox")
        .arg(dir)
        .env_remove("RATEWISE_FIXTURE")
        .env_remove("RATEWISE_SANDBOX")
        .output()
        .unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn set_plan_revert_reset_in_a_sandbox() {
    let dir = sandbox("cli-sandbox");
    let home = dir.join("home");
    let dropin = home.join(".config/pipewire/pipewire.conf.d/60-ratewise.conf");

    let (ok, out, err) = in_sandbox(&dir, "pw-1.2-desktop", &["plan", "rate", "96k", "--save"]);
    assert!(ok, "{err}");
    assert!(out.contains("+    default.clock.rate = 96000"), "{out}");
    assert!(!dropin.exists(), "plan changes nothing");

    let (ok, out, err) = in_sandbox(
        &dir,
        "pw-1.2-desktop",
        &["set", "rate", "96k", "--save", "--restart"],
    );
    assert!(ok, "{err}");
    assert!(
        out.contains("Wrote ~/.config/pipewire/pipewire.conf.d/60-ratewise.conf"),
        "{out}"
    );
    assert!(out.contains("Restart (sandbox, not run)"), "{out}");
    assert!(
        std::fs::read_to_string(&dropin)
            .unwrap()
            .contains("default.clock.rate = 96000")
    );

    let (ok, out, _) = in_sandbox(&dir, "pw-1.2-desktop", &["set", "quantum", "256"]);
    assert!(ok);
    assert!(
        out.contains("Would run (sandbox): pw-metadata -n settings 0 clock.quantum 256"),
        "{out}"
    );

    let (ok, out, err) = in_sandbox(&dir, "pw-1.2-desktop", &["show", "D50s", "--json"]);
    assert!(ok, "{err}");
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
        v["device"]["id"],
        "alsa_output.usb-Topping_D50s-00.analog-stereo"
    );

    // The quantum change was live, so the newest backup is its undo; revert twice.
    let (ok, out, err) = in_sandbox(&dir, "pw-1.2-desktop", &["revert"]);
    assert!(ok, "{err}");
    assert!(out.contains("clock.quantum 1024"), "{out}");
    let (ok, out, err) = in_sandbox(&dir, "pw-1.2-desktop", &["revert", "--dry-run"]);
    assert!(ok, "{err}");
    assert!(
        out.contains("Remove ~/.config/pipewire/pipewire.conf.d/60-ratewise.conf"),
        "{out}"
    );

    let (ok, _, err) = in_sandbox(&dir, "pw-1.2-desktop", &["reset"]);
    assert!(ok, "{err}");
    assert!(!dropin.exists());
    let (ok, out, _) = in_sandbox(&dir, "pw-1.2-desktop", &["reset"]);
    assert!(ok);
    assert!(out.contains("nothing to reset"), "{out}");
}

#[test]
fn refusals_are_plain_english_and_exit_nonzero() {
    let dir = sandbox("cli-refusals");
    let (ok, _, err) = in_sandbox(
        &dir,
        "pw-1.2-desktop",
        &["set", "rate", "96000", "--device", "D50s"],
    );
    assert!(!ok);
    assert!(err.contains("Add --save"), "{err}");
    let (ok, _, err) = in_sandbox(
        &dir,
        "pw-1.2-desktop",
        &["set", "rate", "96000", "--device", "nope"],
    );
    assert!(!ok);
    assert!(err.contains("ratewise list"), "{err}");
    let (ok, _, err) = in_sandbox(&dir, "pw-1.2-desktop", &["set", "rate", "fast"]);
    assert!(!ok);
    assert!(!err.contains("panicked"), "{err}");
}

#[test]
fn a_fixture_without_a_sandbox_never_writes() {
    let out = ratewise()
        .args(["set", "rate", "96000", "--save", "--fixture"])
        .arg(fixture("pw-1.2-desktop"))
        .env_remove("RATEWISE_SANDBOX")
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("--sandbox"));
}

#[test]
fn revert_with_no_backups_explains() {
    let dir = sandbox("cli-no-backups");
    let (ok, out, err) = in_sandbox(&dir, "alsa-only", &["revert"]);
    assert!(ok, "{err}");
    assert!(out.contains("nothing to revert"), "{out}");
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
