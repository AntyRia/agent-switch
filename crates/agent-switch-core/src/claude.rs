use std::path::PathBuf;

use crate::error::{Error, Result};

/// Locate the claude executable on PATH (Windows PATHEXT covers .cmd/.exe).
pub fn find_claude() -> Result<PathBuf> {
    which::which("claude").map_err(|_| Error::ClaudeNotFound)
}

/// `claude --version`, first line of output (None when claude is missing).
pub fn claude_version() -> Option<String> {
    let path = find_claude().ok()?;
    let output = if crate::codex::needs_cmd_wrap(path.as_path()) {
        std::process::Command::new("cmd.exe")
            .arg("/C")
            .arg(&path)
            .arg("--version")
            .output()
            .ok()?
    } else {
        std::process::Command::new(&path).arg("--version").output().ok()?
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .map(|l| l.trim().to_string())
        .find(|l| !l.is_empty())
}
