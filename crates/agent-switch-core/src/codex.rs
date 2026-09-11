use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::profile::Profile;

/// Standard Codex agent instructions — the `base_instructions` shared by
/// every model in Codex 0.148's built-in catalog. Custom model-catalog
/// entries REQUIRE this field (Codex rejects the file when it is missing),
/// so profiles reuse the stock template.
const STANDARD_BASE_INSTRUCTIONS: &str = include_str!("codex_prompt.txt");

/// Context window (tokens) used when the profile does not set one — the
/// same window Codex falls back to for unknown models.
const DEFAULT_CONTEXT_WINDOW: u32 = 272_000;

/// Render the isolated Codex `config.toml` for a profile.
/// The API key is NEVER written here — it is injected per process at launch.
///
/// `catalog_path` is the absolute path of the model-catalog JSON (written
/// next to this file by `create_runtime`). Top-level keys must precede any
/// `[table]` header — TOML would otherwise file them under the table.
///
/// On Windows, `[windows] sandbox = "unelevated"` is pinned: without it,
/// Codex shows an interactive "Set up the Codex agent sandbox" prompt at
/// EVERY launch (the level is read from this config on each start, and the
/// "default" option requires Administrator permissions). The unelevated
/// restricted-token sandbox is the only mode that works for unattended
/// launches from the GUI/CLI.
pub fn generate_codex_config(p: &Profile, catalog_path: &Path) -> String {
    let q = |s: &str| toml::Value::String(s.to_string()).to_string();
    if p.is_official() {
        // Official (OpenAI) direct connection: use Codex's BUILT-IN
        // `openai` provider — no `model_provider` override, no
        // [model_providers.*] table, no base_url. Credentials come from
        // the per-process OPENAI_API_KEY (when the profile carries one) or
        // the subscription login stored in the isolated home; pin the file
        // store so that login never leaks into the OS keyring (a keyring
        // entry is machine-wide and would be shared across profiles).
        let windows_section = if cfg!(target_os = "windows") {
            "\n[windows]\n# Pin the Windows sandbox mode so Codex does not show an interactive\n# sandbox-setup prompt at every launch.\nsandbox = \"unelevated\"\n"
        } else {
            ""
        };
        return format!(
            "model = {}\nmodel_catalog_json = {}\n# The startup update check pops a blocking dialog whose DEFAULT action is\n# `npm install -g @openai/codex` — wrong for a profile-managed runtime.\ncheck_for_update_on_startup = false\n# Store `codex login` credentials in this isolated home (file), not the OS\n# keyring: a keyring entry is machine-wide and would leak across profiles.\ncli_auth_credentials_store = \"file\"\n\n[features]\nplugins = false\n{windows_section}",
            q(&p.model.default),
            q(catalog_path.to_string_lossy().as_ref()),
        );
    }
    let name = &p.codex.provider_name;
    // Codex >= 0.148 removed `wire_api = "chat"` support entirely; every
    // provider type must use the Responses wire API.
    let windows_section = if cfg!(target_os = "windows") {
        "\n[windows]\n# Pin the Windows sandbox mode so Codex does not show an interactive\n# sandbox-setup prompt at every launch. \"unelevated\" (restricted-token\n# sandbox) needs no Administrator permissions — required for launches\n# from the GUI/CLI.\nsandbox = \"unelevated\"\n"
    } else {
        ""
    };
    format!(
        "model = {}\nmodel_provider = {}\nmodel_catalog_json = {}\n# The startup update check pops a blocking dialog whose DEFAULT action is\n# `npm install -g @openai/codex` (upgrades the user's global install) —\n# wrong for a profile-managed runtime, where the user controls the Codex\n# install themselves. Version stays visible via `agent-switch doctor`.\ncheck_for_update_on_startup = false\n\n[features]\n# The curated-plugin sync (a 5k-file OpenAI GitHub repo, fetched at every\n# startup) is useless without a ChatGPT login and is a known flaky point\n# on restricted networks — disable it for third-party profiles.\nplugins = false\n\n[model_providers.{name}]\nname = {}\nbase_url = {}\nenv_key = \"OPENAI_API_KEY\"\nwire_api = \"responses\"\n{windows_section}",
        q(&p.model.default),
        q(name),
        q(catalog_path.to_string_lossy().as_ref()),
        q(&p.name),
        q(&p.provider.base_url),
    )
}

/// Mirror of codex's `project_trust_key` (config crate): the
/// canonicalized path as the `[projects]` trust-map key, lowercased on
/// Windows (the trust lookup is case-insensitive there).
fn project_trust_key(path: &Path) -> String {
    let lower = |s: String| {
        if cfg!(target_os = "windows") {
            s.to_ascii_lowercase()
        } else {
            s
        }
    };
    match std::fs::canonicalize(path) {
        Ok(c) => lower(c.to_string_lossy().into_owned()),
        Err(_) => lower(path.to_string_lossy().into_owned()),
    }
}

/// Append Codex's per-directory trust state (the `[projects]` table) to
/// the freshly generated config.
///
/// Codex persists trust answers into this very `config.toml`
/// (`[projects."<key>"] trust_level = "trusted"`); the every-launch
/// rewrite would wipe them and re-ask the trust prompt on every start.
/// So the rewrite:
/// - carries over the entries Codex wrote in earlier launches, and
/// - marks the launch workspace as trusted — the same per-launch
///   workspace seeding the Claude path does (`seed_claude_home`): the
///   user explicitly launched this profile in this directory.
pub fn with_project_trust(config: String, existing: Option<&str>, workspace: &Path) -> String {
    let mut projects: toml::map::Map<String, toml::Value> = toml::map::Map::new();
    if let Some(raw) = existing {
        if let Ok(doc) = raw.parse::<toml::Value>() {
            if let Some(t) = doc.get("projects").and_then(|v| v.as_table()) {
                for (k, v) in t {
                    projects.insert(k.clone(), v.clone());
                }
            }
        }
    }
    let entry = toml::map::Map::from_iter([(
        "trust_level".to_string(),
        toml::Value::String("trusted".to_string()),
    )]);
    projects.insert(project_trust_key(workspace), toml::Value::Table(entry));
    let mut root = toml::map::Map::new();
    root.insert("projects".to_string(), toml::Value::Table(projects));
    match toml::to_string(&toml::Value::Table(root)) {
        Ok(section) => format!("{config}\n{section}"),
        // Unreachable for this shape; never block the launch on it.
        Err(_) => config,
    }
}

/// Every model id the profile declares: the default model first, then the
/// profile's `models` list (deduplicated, order preserved). The catalog
/// may still drop `codex-auto-*` ids (see `render_model_catalog`).
pub fn profile_model_ids(p: &Profile) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let default = p.model.default.trim();
    if !default.is_empty() {
        out.push(default.to_string());
    }
    for m in &p.model.models {
        let m = m.trim();
        if !m.is_empty() && !out.iter().any(|x| x == m) {
            out.push(m.to_string());
        }
    }
    out
}

/// Render the profile's Codex model-catalog JSON (the file pointed to by
/// `model_catalog_json`) — one entry per known model (see
/// `profile_model_ids`).
///
/// A `model_catalog_json` file REPLACES Codex's built-in model list
/// (verified on 0.148: only the file's entries remain in the merged
/// catalog). The catalog therefore both
/// (a) gives every profile model real metadata — correct context window,
///     no reasoning params — eliminating the "Model metadata ... not
///     found. Defaulting to fallback metadata" warning,
/// (b) removes the built-in OpenAI models, so the TUI stops pushing their
///     marketing tips (the GPT-5.6 Sol `availability_nux`) at a
///     third-party profile, and
/// (c) makes the TUI's /model switcher offer all of the profile's models.
///
/// Two picker behaviors are deliberately shaped here (verified against
/// the codex 0.148 TUI source, `chatwidget/model_popups.rs`):
/// - ids starting with `codex-auto-` are DROPPED from the catalog: with
///   even one such model present, the TUI treats them as built-in "auto
///   modes" and turns /model into a two-tier menu (auto modes + an
///   "All models" indirection) instead of one flat list. Relays serve
///   such ids (e.g. `codex-auto-review`) although they have no meaning
///   for a third-party profile.
/// - every entry carries EXACTLY ONE supported reasoning level
///   (`none`, the profile's effective default since the generated
///   config sets no `model_reasoning_effort`): the TUI applies a
///   selected model and closes the picker in one step only for
///   single-effort models; with an empty list it opens a reasoning
///   sub-popup on every selection, so the picker appears stuck (the
///   model is actually changed behind it).
///
/// Entry schema notes (empirically verified against codex 0.148):
/// - `base_instructions` is REQUIRED (a string) — missing/null aborts
///   the whole file with "expected string or map".
/// - `web_search_tool_type` must be a non-null enum value ("text").
/// - `tool_mode` / `use_responses_lite` stay null/false: the built-in
///   GPT-5.6 entries use code-mode values that would change the tool
///   protocol on third-party servers.
pub fn render_model_catalog(p: &Profile) -> String {
    let window = p.model.context_window.unwrap_or(DEFAULT_CONTEXT_WINDOW);
    let entries: Vec<serde_json::Value> = profile_model_ids(p)
        .iter()
        .filter(|m| !m.starts_with("codex-auto-"))
        .map(|model| {
            serde_json::json!({
                "slug": model,
                "display_name": model,
                "description": "Profile model managed by Agent Switch.",
                "base_instructions": STANDARD_BASE_INSTRUCTIONS,
                "default_reasoning_level": "none",
                "supported_reasoning_levels": [
                    { "effort": "none", "description": "Direct responses without extended reasoning" }
                ],
                "shell_type": "shell_command",
                "visibility": "list",
                "supported_in_api": true,
                "priority": 50,
                "additional_speed_tiers": [],
                "service_tiers": [],
                "availability_nux": null,
                "upgrade": null,
                "model_messages": null,
                "include_skills_usage_instructions": false,
                "include_plugin_usage_instructions": false,
                "include_apps_usage_instructions": false,
                "default_reasoning_summary": "none",
                "support_verbosity": false,
                "default_verbosity": "low",
                "apply_patch_tool_type": "freeform",
                "web_search_tool_type": "text",
                "truncation_policy": { "mode": "tokens", "limit": 10000 },
                "supports_image_detail_original": false,
                "context_window": window,
                "max_context_window": window,
                "comp_hash": null,
                "effective_context_window_percent": 95,
                "experimental_supported_tools": [],
                "input_modalities": ["text"],
                "supports_search_tool": false,
                "use_responses_lite": false,
                "node_repl_auto_review_required": false,
                "node_repl_disabled": false,
                "tool_mode": null,
                "multi_agent_version": null,
                "prefer_websockets": false,
                "auto_review_model_override": null,
                "auto_compact_token_limit": null,
                "supports_reasoning_summaries": false
            })
        })
        .collect();
    // A concrete serde_json::Value always serializes; this cannot fail.
    serde_json::to_string(&serde_json::json!({ "models": entries }))
        .expect("serializing a json! Value cannot fail")
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
                context_window: None,
                models: Vec::new(),
            },
            codex: CodexConfig {
                provider_name: "relay-a".into(),
            },
            cli: String::new(),
        }
    }

    #[test]
    fn config_shape_is_correct() {
        let cfg = generate_codex_config(&sample(), Path::new("/rt/relay-a/.codex/model-catalog.json"));
        assert!(cfg.contains("model = \"gpt-5.6\""));
        assert!(cfg.contains("model_provider = \"relay-a\""));
        assert!(cfg.contains("[model_providers.relay-a]"));
        assert!(cfg.contains("name = \"GPT Relay A\""));
        assert!(cfg.contains("base_url = \"https://example.com/v1\""));
        assert!(cfg.contains("env_key = \"OPENAI_API_KEY\""));
        assert!(cfg.contains("wire_api = \"responses\""));
        // The catalog pointer must be a TOP-LEVEL key: if it lands after a
        // [table] header, TOML files it under that table and Codex ignores
        // it silently (verified against codex 0.148).
        let catalog_line = cfg
            .lines()
            .position(|l| l.starts_with("model_catalog_json = "))
            .unwrap();
        let first_table = cfg
            .lines()
            .position(|l| l.starts_with('['))
            .unwrap();
        assert!(catalog_line < first_table);
        assert!(cfg.contains("model_catalog_json = \"/rt/relay-a/.codex/model-catalog.json\""));
        // The curated-plugin sync is disabled for third-party profiles.
        assert!(cfg.contains("[features]"));
        assert!(cfg.contains("plugins = false"));
    }

    #[test]
    fn config_never_contains_the_api_key() {
        let cfg = generate_codex_config(&sample(), Path::new("/rt/relay-a/.codex/model-catalog.json"));
        assert!(!cfg.contains("sk-super-secret-key-123"));
    }

    #[test]
    fn all_provider_types_use_responses_wire_api() {
        // The sample is openai-compatible; also check the openai type.
        let cfg = generate_codex_config(&sample(), Path::new("/rt/x/model-catalog.json"));
        assert!(cfg.contains("wire_api = \"responses\""));
        assert!(!cfg.contains("wire_api = \"chat\""));
        let mut p = sample();
        p.provider.provider_type = "openai".into();
        let cfg = generate_codex_config(&p, Path::new("/rt/x/model-catalog.json"));
        assert!(cfg.contains("wire_api = \"responses\""));
    }

    #[test]
    fn official_config_uses_builtin_provider_and_file_credentials() {
        let mut p = sample();
        p.provider.provider_type = "official".into();
        let cfg = generate_codex_config(&p, Path::new("/rt/relay-a/.codex/model-catalog.json"));
        // No custom provider: the built-in `openai` provider (no
        // model_provider line, no [model_providers.*] table, no base_url).
        assert!(!cfg.contains("model_provider"));
        assert!(!cfg.contains("[model_providers."));
        assert!(!cfg.contains("base_url"));
        // Login credentials stay in the isolated home, never the keyring.
        assert!(cfg.contains("cli_auth_credentials_store = \"file\""));
        // The catalog + model are still wired up.
        assert!(cfg.contains("model = \"gpt-5.6\""));
        assert!(cfg.contains("model_catalog_json = \"/rt/relay-a/.codex/model-catalog.json\""));
        // No key on disk, ever.
        assert!(!cfg.contains("sk-super-secret-key-123"));
    }

    #[test]
    fn startup_update_check_is_disabled() {
        let cfg = generate_codex_config(&sample(), Path::new("/rt/x/model-catalog.json"));
        // The startup update dialog's default action upgrades the user's
        // GLOBAL codex install — never acceptable from a profile launch.
        assert!(cfg.contains("check_for_update_on_startup = false"));
    }

    #[test]
    fn project_trust_preserves_existing_and_marks_workspace() {
        let ws = std::env::temp_dir().join("agent-switch-trust-test");
        let existing = r#"model = "old"
[projects."C:\\Users\\dev\\other"]
trust_level = "trusted"
"#;
        let cfg = generate_codex_config(&sample(), Path::new("/rt/x/model-catalog.json"));
        let cfg = with_project_trust(cfg, Some(existing), &ws);
        // Previous answers survive the rewrite…
        assert!(cfg.contains("[projects."));
        assert!(cfg.contains("trust_level = \"trusted\""));
        let doc: toml::Value = cfg.parse().unwrap();
        let projects = doc["projects"].as_table().unwrap();
        // …and the launch workspace is present with a trusted entry.
        assert!(projects
            .iter()
            .any(|(k, v)| k != r"C:\Users\dev\other"
                && v["trust_level"] == toml::Value::String("trusted".into())));
        // The managed top-level keys still come first (TOML table order).
        assert!(cfg.starts_with("model = "));
    }

    #[test]
    fn project_trust_without_existing_config() {
        let ws = std::env::temp_dir();
        let cfg = generate_codex_config(&sample(), Path::new("/rt/x/model-catalog.json"));
        let cfg = with_project_trust(cfg, None, &ws);
        let doc: toml::Value = cfg.parse().unwrap();
        let projects = doc["projects"].as_table().unwrap();
        assert!(projects
            .iter()
            .any(|(_, v)| v["trust_level"] == toml::Value::String("trusted".into())));
    }

    #[test]
    fn windows_sandbox_mode_is_pinned_on_windows() {
        let cfg = generate_codex_config(&sample(), Path::new("/rt/x/model-catalog.json"));
        if cfg!(target_os = "windows") {
            // Unattended launches must not hit the interactive
            // sandbox-setup prompt (it blocks the TUI at every start).
            assert!(cfg.contains("[windows]"));
            assert!(cfg.contains("sandbox = \"unelevated\""));
        } else {
            assert!(!cfg.contains("[windows]"));
        }
    }

    #[test]
    fn catalog_covers_the_profile_model_with_stock_template() {
        let json = render_model_catalog(&sample());
        let doc: serde_json::Value = serde_json::from_str(&json).unwrap();
        let entry = &doc["models"][0];
        assert_eq!(entry["slug"], "gpt-5.6");
        assert_eq!(entry["context_window"], DEFAULT_CONTEXT_WINDOW);
        assert_eq!(entry["max_context_window"], DEFAULT_CONTEXT_WINDOW);
        // Both schema traps that make Codex reject the file silently or
        // with "expected string or map".
        assert!(entry["base_instructions"].as_str().unwrap().len() > 1000);
        assert_eq!(entry["web_search_tool_type"], "text");
        // No code-mode tooling for third-party servers.
        assert_eq!(entry["tool_mode"], serde_json::json!(null));
        assert_eq!(entry["use_responses_lite"], serde_json::json!(false));
        // Exactly one supported effort (matching the effort-less third-
        // party default) — with an empty list the TUI's /model picker
        // would not dismiss after a selection.
        assert_eq!(entry["default_reasoning_level"], "none");
        assert_eq!(
            entry["supported_reasoning_levels"],
            serde_json::json!([
                { "effort": "none", "description": "Direct responses without extended reasoning" }
            ])
        );
    }

    #[test]
    fn catalog_drops_codex_auto_models() {
        // Relays list `codex-auto-*` ids (OpenAI "auto modes"); in the
        // TUI they turn /model into a two-tier "All models" menu, so the
        // catalog must not carry them even when the profile does.
        let mut p = sample();
        p.model.models = vec![
            "gpt-5.6-sol".into(),
            "codex-auto-review".into(),
            "codex-auto-fast".into(),
            "gpt-6".into(),
        ];
        // profile_model_ids still reports everything the profile
        // declares (the profile file is the source of truth)…
        assert_eq!(
            profile_model_ids(&p),
            vec![
                "gpt-5.6",
                "gpt-5.6-sol",
                "codex-auto-review",
                "codex-auto-fast",
                "gpt-6"
            ]
        );
        // …but the catalog drops the auto ids: the TUI shows one flat
        // list of the remaining models.
        let json = render_model_catalog(&p);
        let doc: serde_json::Value = serde_json::from_str(&json).unwrap();
        let slugs: Vec<&str> = doc["models"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["slug"].as_str().unwrap())
            .collect();
        assert_eq!(slugs, vec!["gpt-5.6", "gpt-5.6-sol", "gpt-6"]);
        assert!(!json.contains("codex-auto-"));
    }

    #[test]
    fn catalog_uses_the_profile_context_window_when_set() {
        let mut p = sample();
        p.model.context_window = Some(262_144);
        let json = render_model_catalog(&p);
        let doc: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(doc["models"][0]["context_window"], 262_144);
        assert_eq!(doc["models"][0]["max_context_window"], 262_144);
    }

    #[test]
    fn catalog_lists_every_profile_model_default_first() {
        // Relays serve models their /models endpoint never lists, so the
        // user keeps a hand-maintained `models` list; the catalog must
        // carry all of them (that is what /model offers in the TUI).
        let mut p = sample();
        p.model.models = vec![
            "gpt-5.6-sol".into(),
            "gpt-5.6".into(), // duplicate of the default: dropped
            "  gpt-6  ".into(), // whitespace: trimmed
            "   ".into(), // empty: dropped
        ];
        assert_eq!(
            profile_model_ids(&p),
            vec!["gpt-5.6", "gpt-5.6-sol", "gpt-6"]
        );
        let json = render_model_catalog(&p);
        let doc: serde_json::Value = serde_json::from_str(&json).unwrap();
        let slugs: Vec<&str> = doc["models"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["slug"].as_str().unwrap())
            .collect();
        assert_eq!(slugs, vec!["gpt-5.6", "gpt-5.6-sol", "gpt-6"]);
        // Every entry carries the required fields.
        for e in doc["models"].as_array().unwrap() {
            assert!(e["base_instructions"].as_str().unwrap().len() > 1000);
            assert_eq!(e["web_search_tool_type"], "text");
        }
    }
}
