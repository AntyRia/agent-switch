use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::profile::Profile;

/// Render the isolated Codex `config.toml` for a profile.
/// The API key is NEVER written here — it is injected per process at launch.
pub fn generate_codex_config(p: &Profile) -> String {
    let name = &p.codex.provider_name;
    // Codex >= 0.148 removed `wire_api = "chat"` support entirely; every
    // provider type must use the Responses wire API.
    let q = |s: &str| toml::Value::String(s.to_string()).to_string();
    format!(
        "model = {}\nmodel_provider = {}\n\n[model_providers.{name}]\nname = {}\nbase_url = {}\nenv_key = \"OPENAI_API_KEY\"\nwire_api = \"responses\"\n",
        q(&p.model.default),
        q(name),
        q(&p.name),
        q(&p.provider.base_url),
    )
}

/// Locate the codex executable on PATH (Windows PATHEXT covers .cmd/.exe).
pub fn find_codex() -> Result<PathBuf> {
    which::which("codex").map_err(|_| Error::CodexNotFound)
}

/// Windows cannot exec .cmd/.bat directly — they need a cmd.exe wrapper.
pub fn needs_cmd_wrap(path: &Path) -> bool {
    cfg!(windows)
        && path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| {
                let e = e.to_ascii_lowercase();
                e == "cmd" || e == "bat"
            })
            .unwrap_or(false)
}

/// `codex --version`, first line of output (None when codex is missing).
pub fn codex_version() -> Option<String> {
    let path = find_codex().ok()?;
    let output = if needs_cmd_wrap(&path) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::{CodexConfig, ModelConfig, ProviderConfig};

    fn sample() -> Profile {
        Profile {
            id: "relay-a".into(),
            name: "GPT Relay A".into(),
            description: "Primary cloud relay".into(),
            provider: ProviderConfig {
                provider_type: "openai-compatible".into(),
                base_url: "https://example.com/v1".into(),
                api_key: Some("sk-super-secret-key-123".into()),
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
            cli: String::new(),
        }
    }

    #[test]
    fn config_shape_is_correct() {
        let cfg = generate_codex_config(&sample());
        assert!(cfg.contains("model = \"gpt-5.6\""));
        assert!(cfg.contains("model_provider = \"relay-a\""));
        assert!(cfg.contains("[model_providers.relay-a]"));
        assert!(cfg.contains("name = \"GPT Relay A\""));
        assert!(cfg.contains("base_url = \"https://example.com/v1\""));
        assert!(cfg.contains("env_key = \"OPENAI_API_KEY\""));
        assert!(cfg.contains("wire_api = \"responses\""));
    }

    #[test]
    fn config_never_contains_the_api_key() {
        let cfg = generate_codex_config(&sample());
        assert!(!cfg.contains("sk-super-secret-key-123"));
    }

    #[test]
    fn all_provider_types_use_responses_wire_api() {
        // The sample is openai-compatible; also check the openai type.
        let cfg = generate_codex_config(&sample());
        assert!(cfg.contains("wire_api = \"responses\""));
        assert!(!cfg.contains("wire_api = \"chat\""));
        let mut p = sample();
        p.provider.provider_type = "openai".into();
        let cfg = generate_codex_config(&p);
        assert!(cfg.contains("wire_api = \"responses\""));
    }
}
