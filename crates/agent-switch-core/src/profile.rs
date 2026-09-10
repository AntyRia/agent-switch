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
    /// Provider category: "relay" (中转站) or "vllm" (local vLLM). Legacy
    /// values "openai-compatible"/"openai" are still accepted on load.
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
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CodexConfig {
    #[serde(default)]
    pub provider_name: String,
}

impl Profile {
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
