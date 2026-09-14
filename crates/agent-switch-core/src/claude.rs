use std::path::PathBuf;

use crate::error::{Error, Result};

/// Locate the claude executable (process PATH, then well-known install
/// locations, then the login shell's PATH — see `resolve`).
pub fn find_claude() -> Result<PathBuf> {
    crate::resolve::resolve_binary("claude").ok_or(Error::ClaudeNotFound)
}

/// `claude --version`, first line of output (None when claude is missing).
pub fn claude_version() -> Option<String> {
    let path = find_claude().ok()?;
    let mut cmd = if crate::codex::needs_cmd_wrap(path.as_path()) {
        let mut c = std::process::Command::new("cmd.exe");
        c.arg("/C").arg(&path).arg("--version");
        c
    } else {
        let mut c = std::process::Command::new(&path);
        c.arg("--version");
        c
    };
    crate::resolve::prepend_bin_dir_to_path(&mut cmd, &path);
    let output = cmd.output().ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .map(|l| l.trim().to_string())
        .find(|l| !l.is_empty())
}
