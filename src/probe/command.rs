//! Runs external tools with a timeout. A missing tool is an ordinary error, never a panic.

use std::io::{self, Read};
use std::process::{Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug)]
pub struct CommandOutput {
    pub status: ExitStatus,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error("{0} isn't installed")]
    NotFound(String),
    #[error("{program} didn't finish within {} seconds", timeout.as_secs())]
    Timeout { program: String, timeout: Duration },
    #[error("couldn't run {program}: {source}")]
    Io {
        program: String,
        #[source]
        source: io::Error,
    },
}

/// Runs `program args...` with `LC_ALL=C`, capturing output, killing it after `timeout`.
pub fn run(program: &str, args: &[&str], timeout: Duration) -> Result<CommandOutput, RunError> {
    let io_err = |source| RunError::Io {
        program: program.to_string(),
        source,
    };
    let mut child = Command::new(program)
        .args(args)
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| match e.kind() {
            io::ErrorKind::NotFound => RunError::NotFound(program.to_string()),
            _ => io_err(e),
        })?;

    // Drain both pipes on threads so a chatty child can't deadlock on a full pipe.
    let drain = |pipe: Option<Box<dyn Read + Send>>| {
        thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut p) = pipe {
                let _ = p.read_to_end(&mut buf);
            }
            buf
        })
    };
    let out = drain(
        child
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let err = drain(
        child
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait().map_err(io_err)? {
            Some(status) => break status,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(RunError::Timeout {
                    program: program.to_string(),
                    timeout,
                });
            }
            None => thread::sleep(Duration::from_millis(10)),
        }
    };

    let text = |h: thread::JoinHandle<Vec<u8>>| {
        String::from_utf8_lossy(&h.join().unwrap_or_default()).into_owned()
    };
    Ok(CommandOutput {
        status,
        stdout: text(out),
        stderr: text(err),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_tool_is_an_error() {
        let r = run("ratewise-no-such-tool", &[], Duration::from_secs(1));
        assert!(matches!(r, Err(RunError::NotFound(_))));
    }

    #[test]
    fn captures_output() {
        let r = run(
            "sh",
            &["-c", "echo hi; echo err >&2"],
            Duration::from_secs(5),
        )
        .unwrap();
        assert!(r.status.success());
        assert_eq!(r.stdout, "hi\n");
        assert_eq!(r.stderr, "err\n");
    }

    #[test]
    fn times_out() {
        let r = run("sleep", &["5"], Duration::from_millis(100));
        assert!(matches!(r, Err(RunError::Timeout { .. })));
    }
}
