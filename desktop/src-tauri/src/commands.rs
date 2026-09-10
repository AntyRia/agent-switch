//! Tauri invoke handlers (Profile Manager + Launcher) and the profiles
//! file watcher. All commands follow the exact frontend<->backend contract:
//!
//! - list_profiles()  -> Vec<ProfileView>
//! - get_profile(id)  -> Profile  (real api_key, "" when absent)
//! - save_profile(p)  -> { id }
//! - delete_profile(id) -> { id }
//! - test_connection({ base_url, api_key, model, engine, auth_mode })
//!     -> { ok, message, model_count }
//! - fetch_models({ base_url, api_key, engine, auth_mode })
//!     -> { ok, message, models: [id, ...] }
//! - launch_profile(id, workspace) -> { runtime_id, script_path, warning }
//! - list_sessions()  -> Vec<SessionView>
//!     (session_id, engine, profile_id, profile_name, provider_type,
//!      cwd, preview, modified [unix secs], resumable)
//! - resume_session(session_id) -> { runtime_id, script_path, warning }
//! - get_status()     -> { codex_found, codex_version, claude_found,
//!                          claude_version, config_dir, profiles_count }
//!
//! Event: "profiles://changed" is emitted (debounced ~400 ms) when a
//! profile .toml file is created, modified or deleted.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::Emitter;

use agent_switch_core::claude;
use agent_switch_core::codex;
use agent_switch_core::engine::Engine;
use agent_switch_core::health;
use agent_switch_core::launcher;
use agent_switch_core::profile::Profile;
use agent_switch_core::profile_store::{config_root, ProfileStore};
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
    pub model: String,
    /// "codex" (OpenAI /models) or "claude" (Anthropic /v1/messages);
    /// absent from older frontends → codex.
    #[serde(default)]
    pub engine: String,
    #[serde(default)]
    pub auth_mode: Option<String>,
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
    pub engine: String,
    #[serde(default)]
    pub auth_mode: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ModelsOut {
    pub ok: bool,
    pub message: String,
    pub models: Vec<String>,
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
}

#[derive(Debug, Serialize)]
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

/// Health-check the GIVEN fields (not a stored profile). The engine picks
/// the wire protocol: codex → GET <base_url>/models, claude → POST
/// <base_url>/v1/messages with a 1-token request.
#[tauri::command]
pub async fn test_connection(t: TestInput) -> Result<TestOut, String> {
    let engine = Engine::parse(&t.engine);
    let r = health::test_engine(
        engine,
        &t.base_url,
        &t.api_key,
        &t.model,
        t.auth_mode.as_deref(),
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
    let engine = Engine::parse(&m.engine);
    let r = agent_switch_core::models::fetch_models(
        engine,
        &m.base_url,
        &m.api_key,
        m.auth_mode.as_deref(),
    )
    .map_err(|e| e.to_string())?;
    Ok(ModelsOut {
        ok: r.ok,
        message: r.message,
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
    let launch = launcher::prepare_terminal_launch(&store, &profile, &workspace, &[])
        .map_err(|e| e.to_string())?;
    // The key travels in the terminal process environment only — it is not
    // written to the start script (spec §9).
    let warning = match launcher::open_in_system_terminal(&launch.script_path, &workspace, &launch.env) {
        Ok(()) => None,
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
    Ok(pool
        .into_iter()
        .map(|s| {
            let p = by_id.get(s.profile_id.as_str()).copied();
            SessionView {
                session_id: s.session_id,
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
            }
        })
        .collect())
}

/// Resume a session from the pool: same isolated-runtime launch flow as
/// `launch_profile`, but in the session's original workspace (when it
/// still exists) and with the engine's resume args appended.
#[tauri::command]
pub async fn resume_session(session_id: String) -> Result<LaunchOut, String> {
    let store = ProfileStore::new();
    let session = agent_switch_core::sessions::find_session(&store, &session_id)
        .map_err(|e| e.to_string())?;
    let profile = store
        .get(&session.profile_id)
        .map_err(|e| format!("profile no longer exists: {e}"))?;
    let workspace: PathBuf = match session.cwd.as_deref() {
        Some(c) if Path::new(c).is_dir() => PathBuf::from(c),
        _ => std::env::current_dir().map_err(|e| e.to_string())?,
    };
    let launch = launcher::prepare_terminal_launch(
        &store,
        &profile,
        &workspace,
        &session.resume_args(),
    )
    .map_err(|e| e.to_string())?;
    let warning =
        match launcher::open_in_system_terminal(&launch.script_path, &workspace, &launch.env) {
            Ok(()) => None,
            Err(e) => Some(e.to_string()),
        };
    Ok(LaunchOut {
        runtime_id: launch.runtime_id,
        script_path: launch.script_path.to_string_lossy().into_owned(),
        warning,
    })
}

/// App/environment status for the UI header (both CLIs checked).
#[tauri::command]
pub async fn get_status() -> Result<StatusOut, String> {
    let codex_found = codex::find_codex().is_ok();
    let codex_version = codex::codex_version();
    let claude_found = claude::find_claude().is_ok();
    let claude_version = claude::claude_version();
    let config_dir = config_root().to_string_lossy().into_owned();
    let profiles_count = ProfileStore::new()
        .list()
        .map(|v| v.len() as u64)
        .unwrap_or(0);
    Ok(StatusOut {
        codex_found,
        codex_version,
        claude_found,
        claude_version,
        config_dir,
        profiles_count,
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
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
