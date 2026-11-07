//! `ratewise doctor`: a plain-English report of what Ratewise can see. Always exits 0.

use std::fmt::Write;
use std::path::Path;

use crate::probe::{Probes, kernel, stack};

pub fn run(probes: &Probes) -> anyhow::Result<()> {
    super::emit(&report(probes))
}

pub fn report(probes: &Probes) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "Ratewise {} system report", env!("CARGO_PKG_VERSION"));
    if let Some(dir) = probes.fixture_dir() {
        let _ = writeln!(
            s,
            "Reading fixture {}. Sound sources come from the fixture; tools and session are this machine's.",
            dir.display()
        );
    }

    let _ = writeln!(s, "\nSound");
    match probes.pipewire() {
        Ok(dump) => {
            let version = dump.core_version.as_deref().unwrap_or("unknown version");
            let _ = writeln!(
                s,
                "  {:<16} running, {version}, {} audio nodes",
                "PipeWire",
                dump.nodes.len()
            );
        }
        Err(e) => {
            let _ = writeln!(s, "  {:<16} not available: {e}", "PipeWire");
        }
    }
    match probes.procfs() {
        Ok(p) if p.cards.is_empty() => {
            let _ = writeln!(s, "  {:<16} no sound cards", "/proc/asound");
        }
        Ok(p) => {
            let cards: Vec<String> = p
                .cards
                .iter()
                .map(|c| format!("{} ({})", c.id, c.driver))
                .collect();
            let _ = writeln!(
                s,
                "  {:<16} {} card(s): {}",
                "/proc/asound",
                cards.len(),
                cards.join(", ")
            );
        }
        Err(e) => {
            let _ = writeln!(s, "  {:<16} not available: {e}", "/proc/asound");
        }
    }
    if probes.fixture_dir().is_none() {
        let devsnd = match kernel::pcm_device_nodes(Path::new("/dev/snd")) {
            Ok(n) if n.is_empty() => "present, but no PCM device nodes".to_string(),
            Ok(n) => format!("{} PCM device nodes", n.len()),
            Err(e) => {
                format!("not available ({e}). Without it Ratewise can't probe hardware directly.")
            }
        };
        let _ = writeln!(s, "  {:<16} {devsnd}", "/dev/snd");
    }

    let _ = writeln!(s, "\nTools");
    for t in stack::find_tools() {
        let found = t
            .path
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "not found".into());
        let _ = writeln!(s, "  {:<16} {found}", t.name);
    }

    let _ = writeln!(s, "\nSession");
    let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let runtime = match var("XDG_RUNTIME_DIR") {
        Some(d) if Path::new(&d).is_dir() => d,
        Some(d) => format!("{d} (set, but the directory doesn't exist)"),
        None => "not set. Sound servers usually can't be reached without it.".into(),
    };
    let _ = writeln!(s, "  {:<16} {runtime}", "XDG_RUNTIME_DIR");
    let display = match (var("WAYLAND_DISPLAY"), var("DISPLAY")) {
        (Some(w), Some(x)) => format!("Wayland ({w}), X11 ({x})"),
        (Some(w), None) => format!("Wayland ({w})"),
        (None, Some(x)) => format!("X11 ({x})"),
        (None, None) => "none. The GUI won't open; the CLI works.".into(),
    };
    let _ = writeln!(s, "  {:<16} {display}", "Display");
    let _ = writeln!(s, "  {:<16} not built yet", "GUI renderer");
    s
}
