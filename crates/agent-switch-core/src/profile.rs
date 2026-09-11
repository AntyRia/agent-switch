use serde::{Deserialize, Serialize};

/// One provider profile — the single source of truth, stored as `<id>.toml`
/// in the `profiles/` directory.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub provider: ProviderConfig,
    pub model: ModelConfig,
    #[serde(default)]
    pub codex: CodexConfig,
    /// Which CLI to launch: "codex" (OpenAI protocol) or "claude"
    /// (Anthropic protocol). Absent in legacy files → defaults to codex.
    #[serde(default)]
    pub cli: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderConfig {
    /// Provider category: "relay" (中转站), "openai-compatible" (自部署
    /// OpenAI-compatible 推理服务: vLLM / SGLang / llama.cpp / Ollama 等)
    /// or "official" (官方直连: OpenAI / Anthropic 官方服务 — API key or
    /// subscription login, no base-URL override). Legacy values
    /// "vllm"/"openai" are still accepted on load and normalized to the
    /// current values.
    #[serde(rename = "type")]
    pub provider_type: String,
    pub base_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    /// Reserved: name of an environment variable that holds the key.
    /// Takes precedence over `api_key` when the variable is set (spec §9).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
    /// Claude only: how the key is sent — "auth_token" (Authorization:
    /// Bearer, default, the relay convention) or "api_key" (x-api-key,
    /// the official API convention). Ignored by the codex engine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_mode: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelConfig {
    pub default: String,
    /// Claude only: reasoning effort, injected as CLAUDE_CODE_EFFORT_LEVEL.
    /// Set it when the server rejects the CLI's default (some vLLM builds
    /// accept only e.g. xhigh/medium/low). Ignored by the codex engine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// Codex only: the model's context window in tokens. Written into the
    /// generated model catalog. The GUI auto-fills it from the server's
    /// `max_model_len` (vLLM); when absent, Codex's default window
    /// (272000) is used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u32>,
    /// Every known model id for this provider: the profile's default model
    /// plus what the server currently reports (strict sync, refreshed on
    /// every new Codex launch — models the server no longer serves are
    /// dropped so the user is never offered a dead model; `codex-auto-*`
    /// ids are excluded). Codex: written into the generated model catalog,
    /// which is what makes the TUI's /model switcher offer all of them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CodexConfig {
    #[serde(default)]
    pub provider_name: String,
}

impl ProviderConfig {
    /// Fold the legacy values into the two current ones (display-only
    /// field — the engine always comes from the `cli` field, so this
    /// never changes routing):
    /// "vllm"/"openai" → "openai-compatible"; "relay" stays; anything else
    /// passes through untouched (validation rejects unknown values).
    pub fn normalize_provider_type(&mut self) {
        let t = self.provider_type.trim().to_ascii_lowercase();
        let normalized = match t.as_str() {
            "relay" => "relay",
            "official" => "official",
            "vllm" | "openai" | "openai-compatible" => "openai-compatible",
            other => other,
        };
        self.provider_type = normalized.to_string();
    }
}

impl Profile {
    /// True for the "official" provider type: the vendor's own service
    /// (OpenAI / Anthropic) — no relay, no self-hosted box. The generated
    /// config / env get the vendor's built-in endpoint instead of a
    /// `base_url` override, and the key is optional (a subscription login
    /// in the isolated home is an equally valid credential).
    pub fn is_official(&self) -> bool {
        self.provider
            .provider_type
            .trim()
            .eq_ignore_ascii_case("official")
    }

    /// Render the profile as TOML with a stable, human-readable field order.
    pub fn to_toml(&self) -> String {
        let mut s = String::new();
        s.push_str(&format!("id = {}\n", toml_str(&self.id)));
        s.push_str(&format!("name = {}\n", toml_str(&self.name)));
        s.push_str(&format!("description = {}\n", toml_str(&self.description)));
        if !self.cli.trim().is_empty() {
            s.push_str(&format!("cli = {}\n", toml_str(&self.cli)));
        }
        s.push_str("\n[provider]\n");
        s.push_str(&format!("type = {}\n", toml_str(&self.provider.provider_type)));
        s.push_str(&format!("base_url = {}\n", toml_str(&self.provider.base_url)));
        if let Some(key) = &self.provider.api_key {
            s.push_str(&format!("api_key = {}\n", toml_str(key)));
        }
        if let Some(env) = &self.provider.api_key_env {
            s.push_str(&format!("api_key_env = {}\n", toml_str(env)));
        }
        if let Some(mode) = &self.provider.auth_mode {
            s.push_str(&format!("auth_mode = {}\n", toml_str(mode)));
        }
        s.push_str("\n[model]\n");
        s.push_str(&format!("default = {}\n", toml_str(&self.model.default)));
        if !self.model.models.is_empty() {
            let list: Vec<toml::Value> = self
                .model
                .models
                .iter()
                .map(|m| toml::Value::String(m.clone()))
                .collect();
            s.push_str(&format!("models = {}\n", toml::Value::Array(list).to_string()));
        }
        if let Some(window) = self.model.context_window {
            s.push_str(&format!("context_window = {window}\n"));
        }
        if let Some(effort) = &self.model.effort {
            if !effort.trim().is_empty() {
                s.push_str(&format!("effort = {}\n", toml_str(effort)));
            }
        }
        s.push_str("\n[codex]\n");
        s.push_str(&format!(
            "provider_name = {}\n",
            toml_str(&self.codex.provider_name)
        ));
        s
    }
}

/// Quote a string as a TOML basic string (lets `toml` handle escaping).
fn toml_str(s: &str) -> String {
    toml::Value::String(s.to_string()).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_collapses_legacy_types() {
        for legacy in ["vllm", "openai", "VLLM", " openai-compatible "] {
            let mut p = ProviderConfig {
                provider_type: legacy.into(),
                base_url: "http://x".into(),
                api_key: None,
                api_key_env: None,
                auth_mode: None,
            };
            p.normalize_provider_type();
            assert_eq!(p.provider_type, "openai-compatible", "from {legacy:?}");
        }
        let mut p = ProviderConfig {
            provider_type: "relay".into(),
            base_url: "http://x".into(),
            api_key: None,
            api_key_env: None,
            auth_mode: None,
        };
        p.normalize_provider_type();
        assert_eq!(p.provider_type, "relay");
        // Official stays official (case/whitespace-insensitive).
        let mut p = ProviderConfig {
            provider_type: " Official ".into(),
            base_url: "http://x".into(),
            api_key: None,
            api_key_env: None,
            auth_mode: None,
        };
        p.normalize_provider_type();
        assert_eq!(p.provider_type, "official");
        // Unknown values pass through (validation rejects them).
        let mut p = ProviderConfig {
            provider_type: "azure".into(),
            base_url: "http://x".into(),
            api_key: None,
            api_key_env: None,
            auth_mode: None,
        };
        p.normalize_provider_type();
        assert_eq!(p.provider_type, "azure");
    }
}
