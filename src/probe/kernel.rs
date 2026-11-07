//! Kernel-level device access (spec §3.2). Milestone 1 only lists `/dev/snd` PCM nodes;
//! the `HW_REFINE` ioctl probe (no libasound) arrives in milestone 2.

use std::io;
use std::path::Path;

/// Names of PCM device nodes (`pcmC0D0p`, ...) in `dir`, sorted.
pub fn pcm_device_nodes(dir: &Path) -> io::Result<Vec<String>> {
    let mut names: Vec<String> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("pcmC"))
        .collect();
    names.sort();
    Ok(names)
}
