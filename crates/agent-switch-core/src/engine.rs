use crate::profile::Profile;

/// Which CLI — and therefore which wire protocol — a profile targets:
/// Codex speaks the OpenAI protocol, Claude speaks the Anthropic protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    Codex,
    Claude,
}

impl Engine {
    /// Parse the profile's `cli` field. Unknown/empty values fall back to
    /// Codex so legacy profiles (written before the field existed) keep
    /// working; validation rejects unknown values on save.
    pub fn parse(cli: &str) -> Engine {
        if cli.trim().eq_ignore_ascii_case("claude") {
            Engine::Claude
        } else {
            Engine::Codex
        }
    }

    /// Canonical value for the `cli` profile field.
    pub fn value(&self) -> &'static str {
        match self {
            Engine::Codex => "codex",
            Engine::Claude => "claude",
        }
    }

    /// Env var that points at the per-launch isolated config home.
    pub fn home_env(&self) -> &'static str {
        match self {
            Engine::Codex => "CODEX_HOME",
            Engine::Claude => "CLAUDE_CONFIG_DIR",
        }
    }

    /// Directory name of the isolated home under the runtime dir.
    pub fn home_dir_name(&self) -> &'static str {
        match self {
            Engine::Codex => ".codex",
            Engine::Claude => ".claude",
        }
    }

    /// Binary name looked up on PATH.
    pub fn binary(&self) -> &'static str {
        match self {
            Engine::Codex => "codex",
            Engine::Claude => "claude",
        }
    }

    /// Name of the environment variable that carries the API key (spec §9:
    /// it is injected per process, never written to disk). Claude honours
    /// `auth_mode`: "api_key" → x-api-key header, anything else →
    /// Authorization: Bearer (the relay convention, also the default).
    pub fn key_env(&self, auth_mode: Option<&str>) -> &'static str {
        match self {
            Engine::Codex => "OPENAI_API_KEY",
            Engine::Claude => match auth_mode {
                Some(m) if m.trim().eq_ignore_ascii_case("api_key") => "ANTHROPIC_API_KEY",
                _ => "ANTHROPIC_AUTH_TOKEN",
            },
        }
    }
}

impl Profile {
    /// The engine this profile launches (legacy/empty `cli` → Codex).
    pub fn engine(&self) -> Engine {
        Engine::parse(&self.cli)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::{CodexConfig, ModelConfig, ProviderConfig};

    fn profile(cli: &str) -> Profile {
        Profile {
            id: "t".into(),
            name: "T".into(),
            description: String::new(),
            provider: ProviderConfig {
                provider_type: "relay".into(),
                base_url: "https://example.com/v1".into(),
                api_key: None,
                api_key_env: None,
                auth_mode: None,
            },
            model: ModelConfig {
                default: "m".into(),
                effort: None,
                context_window: None,
                models: Vec::new(),
            },
            codex: CodexConfig { provider_name: "t".into() },
            cli: cli.into(),
        }
    }

    #[test]
    fn empty_cli_defaults_to_codex() {
        assert_eq!(profile("").engine(), Engine::Codex);
        assert_eq!(profile("codex").engine(), Engine::Codex);
    }

    #[test]
    fn claude_cli_selects_claude() {
        assert_eq!(profile("claude").engine(), Engine::Claude);
        assert_eq!(profile("Claude").engine(), Engine::Claude);
    }

    #[test]
    fn key_env_varies_by_engine_and_auth_mode() {
        let p = profile("claude");
        assert_eq!(p.engine().key_env(None), "ANTHROPIC_AUTH_TOKEN");
        assert_eq!(p.engine().key_env(Some("auth_token")), "ANTHROPIC_AUTH_TOKEN");
        assert_eq!(p.engine().key_env(Some("api_key")), "ANTHROPIC_API_KEY");
        assert_eq!(profile("codex").engine().key_env(None), "OPENAI_API_KEY");
    }

    #[test]
    fn home_env_and_binary() {
        assert_eq!(Engine::Codex.home_env(), "CODEX_HOME");
        assert_eq!(Engine::Claude.home_env(), "CLAUDE_CONFIG_DIR");
        assert_eq!(Engine::Codex.binary(), "codex");
        assert_eq!(Engine::Claude.binary(), "claude");
        assert_eq!(Engine::Codex.home_dir_name(), ".codex");
        assert_eq!(Engine::Claude.home_dir_name(), ".claude");
    }
}
