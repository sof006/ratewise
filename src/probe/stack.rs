//! Gathers the facts used to detect the sound stack (spec §3.4): installed tools,
//! this user's sound-server processes, `pactl info`, `wireplumber --version`, and
//! whether a systemd user session exists. In fixture mode, the same facts come
//! from `versions.txt` and `processes.txt`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::core::facts::EnvFacts;
use crate::core::stack::tool_version;
use crate::probe::command;

/// Tools Ratewise uses or reports on.
pub const TOOLS: &[&str] = &[
    "pw-dump",
    "pw-metadata",
    "pw-cli",
    "wpctl",
    "pipewire",
    "wireplumber",
    "pactl",
    "pulseaudio",
    "systemctl",
    "jack_lsp",
];

/// Process names worth knowing about. Nothing else is recorded.
pub const SOUND_PROCESSES: &[&str] = &[
    "pipewire",
    "pipewire-pulse",
    "wireplumber",
    "pipewire-media-session",
    "pulseaudio",
    "jackd",
    "jackdbus",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolStatus {
    pub name: &'static str,
    pub path: Option<PathBuf>,
}

pub fn find_tools() -> Vec<ToolStatus> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let dirs: Vec<PathBuf> = std::env::split_paths(&path).collect();
    TOOLS
        .iter()
        .map(|&name| ToolStatus {
            name,
            path: find_in(&dirs, name),
        })
        .collect()
}

fn find_in(dirs: &[PathBuf], name: &str) -> Option<PathBuf> {
    dirs.iter().map(|d| d.join(name)).find(|p| is_executable(p))
}

#[cfg(unix)]
fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(p: &Path) -> bool {
    p.is_file()
}

/// Where the session facts come from.
pub trait EnvSource: Send + Sync {
    fn env_facts(&self) -> EnvFacts;
}

/// The live session.
pub struct LiveEnv;

impl EnvSource for LiveEnv {
    fn env_facts(&self) -> EnvFacts {
        let processes = sound_processes(Path::new("/proc"));
        let (pulse_server_name, pulse_server_version) =
            if pulse_server_reachable(processes.as_deref()) {
                pactl_info()
            } else {
                (None, None)
            };
        let wireplumber_version =
            command::run("wireplumber", &["--version"], Duration::from_secs(3))
                .ok()
                .filter(|o| o.status.success())
                .and_then(|o| tool_version(&o.stdout));
        EnvFacts {
            processes,
            pulse_server_name,
            pulse_server_version,
            wireplumber_version,
            systemd_user_session: systemd_user_session(),
        }
    }
}

/// Names from `SOUND_PROCESSES` that this user is running, by reading `/proc/<pid>/comm`.
pub fn sound_processes(proc: &Path) -> Option<Vec<String>> {
    use std::os::unix::fs::MetadataExt;
    let me = std::fs::metadata(proc.join("self")).ok()?.uid();
    let mut found: Vec<String> = std::fs::read_dir(proc)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .bytes()
                .all(|b| b.is_ascii_digit())
        })
        .filter(|e| e.metadata().is_ok_and(|m| m.uid() == me))
        .filter_map(|e| std::fs::read_to_string(e.path().join("comm")).ok())
        .map(|c| c.trim().to_string())
        .filter(|c| {
            SOUND_PROCESSES
                .iter()
                .any(|p| p == c || (c.len() == 15 && p.starts_with(c.as_str())))
        })
        .collect();
    found.sort();
    found.dedup();
    Some(found)
}

/// Only ask `pactl` when a server is known to be up. If nothing is listening,
/// libpulse may autospawn PulseAudio, and a read-only probe must not start a sound server.
fn pulse_server_reachable(processes: Option<&[String]>) -> bool {
    if std::env::var_os("PULSE_SERVER").is_some() {
        return true;
    }
    let server_running =
        processes.is_some_and(|p| p.iter().any(|n| n == "pulseaudio" || n == "pipewire-pulse"));
    let socket = std::env::var_os("XDG_RUNTIME_DIR")
        .map(|d| Path::new(&d).join("pulse/native"))
        .is_some_and(|s| s.exists());
    server_running && socket
}

fn pactl_info() -> (Option<String>, Option<String>) {
    let Ok(out) = command::run("pactl", &["info"], Duration::from_secs(3)) else {
        return (None, None);
    };
    if !out.status.success() {
        return (None, None);
    }
    let field = |key: &str| {
        out.stdout
            .lines()
            .find_map(|l| l.strip_prefix(key))
            .map(|v| v.trim().to_string())
    };
    (field("Server Name:"), field("Server Version:"))
}

/// `Some(true)` if a systemd user manager is running for this session.
fn systemd_user_session() -> Option<bool> {
    if !Path::new("/run/systemd/system").exists() {
        // Not booted with systemd (OpenRC, runit, containers).
        return Some(false);
    }
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")?;
    Some(Path::new(&runtime).join("systemd/private").exists())
}

/// Facts recorded in a fixture's `versions.txt` and `processes.txt`.
pub struct FixtureEnv {
    pub dir: PathBuf,
}

impl EnvSource for FixtureEnv {
    fn env_facts(&self) -> EnvFacts {
        let versions = std::fs::read_to_string(self.dir.join("versions.txt")).unwrap_or_default();
        let field = |key: &str| {
            versions.lines().find_map(|l| {
                let (k, v) = l.split_once(':')?;
                (k.trim() == key).then(|| v.trim().to_string())
            })
        };
        let processes = std::fs::read_to_string(self.dir.join("processes.txt"))
            .ok()
            .map(|t| {
                t.lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty() && !l.starts_with('#'))
                    .map(String::from)
                    .collect()
            });
        EnvFacts {
            processes,
            pulse_server_name: field("pulse-server"),
            pulse_server_version: field("pulse-server-version"),
            wireplumber_version: field("wireplumber").and_then(|v| tool_version(&v)),
            systemd_user_session: field("systemd-user-session").map(|v| v == "yes"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_sh_but_not_nonsense() {
        let dirs = vec![PathBuf::from("/bin"), PathBuf::from("/usr/bin")];
        assert!(find_in(&dirs, "sh").is_some());
        assert!(find_in(&dirs, "ratewise-no-such-tool").is_none());
        assert!(find_in(&[], "sh").is_none());
    }

    #[test]
    fn process_scan_runs_and_only_reports_sound_processes() {
        let found = sound_processes(Path::new("/proc")).expect("/proc is readable");
        assert!(
            found
                .iter()
                .all(|n| SOUND_PROCESSES.iter().any(|p| p.starts_with(n.as_str())))
        );
        assert_eq!(sound_processes(Path::new("/nonexistent")), None);
    }
}
