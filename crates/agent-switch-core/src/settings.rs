//! Global application settings, persisted in `<config_root>/settings.toml`
//! (one file shared by the CLI and the GUI; the file was a reserved
//! placeholder before this module read from it).
//!
//! Everything here applies to EVERY launch (new conversations and session
//! resumes alike): the dangerous-mode flags, the proxy environment and the
//! terminal used to open the CLI.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::profile_store::config_root;

/// The user's global launch preferences. All fields are optional in the
/// file: a missing/empty file yields the defaults (no dangerous mode, no
/// proxy, auto-detected terminal).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Launch the CLI in bypass mode:
    /// - codex: `--dangerously-bypass-approvals-and-sandbox`
    /// - claude: `--dangerously-skip-permissions`
    /// The user explicitly accepts running with no approval prompts and no
    /// sandbox/permission checks.
    pub dangerous_mode: bool,
    /// Proxy host for launched CLIs (e.g. `127.0.0.1`). Empty = disabled
    /// (direct connection).
    pub proxy_host: String,
    /// Proxy port; used only when `proxy_host` is non-empty.
    pub proxy_port: u16,
    /// Absolute path of the terminal to open the CLI in. Empty = auto-detect
    /// the first available terminal on this machine.
    pub terminal: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            dangerous_mode: false,
            proxy_host: String::new(),
            proxy_port: 0,
            terminal: String::new(),
        }
    }
}

impl Settings {
    /// `<config_root>/settings.toml`.
    pub fn path() -> PathBuf {
        config_root().join("settings.toml")
    }

    /// Load the settings; a missing file, an unreadable file or a corrupt
    /// file all yield the defaults (settings must never block a launch).
    pub fn load() -> Self {
        fs::read_to_string(Self::path())
            .ok()
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Persist atomically (temp + rename).
    pub fn save(&self) -> Result<()> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let text = toml::to_string_pretty(self)
            .map_err(|e| crate::error::Error::Other(format!("failed to serialize settings: {e}")))?;
        let tmp = path.with_extension("toml.tmp");
        fs::write(&tmp, text)?;
        fs::rename(&tmp, &path)?;
        Ok(())
    }

    /// `http://<host>:<port>` when a proxy is configured, else None.
    pub fn proxy_url(&self) -> Option<String> {
        let host = self.proxy_host.trim();
        if host.is_empty() || self.proxy_port == 0 {
            return None;
        }
        Some(format!("http://{host}:{}", self.proxy_port))
    }

    /// The CLI flag for bypass mode, per engine (None when disabled).
    pub fn dangerous_flag(&self, engine: crate::engine::Engine) -> Option<&'static str> {
        if !self.dangerous_mode {
            return None;
        }
        match engine {
            crate::engine::Engine::Codex => Some("--dangerously-bypass-approvals-and-sandbox"),
            crate::engine::Engine::Claude => Some("--dangerously-skip-permissions"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_inert() {
        let s = Settings::default();
        assert!(!s.dangerous_mode);
        assert!(s.proxy_url().is_none());
        assert!(s.dangerous_flag(crate::engine::Engine::Codex).is_none());
    }

    #[test]
    fn proxy_url_needs_host_and_port() {
        let mut s = Settings::default();
        s.proxy_host = "127.0.0.1".into();
        s.proxy_port = 7897;
        assert_eq!(s.proxy_url(), Some("http://127.0.0.1:7897".into()));
        s.proxy_port = 0;
        assert!(s.proxy_url().is_none());
        s.proxy_port = 7897;
        s.proxy_host = "   ".into();
        assert!(s.proxy_url().is_none());
    }

    #[test]
    fn dangerous_flag_is_engine_specific() {
        let mut s = Settings::default();
        s.dangerous_mode = true;
        assert_eq!(
            s.dangerous_flag(crate::engine::Engine::Codex),
            Some("--dangerously-bypass-approvals-and-sandbox")
        );
        assert_eq!(
            s.dangerous_flag(crate::engine::Engine::Claude),
            Some("--dangerously-skip-permissions")
        );
    }

    #[test]
    fn settings_roundtrip_and_corrupt_file_falls_back() {
        let dir = std::env::temp_dir().join(format!(
            "as-settings-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::env::set_var("AGENT_SWITCH_HOME", &dir);
        // No file yet: defaults.
        assert_eq!(Settings::load(), Settings::default());
        let mut s = Settings::default();
        s.dangerous_mode = true;
        s.proxy_host = "10.0.0.1".into();
        s.proxy_port = 1080;
        s.terminal = "C:\\Program Files\\wt\\wt.exe".into();
        s.save().unwrap();
        assert_eq!(Settings::load(), s);
        // Corrupt file: defaults, not an error.
        fs::write(Settings::path(), "not [toml").unwrap();
        assert_eq!(Settings::load(), Settings::default());
        std::env::remove_var("AGENT_SWITCH_HOME");
        let _ = fs::remove_dir_all(&dir);
    }
}
