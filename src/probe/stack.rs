//! Detects which sound-related CLI tools are installed (spec §3.4).
//! Server ownership and version detection arrive in milestone 2.

use std::path::{Path, PathBuf};

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
}
