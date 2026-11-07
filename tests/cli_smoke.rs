use std::process::Command;

fn ratewise() -> Command {
    Command::new(env!("CARGO_BIN_EXE_ratewise"))
}

#[test]
fn help_exits_zero() {
    let out = ratewise().arg("--help").output().unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("doctor"));
}

#[test]
fn version_exits_zero() {
    let out = ratewise().arg("--version").output().unwrap();
    assert!(out.status.success());
}
