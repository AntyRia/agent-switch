use thiserror::Error;

/// Unified error type for the core crate.
#[derive(Debug, Error)]
pub enum Error {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("TOML parse error: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("profile not found: {0}")]
    ProfileNotFound(String),
    #[error("profile validation failed: {0}")]
    Validation(String),
    #[error("Codex CLI not found in PATH")]
    CodexNotFound,
    #[error("Claude CLI not found in PATH")]
    ClaudeNotFound,
    #[error("no API key configured for profile {0}")]
    NoApiKey(String),
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;
