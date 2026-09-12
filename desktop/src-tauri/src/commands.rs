//! Tauri invoke handlers (Profile Manager + Launcher + Settings) and the
//! profiles file watcher. All commands follow the exact frontend<->backend
//! contract:
//!
//! - list_profiles()  -> Vec<ProfileView>
//! - get_profile(id)  -> Profile  (real api_key, "" when absent)
//! - save_profile(p)  -> { id }
//! - delete_profile(id) -> { id }
//! - test_connection({ base_url, api_key, model, engine, auth_mode,
//!     provider_type }) -> { ok, message, model_count }
//!     (provider_type "official": keyless = "login not testable here" ok;
//!      claude always uses the vendor's x-api-key header)
//! - fetch_models({ base_url, api_key, engine, auth_mode, provider_type })
//!     -> { ok, message, models: [id, ...] }
//! - launch_profile(id, workspace) -> { runtime_id, script_path, warning }
//!     (codex: the profile's model list is auto-synced from the server
//!      first — strict replace; a sync failure / terminal fallback is
//!      surfaced via `warning`)
//! - login_profile(id) -> { runtime_id, script_path, warning }
//!     (official login: terminal with ONLY the isolated-home env)
//! - list_sessions()  -> Vec<SessionView>
//!     (session_id, engine, profile_id, profile_name, provider_type,
//!      cwd, preview, modified [unix secs], resumable, pinned, title,
//!      open)
//! - resume_session(session_id) -> { runtime_id, script_path, warning }
//!     (errors when the session is still open in a running terminal)
//! - get_status(force?) -> { codex_found, codex_version, claude_found,
//!     claude_version, config_dir, profiles_count, version }
//!     (the expensive CLI version probes are cached ~30 s per process;
//!      pass force=true to bypass the cache)
//! - get_settings()   -> SettingsOut
//! - save_settings(s) -> ()
//! - get_logs(lines?) -> { path, lines: [str, ...] }
//! - detect_terminals() -> Vec<{ label, bin, path }>
//!
//! Event: "profiles://changed" is emitted (debounced ~400 ms) when a
//! profile .toml file is created, modified or deleted.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::Emitter;

use agent_switch_core::claude;
use agent_switch_core::codex;
use agent_switch_core::engine::Engine;
use agent_switch_core::health;
use agent_switch_core::launcher;
use agent_switch_core::logging;
use agent_switch_core::profile::Profile;
use agent_switch_core::profile_store::{config_root, ProfileStore};
use agent_switch_core::settings::Settings;
use agent_switch_core::terminal;
use notify::Watcher;

/// View shape for list_profiles(): no secrets, key presence as a flag.
#[derive(Debug, Serialize)]
pub struct ProfileView {
    pub id: String,
    pub name: String,
    pub description: String,
    pub model: String,
    pub base_url: String,
    pub provider_type: String,
    /// "codex" or "claude" (legacy profiles normalize to "codex").
    pub cli: String,
    pub has_api_key: bool,
}

/// Full profile per contract; api_key is the real string ("" when absent).
#[derive(Debug, Serialize)]
pub struct ProfileOut {
    pub id: String,
    pub name: String,
    pub description: String,
    /// "codex" or "claude" (legacy profiles normalize to "codex").
    pub cli: String,
    pub provider: ProviderOut,
    pub model: ModelOut,
    pub codex: CodexOut,
}

#[derive(Debug, Serialize)]
pub struct ProviderOut {
    #[serde(rename = "type")]
    pub provider_type: String,
    pub base_url: String,
    pub api_key: String,
    /// Env var name holding the API key (spec §9); null when absent.
    pub api_key_env: Option<String>,
    /// Claude only: "auth_token" (Bearer) or "api_key" (x-api-key);
    /// null when unset (launch defaults to auth_token).
    pub auth_mode: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ModelOut {
    pub default: String,
    pub effort: Option<String>,
    /// Codex only: context window in tokens (null when unset).
    pub context_window: Option<u32>,
    /// Every known model id for the provider (server list merged with
    /// manual entries). Written into the codex model catalog, so this is
    /// what the TUI's /model switcher offers.
    pub models: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct CodexOut {
    pub provider_name: String,
}

/// Profile input per contract (same shape as ProfileOut).
#[derive(Debug, Deserialize)]
pub struct ProfileIn {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// Absent from older frontends → defaults to codex.
    #[serde(default)]
    pub cli: String,
    pub provider: ProviderIn,
    pub model: ModelIn,
    #[serde(default)]
    pub codex: CodexIn,
}

#[derive(Debug, Deserialize)]
pub struct ProviderIn {
    #[serde(rename = "type")]
    pub provider_type: String,
    pub base_url: String,
    #[serde(default)]
    pub api_key: Option<String>,
    /// Env var name holding the API key (spec §9); must round-trip on save.
    #[serde(default)]
    pub api_key_env: Option<String>,
    /// Claude only; round-trips on save.
    #[serde(default)]
    pub auth_mode: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ModelIn {
    pub default: String,
    #[serde(default)]
    pub effort: Option<String>,
    /// Codex only: context window in tokens; absent/null when unset.
    #[serde(default)]
    pub context_window: Option<u32>,
    /// Every known model id (absent from older frontends → empty).
    #[serde(default)]
    pub models: Vec<String>,
}

#[derive(Debug, Deserialize, Default)]
pub struct CodexIn {
    #[serde(default)]
    pub provider_name: String,
}

#[derive(Debug, Deserialize)]
pub struct TestInput {
    pub base_url: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub api_key_env: Option<String>,
    #[serde(default)]
    pub model: String,
    /// "codex" (OpenAI /models) or "claude" (Anthropic /v1/messages);
    /// absent from older frontends → codex.
    #[serde(default)]
    pub engine: String,
    #[serde(default)]
    pub auth_mode: Option<String>,
    /// "official" → vendor conventions (key optional, x-api-key header).
    #[serde(default)]
    pub provider_type: String,
}

#[derive(Debug, Serialize)]
pub struct TestOut {
    pub ok: bool,
    pub message: String,
    pub model_count: Option<u32>,
}

/// Model-list fetch for the GIVEN fields (same shape as TestInput without
/// the model — the point is to discover model ids).
#[derive(Debug, Deserialize)]
pub struct ModelsInput {
    pub base_url: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub api_key_env: Option<String>,
    #[serde(default)]
    pub engine: String,
    #[serde(default)]
    pub auth_mode: Option<String>,
    /// "official" → the vendor's x-api-key convention is forced (claude).
    #[serde(default)]
    pub provider_type: String,
}

#[derive(Debug, Serialize)]
pub struct ModelsOut {
    pub ok: bool,
    pub message: String,
    pub models: Vec<String>,
    /// Context window in tokens when the server advertises one
    /// (vLLM's `max_model_len`); null otherwise.
    pub max_model_len: Option<u32>,
}

#[derive(Debug, Serialize)]
pub struct IdOut {
    pub id: String,
}

#[derive(Debug, Serialize)]
pub struct LaunchOut {
    pub runtime_id: String,
    pub script_path: String,
    pub warning: Option<String>,
}

/// One resumable conversation from the per-profile session pool.
#[derive(Debug, Serialize)]
pub struct SessionView {
    pub session_id: String,
    /// "codex" or "claude".
    pub engine: String,
    pub profile_id: String,
    /// Profile display name; falls back to profile_id when the profile
    /// was deleted (the session row stays, just unresumable).
    pub profile_name: String,
    /// Provider category chip ("relay" / "vllm" / …); "" when the profile
    /// no longer exists.
    pub provider_type: String,
    pub cwd: Option<String>,
    pub preview: String,
    /// Last activity, unix seconds.
    pub modified: u64,
    /// True while the owning profile still exists (resume possible).
    pub resumable: bool,
    /// Pinned by the user (floats to the top of the pool).
    pub pinned: bool,
    /// Custom display title; null when the preview is the row's label.
    pub title: Option<String>,
    /// True while a launched terminal still runs this session (the
    /// session can only be resumed again after it is closed).
    pub open: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct StatusOut {
    pub codex_found: bool,
    pub codex_version: Option<String>,
    pub claude_found: bool,
    pub claude_version: Option<String>,
    pub config_dir: String,
    pub profiles_count: u64,
    /// App version (About page).
    pub version: String,
}

/// Global launch settings (settings.toml), shared by the CLI and the GUI.
#[derive(Debug, Clone, Serialize)]
pub struct SettingsOut {
    /// Dangerous mode: launch CLIs with their bypass flags.
    pub dangerous_mode: bool,
    /// Proxy host ("" = disabled / direct connection).
    pub proxy_host: String,
    /// Proxy port (used only when proxy_host is non-empty).
    pub proxy_port: u16,
    /// Pinned terminal path ("" = auto-detect).
    pub terminal: String,
    /// Where the settings file lives (shown in the UI footer).
    pub path: String,
}

#[derive(Debug, Deserialize)]
pub struct SettingsIn {
    pub dangerous_mode: bool,
    #[serde(default)]
    pub proxy_host: String,
    #[serde(default)]
    pub proxy_port: u16,
    #[serde(default)]
    pub terminal: String,
}

/// One terminal the launcher knows how to open.
#[derive(Debug, Clone, Serialize)]
pub struct TerminalOut {
    pub label: String,
    pub bin: String,
    pub path: String,
}

/// The tail of the log file for the GUI log viewer.
#[derive(Debug, Serialize)]
pub struct LogsOut {
    /// Null before the log file could be opened.
    pub path: Option<String>,
    pub lines: Vec<String>,
}

/// Convert a stored profile into the contract `Profile` output shape.
fn profile_out(p: &Profile) -> ProfileOut {
    ProfileOut {
        id: p.id.clone(),
        name: p.name.clone(),
        description: p.description.clone(),
        cli: p.engine().value().to_string(),
        provider: ProviderOut {
            provider_type: p.provider.provider_type.clone(),
            base_url: p.provider.base_url.clone(),
            api_key: p
                .provider
                .api_key
                .clone()
                .unwrap_or_default(),
            api_key_env: p
                .provider
                .api_key_env
                .clone()
                .filter(|k| !k.is_empty()),
            auth_mode: p
                .provider
                .auth_mode
                .clone()
                .filter(|k| !k.is_empty()),
        },
        model: ModelOut {
            default: p.model.default.clone(),
            effort: p.model.effort.clone(),
            context_window: p.model.context_window,
            models: p.model.models.clone(),
        },
        codex: CodexOut {
            provider_name: p.codex.provider_name.clone(),
        },
    }
}

/// Convert the contract input shape into a core Profile. An empty api_key
/// string means "absent" (stored as None); an empty cli means "codex"
/// (legacy frontends).
fn profile_in(p: ProfileIn) -> Profile {
    Profile {
        id: p.id,
        name: p.name,
        description: p.description,
        provider: agent_switch_core::profile::ProviderConfig {
            provider_type: p.provider.provider_type,
            base_url: p.provider.base_url,
            api_key: p.provider.api_key.filter(|k| !k.is_empty()),
            // Round-trip the env var name (spec §9); empty string means absent.
            api_key_env: p.provider.api_key_env.filter(|k| !k.is_empty()),
            auth_mode: p
                .provider
                .auth_mode
                .filter(|k| !k.trim().is_empty())
                .map(|k| k.trim().to_ascii_lowercase()),
        },
        model: agent_switch_core::profile::ModelConfig {
            default: p.model.default,
            effort: p
                .model
                .effort
                .filter(|e| !e.trim().is_empty())
                .map(|e| e.trim().to_string()),
            context_window: p.model.context_window,
            // Trim entries, drop empties and duplicates (order kept);
            // the stored profile should stay as clean as the input.
            models: {
                let mut seen: std::collections::HashSet<String> =
                    std::collections::HashSet::new();
                p.model
                    .models
                    .into_iter()
                    .map(|m| m.trim().to_string())
                    .filter(|m| !m.is_empty() && seen.insert(m.clone()))
                    .collect()
            },
        },
        codex: agent_switch_core::profile::CodexConfig {
            provider_name: p.codex.provider_name,
        },
        cli: if p.cli.trim().is_empty() {
            "codex".to_string()
        } else {
            p.cli.trim().to_ascii_lowercase()
        },
    }
}

/// All parseable profiles, sorted by id.
#[tauri::command]
pub async fn list_profiles() -> Result<Vec<ProfileView>, String> {
    let profiles = ProfileStore::new()
        .list()
        .map_err(|e| e.to_string())?;
    Ok(profiles
        .iter()
        .map(|p| ProfileView {
            id: p.id.clone(),
            name: p.name.clone(),
            description: p.description.clone(),
            model: p.model.default.clone(),
            base_url: p.provider.base_url.clone(),
            provider_type: p.provider.provider_type.clone(),
            cli: p.engine().value().to_string(),
            has_api_key: p
                .provider
                .api_key
                .as_deref()
                .map(|k| !k.is_empty())
                .unwrap_or(false)
                || p
                    .provider
                    .api_key_env
                    .as_deref()
                    .map(|k| !k.is_empty())
                    .unwrap_or(false),
        })
        .collect())
}

/// One full profile (real api_key, "" when absent).
#[tauri::command]
pub async fn get_profile(id: String) -> Result<ProfileOut, String> {
    let p = ProfileStore::new()
        .get(&id)
        .map_err(|e| e.to_string())?;
    Ok(profile_out(&p))
}

/// Validate and persist a profile. If the id already exists the file is
/// updated in place, otherwise a new profile is created. When `create` is
/// true (the GUI "New profile" flow) an existing id is an error instead of
/// a silent overwrite.
#[tauri::command]
pub async fn save_profile(p: ProfileIn, create: Option<bool>) -> Result<IdOut, String> {
    let store = ProfileStore::new();
    let profile = profile_in(p);
    let id = profile.id.clone();
    if create.unwrap_or(false) && store.path_for(&id).exists() {
        return Err(format!("profile already exists: {id}"));
    }
    // old_id == profile.id means "in place update" (no rename bookkeeping).
    let old = if store.path_for(&id).exists() { Some(id.as_str()) } else { None };
    store.save(&profile, old).map_err(|e| e.to_string())?;
    Ok(IdOut { id })
}

/// Delete a profile by id.
#[tauri::command]
pub async fn delete_profile(id: String) -> Result<IdOut, String> {
    ProfileStore::new()
        .delete(&id)
        .map_err(|e| e.to_string())?;
    Ok(IdOut { id })
}

fn resolve_editor_key(literal: &str, env_name: Option<&str>) -> String {
    env_name.and_then(|name| std::env::var(name).ok()).filter(|v| !v.is_empty()).unwrap_or_else(|| literal.to_string())
}

/// Health-check the GIVEN fields (not a stored profile). The engine picks
/// the wire protocol: codex → GET <base_url>/models, claude → POST
/// <base_url>/v1/messages with a 1-token request.
#[tauri::command]
pub async fn test_connection(t: TestInput) -> Result<TestOut, String> {
    let key = resolve_editor_key(&t.api_key, t.api_key_env.as_deref());
    let engine = Engine::parse(&t.engine);
    let official = t.provider_type.trim().eq_ignore_ascii_case("official");
    let r = health::test_engine(
        engine,
        &t.base_url,
        &key,
        &t.model,
        t.auth_mode.as_deref(),
        official,
    )
    .map_err(|e| e.to_string())?;
    Ok(TestOut {
        ok: r.ok,
        message: r.message,
        model_count: r.model_count,
    })
}

/// Fetch the provider's model list for the GIVEN fields. The engine picks
/// the endpoint: codex → GET <base>/models, claude → GET <base>/v1/models
/// (each falls back to the other URL convention).
#[tauri::command]
pub async fn fetch_models(m: ModelsInput) -> Result<ModelsOut, String> {
    let key = resolve_editor_key(&m.api_key, m.api_key_env.as_deref());
    let engine = Engine::parse(&m.engine);
    // Official claude always speaks the vendor's x-api-key convention.
    let auth_mode = if m
        .provider_type
        .trim()
        .eq_ignore_ascii_case("official")
    {
        Some("api_key")
    } else {
        m.auth_mode.as_deref()
    };
    let r = agent_switch_core::models::fetch_models(
        engine,
        &m.base_url,
        &key,
        auth_mode,
    )
    .map_err(|e| e.to_string())?;
    Ok(ModelsOut {
        ok: r.ok,
        message: r.message,
        max_model_len: r.max_model_len,
        models: r.models,
    })
}

/// Prepare an isolated runtime + start script, then open it in the user's
/// system terminal. A terminal-open failure does not fail the command:
/// the payload is still returned with `warning` set.
#[tauri::command]
pub async fn launch_profile(id: String, workspace: Option<String>) -> Result<LaunchOut, String> {
    let store = ProfileStore::new();
    let profile = store.get(&id).map_err(|e| e.to_string())?;
    let workspace: PathBuf = match workspace {
        Some(w) => PathBuf::from(w),
        None => std::env::current_dir().map_err(|e| e.to_string())?,
    };
    // Codex only: before a NEW conversation, refresh the profile's model
    // list from the server (additive merge, persisted in place). The
    // merged list feeds the model catalog, which is what the TUI's
    // /model switcher offers. Never blocks the launch — a failure is
    // just a warning, the stored list is used as-is.
    let (profile, sync_note) = if profile.engine() == Engine::Codex {
        match agent_switch_core::models::sync_profile_models(&store, &profile) {
            Ok(r) => r,
            Err(_) => (profile, None),
        }
    } else {
        (profile, None)
    };
    // Global launch settings (dangerous-mode flag, proxy env, pinned
    // terminal) apply to every launch.
    let settings = Settings::load();
    let launch = launcher::prepare_terminal_launch(
        &store,
        &profile,
        &settings,
        &workspace,
        &[],
        "start",
    )
    .map_err(|e| e.to_string())?;
    // The key travels in the terminal process environment only — it is not
    // written to the start script (spec §9). A terminal-open failure wins
    // over the fallback note, which wins over the sync note in `warning`.
    let warning =
        match launcher::open_in_system_terminal(&launch.script_path, &workspace, &launch.env, &settings.terminal)
        {
            Ok(outcome) => {
                if outcome.used_fallback {
                    Some(format!(
                        "configured terminal unavailable; opened in '{}' instead",
                        outcome.label
                    ))
                } else {
                    sync_note
                }
            }
            Err(e) => Some(e.to_string()),
        };
    Ok(LaunchOut {
        runtime_id: launch.runtime_id,
        script_path: launch.script_path.to_string_lossy().into_owned(),
        warning,
    })
}

/// Official-account login for a profile: opens a terminal in the
/// profile's isolated home running `codex login` / `claude` (login
/// screen). The script carries ONLY the isolated-home env — no provider
/// vars, no key: the subscription credential lands in that home and
/// nowhere else.
#[tauri::command]
pub async fn login_profile(id: String) -> Result<LaunchOut, String> {
    let store = ProfileStore::new();
    let profile = store.get(&id).map_err(|e| e.to_string())?;
    let workspace = std::env::current_dir().map_err(|e| e.to_string())?;
    let launch = launcher::prepare_login(&store, &profile, &workspace)
        .map_err(|e| e.to_string())?;
    let warning = match launcher::open_in_system_terminal(
        &launch.script_path,
        &workspace,
        &[],
        &Settings::load().terminal,
    ) {
        Ok(outcome) => {
            if outcome.used_fallback {
                Some(format!(
                    "configured terminal unavailable; opened in '{}' instead",
                    outcome.label
                ))
            } else {
                None
            }
        }
        Err(e) => Some(e.to_string()),
    };
    Ok(LaunchOut {
        runtime_id: launch.runtime_id,
        script_path: launch.script_path.to_string_lossy().into_owned(),
        warning,
    })
}

/// The conversation pool: every session found in the per-profile
/// `runtime/<profile-id>` homes, newest first, enriched with the owning
/// profile's name and provider type (chip data for the UI).
#[tauri::command]
pub async fn list_sessions() -> Result<Vec<SessionView>, String> {
    let store = ProfileStore::new();
    let profiles = store.list().unwrap_or_default();
    let by_id: std::collections::HashMap<&str, &Profile> = profiles
        .iter()
        .map(|p| (p.id.as_str(), p))
        .collect();
    let pool = agent_switch_core::sessions::list_sessions(&store)
        .map_err(|e| e.to_string())?;
    let meta = agent_switch_core::sessions::load_meta(&store);
    // Sessions still running in a terminal (also prunes dead locks).
    let open_ids = agent_switch_core::sessions::open_session_ids(&store);
    Ok(pool
        .into_iter()
        .map(|s| {
            let p = by_id.get(s.profile_id.as_str()).copied();
            SessionView {
                session_id: s.session_id.clone(),
                engine: s.engine.value().to_string(),
                profile_id: s.profile_id.clone(),
                profile_name: p
                    .map(|p| p.name.clone())
                    .unwrap_or_else(|| s.profile_id.clone()),
                provider_type: p
                    .map(|p| p.provider.provider_type.clone())
                    .unwrap_or_default(),
                cwd: s.cwd,
                preview: s.preview,
                modified: s
                    .modified
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0),
                resumable: p.is_some(),
                pinned: meta.is_pinned(&s.session_id),
                title: meta.titles.get(&s.session_id).cloned(),
                open: open_ids.contains(&s.session_id),
            }
        })
        .collect())
}

/// Pin or unpin a pool session (persisted in sessions-meta.json).
#[tauri::command]
pub async fn set_session_pinned(session_id: String, pinned: bool) -> Result<(), String> {
    let store = ProfileStore::new();
    agent_switch_core::sessions::find_session(&store, &session_id)
        .map_err(|e| e.to_string())?;
    agent_switch_core::sessions::set_pinned(&store, &session_id, pinned)
        .map_err(|e| e.to_string())
}

/// Set a session's custom title; an empty title clears it.
#[tauri::command]
pub async fn set_session_title(session_id: String, title: String) -> Result<(), String> {
    let store = ProfileStore::new();
    agent_switch_core::sessions::find_session(&store, &session_id)
        .map_err(|e| e.to_string())?;
    agent_switch_core::sessions::set_title(&store, &session_id, &title)
        .map_err(|e| e.to_string())
}

/// Delete a session: removes its transcript file(s) from the profile home
/// and its pin/title meta. Nothing outside `runtime/` is ever touched.
#[tauri::command]
pub async fn delete_session(session_id: String) -> Result<(), String> {
    let store = ProfileStore::new();
    agent_switch_core::sessions::find_session(&store, &session_id)
        .map_err(|e| e.to_string())?;
    agent_switch_core::sessions::delete_session(&store, &session_id)
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// One-click cleanup: delete every pool session that is neither pinned nor
/// currently open. Returns how many sessions were removed.
#[tauri::command]
pub async fn clear_unpinned_sessions() -> Result<usize, String> {
    let store = ProfileStore::new();
    agent_switch_core::sessions::clear_unpinned_sessions(&store).map_err(|e| e.to_string())
}

/// Resume a session from the pool: same isolated-runtime launch flow as
/// `launch_profile`, but in the session's original workspace (when it
/// still exists) and with the engine's resume args appended.
#[tauri::command]
pub async fn resume_session(session_id: String) -> Result<LaunchOut, String> {
    let store = ProfileStore::new();
    let session = agent_switch_core::sessions::find_session(&store, &session_id)
        .map_err(|e| e.to_string())?;
    // A session that is still running must not be resumed again — a second
    // process writing the same transcript would corrupt it.
    if agent_switch_core::sessions::session_is_open(&store, &session_id) {
        return Err("session already open: close the running terminal window first".to_string());
    }
    let profile = store
        .get(&session.profile_id)
        .map_err(|e| format!("profile no longer exists: {e}"))?;
    let workspace: PathBuf = match session.cwd.as_deref() {
        Some(c) if Path::new(c).is_dir() => PathBuf::from(c),
        _ => std::env::current_dir().map_err(|e| e.to_string())?,
    };
    // A per-session start-script name keeps the session id visible in the
    // launched process tree, which is how the "open" state is detected.
    let stem: String = format!(
        "resume-{}",
        session_id
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '.')
            .collect::<String>()
    );
    // Global launch settings apply to resumes as well.
    let settings = Settings::load();
    let launch = launcher::prepare_terminal_launch(
        &store,
        &profile,
        &settings,
        &workspace,
        &session.resume_args(),
        &stem,
    )
    .map_err(|e| e.to_string())?;
    // Record the session as open. The atomic create_new inside is the
    // real double-open guard (covers the race between the check above and
    // now); on a failure the lock already exists, so we must not remove it.
    if let Err(e) = agent_switch_core::sessions::mark_session_open(&store, &session_id) {
        return Err(e.to_string());
    }
    let warning =
        match launcher::open_in_system_terminal(&launch.script_path, &workspace, &launch.env, &settings.terminal)
        {
            Ok(outcome) => {
                if outcome.used_fallback {
                    Some(format!(
                        "configured terminal unavailable; opened in '{}' instead",
                        outcome.label
                    ))
                } else {
                    None
                }
            }
            // Nothing was actually launched: release the lock again.
            Err(e) => {
                agent_switch_core::sessions::clear_open_lock(&store, &session_id);
                Some(e.to_string())
            }
        };
    Ok(LaunchOut {
        runtime_id: launch.runtime_id,
        script_path: launch.script_path.to_string_lossy().into_owned(),
        warning,
    })
}

/// App/environment status for the UI header (both CLIs checked).
///
/// The CLI version probes spawn a node process each, so they are cached
/// per process for `STATUS_TTL`; the cheap fields (config dir, profile
/// count) are refreshed on every call. `force=true` bypasses the cache.
const STATUS_TTL: Duration = Duration::from_secs(30);
static STATUS_CACHE: Mutex<Option<(Instant, StatusOut)>> = Mutex::new(None);

#[tauri::command]
pub async fn get_status(force: Option<bool>) -> Result<StatusOut, String> {
    let guard = STATUS_CACHE.lock().ok();
    if !force.unwrap_or(false) {
        if let Some(ref cache) = guard {
            if let Some((t, s)) = &**cache {
                if t.elapsed() < STATUS_TTL {
                    // Cheap fields stay fresh; only the version probes are
                    // cached.
                    let profiles_count = ProfileStore::new()
                        .list()
                        .map(|v| v.len() as u64)
                        .unwrap_or(0);
                    return Ok(StatusOut {
                        profiles_count,
                        ..s.clone()
                    });
                }
            }
        }
    }
    let codex_found = codex::find_codex().is_ok();
    let codex_version = codex::codex_version();
    let claude_found = claude::find_claude().is_ok();
    let claude_version = claude::claude_version();
    let config_dir = config_root().to_string_lossy().into_owned();
    let profiles_count = ProfileStore::new()
        .list()
        .map(|v| v.len() as u64)
        .unwrap_or(0);
    let out = StatusOut {
        codex_found,
        codex_version,
        claude_found,
        claude_version,
        config_dir,
        profiles_count,
        version: env!("CARGO_PKG_VERSION").to_string(),
    };
    if let Some(mut cache) = guard {
        *cache = Some((Instant::now(), out.clone()));
    }
    Ok(out)
}

/// Read the global launch settings.
#[tauri::command]
pub async fn get_settings() -> Result<SettingsOut, String> {
    let s = Settings::load();
    Ok(SettingsOut {
        path: Settings::path().to_string_lossy().into_owned(),
        dangerous_mode: s.dangerous_mode,
        proxy_host: s.proxy_host,
        proxy_port: s.proxy_port,
        terminal: s.terminal,
    })
}

/// Persist the global launch settings (atomic write in core).
#[tauri::command]
pub async fn save_settings(s: SettingsIn) -> Result<(), String> {
    let settings = Settings {
        dangerous_mode: s.dangerous_mode,
        proxy_host: s.proxy_host.trim().to_string(),
        proxy_port: s.proxy_port,
        terminal: s.terminal.trim().to_string(),
    };
    settings.save().map_err(|e| e.to_string())?;
    logging::info(&format!(
        "settings updated: dangerous_mode={}, proxy={}, terminal={}",
        settings.dangerous_mode,
        settings.proxy_url().unwrap_or_else(|| "disabled".into()),
        if settings.terminal.is_empty() {
            "auto-detect".to_string()
        } else {
            settings.terminal.clone()
        },
    ));
    Ok(())
}

/// Tail of the log file (same file the CLI `logs` command shows).
#[tauri::command]
pub async fn get_logs(lines: Option<usize>) -> Result<LogsOut, String> {
    let path = logging::path().map(|p| p.to_string_lossy().into_owned());
    Ok(LogsOut {
        path,
        lines: logging::recent_lines(lines.unwrap_or(200)),
    })
}

/// All known terminals found on this machine, in auto-detect order.
#[tauri::command]
pub async fn detect_terminals() -> Result<Vec<TerminalOut>, String> {
    Ok(terminal::detect_terminals()
        .into_iter()
        .map(|t| TerminalOut {
            label: t.label,
            bin: t.bin,
            path: t.path,
        })
        .collect())
}

/// True for create/modify/delete events touching a .toml file.
fn is_toml_change(event: &notify::Event) -> bool {
    let kind_matches = matches!(
        event.kind,
        notify::EventKind::Create(_)
            | notify::EventKind::Modify(_)
            | notify::EventKind::Remove(_)
    );
    kind_matches
        && event
            .paths
            .iter()
            .any(|p| p.extension().and_then(|e| e.to_str()) == Some("toml"))
}

/// Spawn the profiles watcher: notify::RecommendedWatcher on
/// <config_root>/profiles/ (created if missing), debounced ~400 ms, then
/// emit "profiles://changed" to all windows.
pub fn spawn_profile_watcher(app_handle: tauri::AppHandle) {
    let profiles_dir = config_root().join("profiles");
    std::thread::spawn(move || {
        if let Err(e) = run_watcher(app_handle, &profiles_dir) {
            eprintln!("profile watcher failed: {e}");
        }
    });
}

fn run_watcher(app_handle: tauri::AppHandle, profiles_dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all(profiles_dir)?;
    let (tx, rx) = std::sync::mpsc::channel();
    let mut watcher =
        notify::RecommendedWatcher::new(tx, notify::Config::default())?;
    watcher.watch(profiles_dir, notify::RecursiveMode::NonRecursive)?;

    let debounce = Duration::from_millis(400);
    let tick = Duration::from_millis(100);
    let mut dirty = false;
    let mut last = Instant::now();
    loop {
        match rx.recv_timeout(tick) {
            Ok(Ok(event)) if is_toml_change(&event) => {
                dirty = true;
                last = Instant::now();
            }
            // Non-toml paths, watch errors, timeouts: just keep ticking.
            Ok(_) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if dirty && last.elapsed() >= debounce {
            dirty = false;
            last = Instant::now();
            let _ = app_handle.emit(
                "profiles://changed",
                serde_json::json!({ "source": "watcher" }),
            );
        }
    }
    Ok(())
}
