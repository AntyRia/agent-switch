use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::engine::Engine;
use crate::error::Result;
use crate::profile::Profile;
use crate::profile_store::ProfileStore;

/// One isolated CLI config home, owned by a single profile.
///
/// The home PERSISTS across launches: conversation history (Claude session
/// transcripts, Codex rollouts) lives here and is therefore resumable via
/// the session pool. It stays fully separate from the user's global
/// `~/.codex` / `~/.claude` — the whole point of the product.
pub struct Runtime {
    /// `<root>/rt/<compact home name>` (see `compact_home_name`).
    pub dir: PathBuf,
    /// Engine the runtime was created for.
    pub engine: Engine,
    /// `<dir>/.codex` (Codex) or `<dir>/.claude` (Claude) — the value for
    /// the CODEX_HOME / CLAUDE_CONFIG_DIR env var.
    pub home: PathBuf,
}

/// On-disk name of a profile's persistent home dir: `<root>/rt/<name>`.
///
/// Already-short ids (≤ 8 chars) are used verbatim; longer ids — the
/// auto-generated 36-char UUIDs — map to an 8-hex-char token: the first 8
/// hex digits of the id for UUIDs, else of sha256(id).
///
/// The bound is load-bearing: Codex's background app-server daemon (≥ v0.166)
/// binds an AF_UNIX socket at `<CODEX_HOME>/app-server-control/
/// app-server-control.sock`, and Windows only accepts socket paths < 108
/// bytes (`SUN_LEN`). With the 0.4.9 layout the socket path was
/// `C:\Users\<u>\AppData\Roaming\agent-switch\runtime\<36-char-uuid>/.codex/
/// app-server-control/app-server-control.sock` = 133+u bytes — already over
/// the limit for EVERY username, so the daemon died with "path must be
/// shorter than SUN_LEN" (138 bytes on a 5-char username). The 0.4.10
/// layout is 100+u bytes: it fits for usernames up to 7 chars, and macOS
/// (limit 104) is unaffected at 83+u bytes.
pub fn compact_home_name(id: &str) -> String {
    if id.len() <= 8 {
        return id.to_string();
    }
    if let Ok(u) = uuid::Uuid::parse_str(id) {
        return u.as_simple().to_string()[..8].to_string();
    }
    use sha2::Digest;
    let digest = sha2::Sha256::digest(id.as_bytes());
    let mut out = String::with_capacity(8);
    for b in digest.iter().take(4) {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// One-time migration of a 0.4.9 per-profile dir: the home used to live at
/// `<root>/runtime/<full-id>` (root segment renamed by
/// `ProfileStore::ensure_runtime_root`). If the legacy dir exists, move its
/// content into the compact dir — session history must survive the rename,
/// `codex resume` / `claude --resume` look inside CODEX_HOME.
fn migrate_legacy_profile_dir(base: &Path, id: &str) {
    let name = compact_home_name(id);
    if name == id {
        return;
    }
    let legacy = base.join(id);
    if !legacy.is_dir() {
        return;
    }
    let compact = base.join(&name);
    let ok = if compact.is_dir() {
        crate::profile_store::ProfileStore::merge_dir(&legacy, &compact).is_ok()
    } else {
        fs::rename(&legacy, &compact).is_ok()
    };
    if !ok {
        crate::logging::warn(&format!(
            "could not migrate legacy runtime dir {} to {}",
            legacy.display(),
            compact.display()
        ));
    }
}

/// Get (creating if needed) the profile's persistent isolated home. Codex's
/// generated `config.toml` is rewritten from the profile on every call so
/// the profile stays the single source of routing truth; the `[projects]`
/// trust table Codex writes into that file is carried over (and the launch
/// `workspace` is marked trusted — see `codex::with_project_trust`).
/// Claude needs no config file — its base URL, model and key travel as
/// per-process environment variables at launch (spec §9).
pub fn create_runtime(
    store: &ProfileStore,
    profile: &Profile,
    workspace: &Path,
) -> Result<Runtime> {
    let engine = profile.engine();
    store.ensure_runtime_root()?;
    let runtime_dir = store.runtime_dir();
    migrate_legacy_profile_dir(&runtime_dir, &profile.id);
    let dir = runtime_dir.join(compact_home_name(&profile.id));
    let home = dir.join(engine.home_dir_name());
    fs::create_dir_all(&home)?;
    if matches!(engine, Engine::Codex) {
        // The model catalog gives the profile model real metadata (context
        // window, no reasoning params) and replaces the built-in OpenAI
        // model list — see codex::render_model_catalog.
        let catalog_path = home.join("model-catalog.json");
        let config_path = home.join("config.toml");
        let existing = fs::read_to_string(&config_path).ok();
        let config = crate::codex::generate_codex_config(profile, &catalog_path);
        let config = crate::codex::with_project_trust(config, existing.as_deref(), workspace);
        fs::write(&config_path, config)?;
        fs::write(&catalog_path, crate::codex::render_model_catalog(profile))?;
    } else {
        // Base pre-seed (theme + onboarding); the launch paths re-seed with
        // the workspace for the per-project trust entry. Best effort: a
        // seeding failure must not block the launch.
        let _ = seed_claude_home(&home, None);
    }
    Ok(Runtime { dir, engine, home })
}

/// Pre-seed a Claude config home so the FIRST launch in a fresh isolated
/// environment does not replay the onboarding prompts (theme picker,
/// per-project "trust this folder" dialog, project onboarding).
///
/// Verified against Claude Code v2.1.267: with `CLAUDE_CONFIG_DIR=<home>`,
/// the state file `.claude.json` lives INSIDE the home dir (not as a
/// sibling of it), and `settings.json` (where /config stores the theme) is
/// created there on first use.
///
/// Merge semantics — never clobber Claude's own state:
/// - `settings.json` is written only when absent (a later /config change
///   rewrites it).
/// - `.claude.json` is read-modify-written, preserving everything Claude
///   wrote itself; if the file exists but is unparseable it is left alone
///   (Claude Code has its own recovery path for that).
pub fn seed_claude_home(home: &Path, workspace: Option<&Path>) -> Result<()> {
    // Theme: skip the TUI theme picker (default to dark).
    let settings = home.join("settings.json");
    if !settings.exists() {
        fs::write(&settings, "{\n  \"theme\": \"dark\"\n}\n")?;
    }
    // Onboarding + per-project trust.
    let state = home.join(".claude.json");
    let existing = fs::read_to_string(&state).ok();
    let mut doc: serde_json::Value = match &existing {
        Some(text) => match serde_json::from_str(text) {
            Ok(v) => v,
            Err(_) => return Ok(()), // corrupt: leave it for Claude's recovery
        },
        None => serde_json::json!({}),
    };
    if let Some(obj) = doc.as_object_mut() {
        obj.insert("hasCompletedOnboarding".into(), serde_json::json!(true));
        if let Some(ws) = workspace {
            let key = project_path_key(ws);
            let projects = obj
                .entry("projects")
                .or_insert_with(|| serde_json::json!({}));
            if let Some(projects) = projects.as_object_mut() {
                let entry = projects
                    .entry(key)
                    .or_insert_with(|| serde_json::json!({}));
                if let Some(entry) = entry.as_object_mut() {
                    entry.insert(
                        "hasTrustDialogAccepted".into(),
                        serde_json::json!(true),
                    );
                    entry
                        .entry("projectOnboardingSeenCount")
                        .or_insert_with(|| serde_json::json!(1));
                }
            }
        }
    }
    let text = serde_json::to_string_pretty(&doc)
        .map_err(|e| crate::error::Error::Other(format!("failed to serialize state: {e}")))?;
    fs::write(&state, text)?;
    Ok(())
}

/// Project keys in `.claude.json` use forward-slash paths with no trailing
/// slash (e.g. `C:/Users/dev/proj` on Windows, `/home/dev/proj` on Unix).
fn project_path_key(ws: &Path) -> String {
    let mut s = ws.to_string_lossy().replace('\\', "/");
    while s.len() > 1 && s.ends_with('/') {
        s.pop();
    }
    s
}

/// Delete runtime dirs older than 7 days, then keep only the newest 20.
/// Returns the deleted paths. Never touches `profiles/`, and never prunes
/// a dir named after an existing profile — in full (≤ 0.4.9) or compact
/// form (those are the persistent per-profile homes — pruning them would
/// destroy conversation history). Only legacy ephemeral `rt/<uuid>` dirs
/// from older versions are pruned.
pub fn cleanup(store: &ProfileStore) -> Vec<PathBuf> {
    let root = store.runtime_dir();
    let home_names: std::collections::HashSet<String> = store
        .list()
        .map(|ps| {
            ps.into_iter()
                .flat_map(|p| {
                    let compact = compact_home_name(&p.id);
                    [p.id, compact]
                })
                .collect()
        })
        .unwrap_or_default();
    let mut deleted = Vec::new();
    let mut dirs: Vec<(PathBuf, SystemTime)> = Vec::new();
    if let Ok(entries) = fs::read_dir(&root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let is_profile_home = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| home_names.contains(n));
                if is_profile_home {
                    continue;
                }
                let mtime = entry
                    .metadata()
                    .and_then(|m| m.modified())
                    .unwrap_or(SystemTime::UNIX_EPOCH);
                dirs.push((path, mtime));
            }
        }
    }
    let now = SystemTime::now();
    const MAX_AGE: Duration = Duration::from_secs(7 * 24 * 3600);
    dirs.retain(|(path, mtime)| {
        let expired = now
            .duration_since(*mtime)
            .map(|age| age > MAX_AGE)
            .unwrap_or(false);
        if expired {
            let _ = fs::remove_dir_all(path);
            deleted.push(path.clone());
            false
        } else {
            true
        }
    });
    // Newest first; drop everything beyond the first 20.
    dirs.sort_by(|a, b| b.1.cmp(&a.1));
    for (path, _) in dirs.into_iter().skip(20) {
        let _ = fs::remove_dir_all(&path);
        deleted.push(path);
    }
    deleted
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;

    /// Open a directory so its timestamps can be modified.
    /// Windows needs FILE_FLAG_BACKUP_SEMANTICS for directory handles.
    fn open_dir_for_time(path: &std::path::Path) -> std::io::Result<File> {
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
            std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
                .open(path)
        }
        #[cfg(not(windows))]
        {
            File::open(path)
        }
    }

    #[test]
    fn cleanup_removes_old_and_keeps_recent() {
        let dir = std::env::temp_dir().join(format!(
            "as-clean-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let store = ProfileStore { root: dir.clone() };
        let rt = store.runtime_dir();
        fs::create_dir_all(&rt).unwrap();
        let mut paths = Vec::new();
        for name in ["r1", "r2", "r3", "old"] {
            let p = rt.join(name);
            fs::create_dir_all(&p).unwrap();
            paths.push(p);
        }
        // Backdate the "old" dir by 8 days.
        let old = open_dir_for_time(&paths[3]).unwrap();
        old.set_modified(SystemTime::now() - Duration::from_secs(8 * 24 * 3600))
            .unwrap();

        let deleted = cleanup(&store);
        assert!(deleted
            .iter()
            .any(|p| p.file_name().map(|n| n == "old").unwrap_or(false)));
        assert!(!paths[3].exists());
        assert!(paths[0].exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cleanup_never_prunes_profile_homes() {
        let dir = std::env::temp_dir().join(format!(
            "as-clean-profile-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let store = ProfileStore { root: dir.clone() };
        let profile = Profile {
            id: "keepme".into(),
            name: "K".into(),
            description: String::new(),
            provider: crate::profile::ProviderConfig {
                provider_type: "relay".into(),
                base_url: "https://example.com".into(),
                api_key: None,
                api_key_env: None,
                auth_mode: None,
            },
            model: crate::profile::ModelConfig {
                default: "m".into(),
                effort: None,
                context_window: None,
                models: Vec::new(),
            },
            codex: crate::profile::CodexConfig {
                provider_name: "keepme".into(),
            },
            cli: "claude".into(),
        };
        store.save(&profile, None).unwrap();
        let home = store.runtime_dir().join("keepme");
        fs::create_dir_all(&home).unwrap();
        // Backdate it beyond the 7-day limit: it must still survive.
        let f = open_dir_for_time(&home).unwrap();
        f.set_modified(SystemTime::now() - Duration::from_secs(8 * 24 * 3600))
            .unwrap();

        let deleted = cleanup(&store);
        assert!(home.exists(), "profile home was pruned");
        assert!(!deleted
            .iter()
            .any(|p| p.file_name().map(|n| n == "keepme").unwrap_or(false)));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn seed_claude_home_skips_onboarding_and_never_writes_key() {
        let dir = std::env::temp_dir().join(format!(
            "as-seed-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let home = dir.join(".claude");
        fs::create_dir_all(&home).unwrap();
        seed_claude_home(&home, Some(Path::new(r"C:\Users\dev\my-proj"))).unwrap();

        let settings: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(home.join("settings.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(settings["theme"], "dark");

        let state: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(home.join(".claude.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(state["hasCompletedOnboarding"], true);
        assert_eq!(
            state["projects"]["C:/Users/dev/my-proj"]["hasTrustDialogAccepted"],
            true
        );
        // A re-seed (next launch) must be idempotent, not additive.
        seed_claude_home(&home, None).unwrap();
        let state: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(home.join(".claude.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(state["hasCompletedOnboarding"], true);
        assert_eq!(
            state["projects"]["C:/Users/dev/my-proj"]["hasTrustDialogAccepted"],
            true
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn seed_claude_home_preserves_existing_state_and_settings() {
        let dir = std::env::temp_dir().join(format!(
            "as-seed-keep-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let home = dir.join(".claude");
        fs::create_dir_all(&home).unwrap();
        // Claude's own state, written by a previous real session.
        fs::write(
            home.join(".claude.json"),
            r#"{"userID":"keep-me","projects":{"C:/old/path":{"hasTrustDialogAccepted":true,"projectOnboardingSeenCount":3}}}"#,
        )
        .unwrap();
        // The user's own theme choice — must not be clobbered.
        fs::write(home.join("settings.json"), "{\"theme\":\"light\"}").unwrap();

        seed_claude_home(&home, Some(Path::new("/home/dev/new-ws/"))).unwrap();

        let state: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(home.join(".claude.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(state["userID"], "keep-me");
        assert_eq!(state["hasCompletedOnboarding"], true);
        // New workspace trusted…
        assert_eq!(
            state["projects"]["/home/dev/new-ws"]["hasTrustDialogAccepted"],
            true
        );
        // …existing project entry preserved intact.
        assert_eq!(
            state["projects"]["C:/old/path"]["projectOnboardingSeenCount"],
            3
        );
        // User settings untouched.
        let settings: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(home.join("settings.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(settings["theme"], "light");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn seed_claude_home_ignores_corrupt_state_file() {
        let dir = std::env::temp_dir().join(format!(
            "as-seed-corrupt-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let home = dir.join(".claude");
        fs::create_dir_all(&home).unwrap();
        fs::write(home.join(".claude.json"), "{not json").unwrap();
        seed_claude_home(&home, None).unwrap();
        // The corrupt file is left for Claude Code's own recovery path.
        assert_eq!(fs::read_to_string(home.join(".claude.json")).unwrap(), "{not json");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn create_runtime_writes_config_without_key() {
        let dir = std::env::temp_dir().join(format!(
            "as-rt-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let store = ProfileStore { root: dir.clone() };
        let profile = Profile {
            id: "t".into(),
            name: "T".into(),
            description: String::new(),
            provider: crate::profile::ProviderConfig {
                provider_type: "openai-compatible".into(),
                base_url: "http://127.0.0.1:8000/v1".into(),
                api_key: Some("sk-never-write-me".into()),
                api_key_env: None,
                auth_mode: None,
            },
            model: crate::profile::ModelConfig {
                default: "m".into(),
                effort: None,
                context_window: None,
                models: Vec::new(),
            },
            codex: crate::profile::CodexConfig {
                provider_name: "t".into(),
            },
            cli: String::new(), // legacy shape → codex
        };
        let rt = create_runtime(&store, &profile, std::path::Path::new(".")).unwrap();
        assert!(matches!(rt.engine, Engine::Codex));
        let config_path = rt.home.join("config.toml");
        assert!(config_path.exists());
        let content = fs::read_to_string(&config_path).unwrap();
        assert!(content.contains("model_provider = \"t\""));
        assert!(!content.contains("sk-never-write-me"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn create_runtime_for_claude_makes_empty_claude_home() {
        let dir = std::env::temp_dir().join(format!(
            "as-rt-claude-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let store = ProfileStore { root: dir.clone() };
        let profile = Profile {
            id: "c".into(),
            name: "C".into(),
            description: String::new(),
            provider: crate::profile::ProviderConfig {
                provider_type: "relay".into(),
                base_url: "https://relay.example.com".into(),
                api_key: Some("sk-never-write-me-either".into()),
                api_key_env: None,
                auth_mode: Some("auth_token".into()),
            },
            model: crate::profile::ModelConfig {
                default: "claude-sonnet-4-5".into(),
                effort: None,
                context_window: None,
                models: Vec::new(),
            },
            codex: crate::profile::CodexConfig {
                provider_name: "c".into(),
            },
            cli: "claude".into(),
        };
        let rt = create_runtime(&store, &profile, std::path::Path::new(".")).unwrap();
        assert!(matches!(rt.engine, Engine::Claude));
        assert!(rt.home.ends_with(".claude"));
        assert!(rt.home.is_dir());
        // Claude needs no config file; the key must not land on disk.
        assert!(!rt.home.join("config.toml").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn compact_home_name_is_bounded_and_stable() {
        // Short ids stay verbatim (no migration needed for them).
        assert_eq!(compact_home_name("t"), "t");
        assert_eq!(compact_home_name("abcdefgh"), "abcdefgh");
        // UUIDs: first 8 hex digits, dashes stripped.
        assert_eq!(
            compact_home_name("5c6479a1-b596-47fc-9525-12b0e6fd20b5"),
            "5c6479a1"
        );
        assert_eq!(
            compact_home_name("ABCD1234-EF00-4000-8000-000000000000"),
            "abcd1234"
        );
        // Other long ids: 8 hex of sha256 — stable, distinct, hex-only.
        let a = compact_home_name("a-very-long-profile-id-that-is-not-a-uuid");
        let b = compact_home_name("a-very-long-profile-id-that-is-not-a-uuid");
        assert_eq!(a, b);
        assert_eq!(a.len(), 8);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(
            a,
            compact_home_name("a-very-long-profile-id-that-is-not-a-uuid-x")
        );
    }

    fn profile_with_id(id: &str) -> Profile {
        Profile {
            id: id.into(),
            name: format!("P {id}"),
            description: String::new(),
            provider: crate::profile::ProviderConfig {
                provider_type: "openai-compatible".into(),
                base_url: "http://127.0.0.1:8000/v1".into(),
                api_key: Some("EMPTY".into()),
                api_key_env: None,
                auth_mode: None,
            },
            model: crate::profile::ModelConfig {
                default: "m".into(),
                effort: None,
                context_window: None,
                models: Vec::new(),
            },
            codex: crate::profile::CodexConfig {
                provider_name: id.into(),
            },
            cli: String::new(),
        }
    }

    #[test]
    fn create_runtime_uses_compact_dir_and_migrates_legacy_full_id() {
        let dir = std::env::temp_dir().join(format!(
            "as-rt-migrate-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let store = ProfileStore { root: dir.clone() };
        let id = "5c6479a1-b596-47fc-9525-12b0e6fd20b5";
        // 0.4.9 layout: the home lived at root/runtime/<full-uuid>/.codex.
        let legacy_home = store.root.join("runtime").join(id).join(".codex");
        let day = legacy_home.join("sessions/2026/09/10");
        fs::create_dir_all(&day).unwrap();
        fs::write(day.join("rollout-old.jsonl"), "data").unwrap();
        fs::write(legacy_home.join("config.toml"), "old config").unwrap();

        let rt = create_runtime(&store, &profile_with_id(id), std::path::Path::new(".")).unwrap();
        // The home moved to rt/<first-8-hex>; the socket-relevant tail is
        // what made the name budget necessary.
        assert_eq!(rt.dir.file_name().unwrap().to_str().unwrap(), "5c6479a1");
        assert!(rt.dir.starts_with(store.runtime_dir()));
        // History survived the rename: resume still finds the rollout.
        assert!(rt.home.join("sessions/2026/09/10/rollout-old.jsonl").exists());
        assert!(!store.root.join("runtime").exists(), "legacy root migrated");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cleanup_never_prunes_compact_profile_homes() {
        let dir = std::env::temp_dir().join(format!(
            "as-clean-compact-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let store = ProfileStore { root: dir.clone() };
        let id = "5c6479a1-b596-47fc-9525-12b0e6fd20b5";
        store.save(&profile_with_id(id), None).unwrap();
        let home = store.runtime_dir().join(compact_home_name(id));
        fs::create_dir_all(&home).unwrap();
        let f = open_dir_for_time(&home).unwrap();
        f.set_modified(SystemTime::now() - Duration::from_secs(8 * 24 * 3600))
            .unwrap();

        let deleted = cleanup(&store);
        assert!(home.exists(), "compact profile home was pruned");
        assert!(!deleted
            .iter()
            .any(|p| p == &home));
        let _ = fs::remove_dir_all(&dir);
    }
}
