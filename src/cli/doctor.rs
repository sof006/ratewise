//! `ratewise doctor`: a plain-English report of what Ratewise can see. Always exits 0.

use std::fmt::Write;
use std::path::Path;

use crate::core::facts::KernelOutcome;
use crate::core::model::{SessionManager, SoundServer, WirePlumberConfig};
use crate::probe::{Probes, stack};

const W: usize = 16;

pub fn run(probes: &Probes) -> anyhow::Result<()> {
    super::emit(&report(probes))
}

pub fn report(probes: &Probes) -> String {
    let g = probes.gather();
    let st = &g.stack;
    let mut s = String::new();
    let _ = writeln!(s, "Ratewise {} system report", env!("CARGO_PKG_VERSION"));
    if let Some(dir) = probes.fixture_dir() {
        let _ = writeln!(
            s,
            "Reading fixture {}. Sound facts come from the fixture; tools and session are this machine's.",
            dir.display()
        );
    }

    let _ = writeln!(s, "\nSound stack");
    let _ = writeln!(s, "  {:<W$} {}", "Summary", st.summary());
    let server = match st.server {
        SoundServer::PipeWire => "PipeWire",
        SoundServer::PulseAudio => "PulseAudio",
        SoundServer::Jack => {
            "JACK (standalone). Ratewise shows it read-only; change it with your JACK tool."
        }
        SoundServer::Alsa => "none: programs use ALSA devices directly",
        SoundServer::None => "none, and no sound cards",
    };
    let _ = writeln!(s, "  {:<W$} {server}", "Owns audio");
    match &g.pipewire {
        Ok(dump) => {
            let _ = writeln!(
                s,
                "  {:<W$} running, {}, {} audio nodes",
                "PipeWire",
                dump.core_version.as_deref().unwrap_or("unknown version"),
                dump.nodes.len()
            );
        }
        Err(e) => {
            let _ = writeln!(s, "  {:<W$} not available: {e}", "PipeWire");
        }
    }
    if st.server == SoundServer::PipeWire {
        let sm = match &st.session_manager {
            Some(SessionManager::WirePlumber { version, config }) => {
                let format = match config {
                    Some(WirePlumberConfig::Lua04) => "0.4 Lua config",
                    Some(WirePlumberConfig::SpaJson05) => "0.5 SPA-JSON config",
                    None => "config format unknown",
                };
                format!(
                    "WirePlumber {} ({format})",
                    version.as_deref().unwrap_or("(version unknown)")
                )
            }
            Some(SessionManager::PipewireMediaSession) => {
                "pipewire-media-session (deprecated; per-device rules aren't supported)".into()
            }
            None => "none found. Devices may not appear until one is running.".into(),
        };
        let _ = writeln!(s, "  {:<W$} {sm}", "Session manager");
        let pulse = if st.pipewire_pulse {
            "pipewire-pulse is running, so PulseAudio apps go through PipeWire"
        } else {
            "pipewire-pulse isn't running, so PulseAudio-only apps won't have sound"
        };
        let _ = writeln!(s, "  {:<W$} {pulse}", "Pulse apps");
    }
    if st.server == SoundServer::PulseAudio {
        let _ = writeln!(
            s,
            "  {:<W$} {}",
            "PulseAudio",
            st.pulseaudio_version
                .as_deref()
                .unwrap_or("version unknown")
        );
    }
    if st.jack_running {
        let _ = writeln!(s, "  {:<W$} a standalone JACK server is running", "JACK");
    }
    let systemd = match st.systemd_user_session {
        Some(true) => "yes, so audio services can be restarted with systemctl --user",
        Some(false) => "no, so restarting audio services needs a log out and back in",
        None => "unknown",
    };
    let _ = writeln!(s, "  {:<W$} {systemd}", "systemd user");

    let _ = writeln!(s, "\nHardware");
    match &g.procfs {
        Ok(p) if p.cards.is_empty() => {
            let _ = writeln!(s, "  {:<W$} no sound cards", "/proc/asound");
        }
        Ok(p) => {
            let cards: Vec<String> = p
                .cards
                .iter()
                .map(|c| format!("{} ({})", c.id, c.driver))
                .collect();
            let _ = writeln!(
                s,
                "  {:<W$} {} card(s): {}",
                "/proc/asound",
                cards.len(),
                cards.join(", ")
            );
        }
        Err(e) => {
            let _ = writeln!(s, "  {:<W$} not available: {e}", "/proc/asound");
        }
    }
    let kernel = match &g.kernel {
        Ok(probes) if probes.is_empty() => "no PCM device nodes".to_string(),
        Ok(probes) => {
            let count =
                |f: fn(&KernelOutcome) -> bool| probes.iter().filter(|p| f(&p.result)).count();
            let ok = count(|r| matches!(r, KernelOutcome::Ok(_)));
            let busy = count(|r| matches!(r, KernelOutcome::Busy));
            let denied = count(|r| matches!(r, KernelOutcome::PermissionDenied));
            let failed = count(|r| matches!(r, KernelOutcome::Failed { .. }));
            let mut parts = vec![format!("{} PCM devices: {ok} probed", probes.len())];
            if busy > 0 {
                parts.push(format!("{busy} busy (in use, which is normal)"));
            }
            if denied > 0 {
                parts.push(format!(
                    "{denied} no permission (are you in the `audio` group?)"
                ));
            }
            if failed > 0 {
                parts.push(format!("{failed} failed"));
            }
            parts.join(", ")
        }
        Err(e) => format!("not available: {e}"),
    };
    let _ = writeln!(s, "  {:<W$} {kernel}", "/dev/snd");

    let _ = writeln!(s, "\nTools");
    for t in stack::find_tools() {
        let found = t
            .path
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "not found".into());
        let _ = writeln!(s, "  {:<W$} {found}", t.name);
    }

    let _ = writeln!(s, "\nSession");
    let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let runtime = match var("XDG_RUNTIME_DIR") {
        Some(d) if Path::new(&d).is_dir() => d,
        Some(d) => format!("{d} (set, but the directory doesn't exist)"),
        None => "not set. Sound servers usually can't be reached without it.".into(),
    };
    let _ = writeln!(s, "  {:<W$} {runtime}", "XDG_RUNTIME_DIR");
    let display = match (var("WAYLAND_DISPLAY"), var("DISPLAY")) {
        (Some(w), Some(x)) => format!("Wayland ({w}), X11 ({x})"),
        (Some(w), None) => format!("Wayland ({w})"),
        (None, Some(x)) => format!("X11 ({x})"),
        (None, None) => "none. The GUI won't open; the CLI works.".into(),
    };
    let _ = writeln!(s, "  {:<W$} {display}", "Display");
    let renderer = if !cfg!(feature = "gui") {
        "none: this build has no window"
    } else if std::env::var("RATEWISE_RENDERER").as_deref() == Ok("software") {
        "software (forced by RATEWISE_RENDERER=software)"
    } else {
        "OpenGL (FemtoVG), switching to software automatically if OpenGL fails. \
         RATEWISE_RENDERER=software forces software."
    };
    let _ = writeln!(s, "  {:<W$} {renderer}", "GUI renderer");
    s
}
