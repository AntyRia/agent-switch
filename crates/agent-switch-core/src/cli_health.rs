//! CLI presence is not installation health: an npm shim can point at a missing
//! binary or at the shell-text placeholder left by an interrupted postinstall.

use crate::{
    engine::Engine,
    error::{Error, Result},
};
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub(crate) fn command(binary: &Path) -> Command {
    let mut command = if crate::codex::needs_cmd_wrap(binary) {
        let mut cmd = Command::new("cmd.exe");
        cmd.args(["/D", "/C"]).arg(binary);
        cmd
    } else {
        Command::new(binary)
    };
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // health probes never open windows
    }
    crate::resolve::prepend_bin_dir_to_path(&mut command, binary);
    command
}

/// Drain bounded output concurrently so a broken command cannot fill a pipe
/// and freeze the GUI. Both process completion and output have a deadline.
pub(crate) fn output(mut command: Command, timeout: Duration) -> Result<String> {
    let child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    collect_output(child, timeout)
}

fn collect_output(mut child: std::process::Child, timeout: Duration) -> Result<String> {
    let deadline = Instant::now() + timeout;
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = stdout.take(1024 * 1024).read_to_end(&mut bytes);
        let _ = tx.send(result.map(|_| String::from_utf8_lossy(&bytes).into_owned()));
    });
    loop {
        if let Some(status) = child.try_wait()? {
            if !status.success() {
                return Err(Error::Other(format!("CLI exited with {status}")));
            }
            return rx
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .map_err(|_| Error::Other("CLI output probe timed out".into()))?
                .map_err(Error::from);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::Other("CLI health probe timed out".into()));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

pub fn version(engine: Engine, binary: &Path) -> Result<String> {
    let mut cmd = command(binary);
    cmd.arg("--version");
    let text = output(cmd, Duration::from_secs(8))?;
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    let expected = match engine {
        Engine::Claude => "Claude Code",
        Engine::Codex => "codex",
    };
    if !line.contains(expected) || !line.chars().any(|c| c.is_ascii_digit()) {
        return Err(Error::Other(
            "CLI returned an invalid version response".into(),
        ));
    }
    Ok(line.to_string())
}

pub fn ensure_ready(engine: Engine) -> Result<String> {
    let binary = crate::launcher::find_engine_binary(engine)?;
    version(engine, &binary).map_err(|error| Error::Other(format!(
        "{} CLI at '{}' failed its startup check ({error}). Repair the CLI installation, check free disk space, then re-detect it. / CLI 安装损坏或无法启动，请检查磁盘空间、修复安装后重新检测。",
        engine.binary(), binary.display()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn shell(script: &str) -> Command {
        #[cfg(windows)]
        {
            let mut cmd = Command::new("cmd.exe");
            cmd.args(["/D", "/C", script]);
            cmd
        }
        #[cfg(not(windows))]
        {
            let mut cmd = Command::new("sh");
            cmd.args(["-c", script]);
            cmd
        }
    }
    #[test]
    fn broken_install_placeholder_and_invalid_version_fail_health_check() {
        let root = std::env::temp_dir().join(format!("as-health-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let placeholder = root.join("claude.exe");
        std::fs::write(&placeholder, "echo native binary not installed").unwrap();
        assert!(version(Engine::Claude, &placeholder).is_err());
        #[cfg(windows)]
        let (script, contents) = (
            root.join("claude.cmd"),
            "@echo off\necho native binary not installed\nexit /b 0\n",
        );
        #[cfg(not(windows))]
        let (script, contents) = (
            root.join("claude"),
            "#!/bin/sh\necho native binary not installed\nexit 0\n",
        );
        std::fs::write(&script, contents).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        assert!(version(Engine::Claude, &script).is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn successful_output_on_a_failed_command_is_not_healthy() {
        #[cfg(windows)]
        let cmd = shell("echo 2.1.300 Claude Code & exit 1");
        #[cfg(not(windows))]
        let cmd = shell("echo '2.1.300 Claude Code'; exit 1");
        assert!(output(cmd, Duration::from_secs(2)).is_err());
    }
    #[test]
    fn hung_cli_is_bounded() {
        #[cfg(windows)]
        let mut cmd = shell("ping -n 5 127.0.0.1 >nul");
        #[cfg(not(windows))]
        let mut cmd = shell("sleep 5");
        // Process creation can be delayed by Windows antivirus or parallel
        // builds; measure the running probe's deadline independently.
        let child = cmd
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let start = Instant::now();
        assert!(collect_output(child, Duration::from_millis(100)).is_err());
        assert!(start.elapsed() < Duration::from_secs(2));
    }
}
