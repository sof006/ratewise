//! The window: it opens and renders under Xvfb (spec §6.4's self-test), and without a
//! display it explains itself instead of crashing.
#![cfg(feature = "gui")]

mod common;

use std::process::Command;

use common::fixture;

fn ratewise() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_ratewise"));
    c.env_remove("RATEWISE_PAGE").env_remove("RATEWISE_TAB");
    c
}

#[test]
fn without_a_display_the_window_explains_and_exits_non_zero() {
    let out = ratewise()
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .args(["--fixture"])
        .arg(fixture("no-sound"))
        .output()
        .unwrap();
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("No display") && err.contains("ratewise list"),
        "{err}"
    );
}

#[test]
fn self_test_renders_under_xvfb() {
    if Command::new("xvfb-run").arg("--help").output().is_err() {
        eprintln!("skipped: xvfb-run isn't installed");
        return;
    }
    for name in ["pw-1.2-desktop", "no-sound"] {
        let out = Command::new("xvfb-run")
            .args([
                "-a",
                env!("CARGO_BIN_EXE_ratewise"),
                "--self-test",
                "--fixture",
            ])
            .arg(fixture(name))
            .env("RATEWISE_RENDERER", "software")
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(out.status.success(), "{name}: {out:?}");
        assert!(stdout.contains("Self-test passed"), "{name}: {stdout}");
    }
}
