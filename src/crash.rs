//! The panic handler (spec §6.3): write a crash log to the state folder and, when the
//! window is open, tell the user with a zenity or kdialog popup.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static GUI: AtomicBool = AtomicBool::new(false);

/// Whether the window is running, so a crash should show a popup too.
pub fn set_gui(on: bool) {
    GUI.store(on, Ordering::Relaxed);
}

/// Installs the handler. The default handler still prints the message to stderr.
pub fn install() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        default(info);
        let log = write_log(&report(info));
        if GUI.load(Ordering::Relaxed) {
            popup(log.as_deref());
        }
    }));
}

fn report(info: &std::panic::PanicHookInfo<'_>) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "Ratewise {} crashed.", env!("CARGO_PKG_VERSION"));
    let _ = writeln!(s, "{info}");
    let thread = std::thread::current();
    let _ = writeln!(s, "Thread: {}", thread.name().unwrap_or("unnamed"));
    let _ = writeln!(
        s,
        "\nBacktrace:\n{}",
        std::backtrace::Backtrace::force_capture()
    );
    s
}

fn state_dir() -> Option<PathBuf> {
    let var = |k: &str| {
        std::env::var_os(k)
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
    };
    var("XDG_STATE_HOME")
        .or_else(|| var("HOME").map(|h| h.join(".local/state")))
        .map(|d| d.join("ratewise"))
}

/// Writes the log, returning its path. Never panics.
fn write_log(text: &str) -> Option<PathBuf> {
    let dir = state_dir()?;
    std::fs::create_dir_all(&dir).ok()?;
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let path = dir.join(format!("crash-{secs}.log"));
    std::fs::write(&path, text).ok()?;
    Some(path)
}

fn popup(log: Option<&Path>) {
    let text = match log {
        Some(p) => format!(
            "Ratewise ran into a problem and has to close.\n\nA report was saved to {}. \
             Please include it if you report this.",
            p.display()
        ),
        None => "Ratewise ran into a problem and has to close.".to_string(),
    };
    let tries: [(&str, Vec<&str>); 2] = [
        (
            "zenity",
            vec!["--error", "--title=Ratewise", "--text", &text],
        ),
        ("kdialog", vec!["--title", "Ratewise", "--error", &text]),
    ];
    for (program, args) in tries {
        let status = std::process::Command::new(program)
            .args(&args)
            .stdin(std::process::Stdio::null())
            .status();
        if status.is_ok() {
            return;
        }
    }
}
