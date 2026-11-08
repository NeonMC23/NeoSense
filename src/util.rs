use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct CommandOutput {
    pub success: bool,
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl CommandOutput {
    pub fn combined_error(&self) -> String {
        let err = self.stderr.trim();
        if !err.is_empty() {
            return err.to_string();
        }
        let out = self.stdout.trim();
        if !out.is_empty() {
            return out.to_string();
        }
        format!(
            "exit status {}",
            self.code.map_or("unknown".into(), |c| c.to_string())
        )
    }
}

/// Run a program without a shell, with a bounded timeout.
///
/// This is deliberately small: no piping through `sh -c`, no string
/// interpolation, no hidden environment. External command failures are
/// returned as data so callers can surface them.
pub fn run_command(
    program: &str,
    args: &[&str],
    timeout: Duration,
) -> Result<CommandOutput, String> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{program}: {e}"))?;

    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait_with_output();
                return Err(format!(
                    "{program} a dépassé le délai de {} ms",
                    timeout.as_millis()
                ));
            }
            Err(e) => return Err(format!("{program}: {e}")),
        }
    }

    let output = child
        .wait_with_output()
        .map_err(|e| format!("{program}: {e}"))?;
    Ok(CommandOutput {
        success: output.status.success(),
        code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}
