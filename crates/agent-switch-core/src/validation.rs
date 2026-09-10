use crate::profile::Profile;

/// Maximum length of a profile id.
pub const MAX_ID_LEN: usize = 64;

fn id_char_ok(c: char) -> bool {
    c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_'
}

/// Return the list of validation errors for a profile (empty = valid).
pub fn validate_profile(p: &Profile) -> Vec<String> {
    let mut errors: Vec<String> = Vec::new();
    if p.id.is_empty() {
        errors.push("id must not be empty".to_string());
    } else if p.id.len() > MAX_ID_LEN {
        errors.push("id must be at most 64 characters".to_string());
    } else if !p.id.chars().all(id_char_ok) {
        errors.push("id may only contain a-z, 0-9, '-' and '_'".to_string());
    }
    if p.name.trim().is_empty() {
        errors.push("name must not be empty".to_string());
    }
    if !p.provider.base_url.starts_with("http://")
        && !p.provider.base_url.starts_with("https://")
    {
        errors.push(
            "provider.base_url must start with http:// or https://".to_string(),
        );
    }
    // cli: empty (legacy) or "codex"/"claude".
    let cli = p.cli.trim().to_ascii_lowercase();
    if !cli.is_empty() && cli != "codex" && cli != "claude" {
        errors.push("cli must be \"codex\" or \"claude\"".to_string());
    }
    // provider.type: relay/vllm plus the two legacy values (kept so old
    // profiles stay valid; the GUI normalizes them on save).
    let pt = p.provider.provider_type.trim().to_ascii_lowercase();
    if pt.is_empty() {
        errors.push("provider.type must not be empty".to_string());
    } else if !matches!(pt.as_str(), "relay" | "vllm" | "openai-compatible" | "openai") {
        errors.push("provider.type must be \"relay\" or \"vllm\"".to_string());
    }
    // auth_mode (claude only): absent/empty or one of the two known values.
    if let Some(mode) = p.provider.auth_mode.as_deref() {
        if !mode.trim().is_empty()
            && !mode.eq_ignore_ascii_case("auth_token")
            && !mode.eq_ignore_ascii_case("api_key")
        {
            errors.push(
                "provider.auth_mode must be \"auth_token\" or \"api_key\"".to_string(),
            );
        }
    }
    // Note: an API key is intentionally NOT required — local servers
    // (e.g. vLLM on 127.0.0.1) run fine without one; launch then injects
    // an empty OPENAI_API_KEY.
    if p.model.default.trim().is_empty() {
        errors.push("model.default must not be empty".to_string());
    }
    if p.codex.provider_name.trim().is_empty() {
        errors.push("codex.provider_name must not be empty".to_string());
    }
    errors
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::{CodexConfig, ModelConfig, ProviderConfig};

    fn profile() -> Profile {
        Profile {
            id: "relay-a".into(),
            name: "GPT Relay A".into(),
            description: "Primary cloud relay".into(),
            provider: ProviderConfig {
                provider_type: "openai-compatible".into(),
                base_url: "https://example.com/v1".into(),
                api_key: Some("sk-test".into()),
                api_key_env: None,
                auth_mode: None,
            },
            model: ModelConfig {
                default: "gpt-5.6".into(),
                effort: None,
            },
            codex: CodexConfig {
                provider_name: "relay-a".into(),
            },
            cli: "codex".into(),
        }
    }

    #[test]
    fn valid_profile_has_no_errors() {
        assert!(validate_profile(&profile()).is_empty());
    }

    #[test]
    fn bad_id_charset_rejected() {
        let mut p = profile();
        p.id = "Bad ID!".into();
        assert!(validate_profile(&p)
            .iter()
            .any(|e| e.contains("a-z, 0-9")));
    }

    #[test]
    fn missing_key_allowed_for_local_servers() {
        let mut p = profile();
        p.provider.api_key = None;
        assert!(validate_profile(&p).is_empty());
    }

    #[test]
    fn api_key_env_counts_as_key() {
        let mut p = profile();
        p.provider.api_key = None;
        p.provider.api_key_env = Some("OPENAI_API_KEY".into());
        assert!(validate_profile(&p).is_empty());
    }

    #[test]
    fn bad_url_rejected() {
        let mut p = profile();
        p.provider.base_url = "ftp://nope".into();
        assert!(validate_profile(&p)
            .iter()
            .any(|e| e.contains("base_url")));
    }

    #[test]
    fn empty_model_rejected() {
        let mut p = profile();
        p.model.default = " ".into();
        assert!(validate_profile(&p)
            .iter()
            .any(|e| e.contains("model.default")));
    }

    #[test]
    fn legacy_profile_without_cli_is_valid() {
        let mut p = profile();
        p.cli = String::new(); // files written before the field existed
        assert!(validate_profile(&p).is_empty());
    }

    #[test]
    fn unknown_cli_rejected() {
        let mut p = profile();
        p.cli = "gemini".into();
        assert!(validate_profile(&p).iter().any(|e| e.contains("cli")));
    }

    #[test]
    fn new_and_legacy_provider_types_accepted() {
        for t in ["relay", "vllm", "openai-compatible", "openai"] {
            let mut p = profile();
            p.provider.provider_type = t.into();
            assert!(
                !validate_profile(&p).iter().any(|e| e.contains("provider.type")),
                "type {t} should be accepted"
            );
        }
        let mut p = profile();
        p.provider.provider_type = "azure".into();
        assert!(validate_profile(&p).iter().any(|e| e.contains("provider.type")));
    }

    #[test]
    fn auth_mode_validated() {
        let mut p = profile();
        p.cli = "claude".into();
        p.provider.auth_mode = Some("auth_token".into());
        assert!(validate_profile(&p).is_empty());
        p.provider.auth_mode = Some("bearer".into());
        assert!(validate_profile(&p).iter().any(|e| e.contains("auth_mode")));
    }
}
