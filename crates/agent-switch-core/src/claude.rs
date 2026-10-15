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
    crate::cli_health::version(crate::engine::Engine::Claude, &path).ok()
}
