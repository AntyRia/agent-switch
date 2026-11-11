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
    } else {
        // The move rewrites the daemon's absolute `current` link — point it
        // back at the release the home actually holds.
        repair_codex_daemon_link(&compact);
    }
}

/// Repair the Codex daemon's `current` link after a profile home has moved.
///
/// Codex (≥ v0.166) installs its background app-server daemon into
/// `.codex/packages/app-server-daemon/` and points the `current`
/// directory junction at the active `releases/<version>` dir — but it
/// stores the junction target as an ABSOLUTE path. Every time we move a
/// home (0.4.9 `runtime/` → `rt/`, full-id → compact name) the junction
/// dangles and Codex refuses to start with "daemon executable not found
/// at …\packages\app-server-daemon\current\bin\codex.exe".
///
/// Re-point the link at the release the home holds: the version recorded in
/// `auto-update-version` when present, else the newest `releases/` entry.
/// Runs on every Codex launch and after every home migration, so a dangling
/// absolute link is always repaired before the daemon starts.
/// Best effort — a failure here must never block the launch (Codex can
/// reinstall the daemon via `codex app-server daemon start`).
pub(crate) fn repair_codex_daemon_link(dir: &Path) {
    let pkg = dir.join(".codex/packages/app-server-daemon");
    let Some(release) = pick_codex_daemon_release(&pkg) else {
        return;
    };
    let current = pkg.join("current");
    let md = fs::symlink_metadata(&current).ok();
    let broken = match &md {
        // A junction/symlink that no longer resolves: an absolute target
        // outlived a home move.
        Some(m) if m.file_type().is_symlink() => fs::metadata(&current).is_err(),
        // A real dir (a copy) is already resolvable: leave alone. A plain
        // file at `current` is never what Codex expects: replace it.
        Some(m) => !m.file_type().is_dir(),
        // Missing: create it.
        None => true,
    };
    if !broken {
        return;
    }
    if let Some(m) = &md {
        // `symlink_metadata` reports a directory junction as a symlink, not
        // a dir — and only `remove_dir` removes it (remove_file gets access
        // denied). remove_dir on a (dangling) junction does not follow it.
        #[cfg(windows)]
        let as_dir = m.is_dir() || m.file_type().is_symlink();
        #[cfg(not(windows))]
        let as_dir = m.is_dir();
        let _ = if as_dir {
            fs::remove_dir(&current)
        } else {
            fs::remove_file(&current)
        };
    }
    // Windows: an absolute junction, the same layout Codex and `mklink /J`
    // write (the kernel resolves the mount point from the substitute name
    // alone — a relative target stores fine but never resolves). The
    // launch-time repair above keeps it pointed after any home move.
    // Unix: a relative symlink, which survives moves for free.
    let made = (|| -> std::io::Result<()> {
        #[cfg(windows)]
        create_dir_junction(&current, &pkg.join("releases").join(&release))?;
        #[cfg(not(windows))]
        std::os::unix::fs::symlink(
            std::path::Path::new(&format!("releases/{release}")),
            &current,
        )?;
        Ok(())
    })();
    if let Err(e) = made {
        crate::logging::warn(&format!(
            "could not repair the codex daemon link {} -> releases/{release}: {e}",
            current.display()
        ));
    }
}

/// Pick the daemon release to point `current` at: the version recorded in
/// `auto-update-version` (what the daemon's auto-updater last installed)
/// when it still exists, else the newest `releases/<name>` directory.
fn pick_codex_daemon_release(pkg: &Path) -> Option<String> {
    let releases = pkg.join("releases");
    let mut names: Vec<String> = fs::read_dir(&releases)
        .ok()?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !n.starts_with('.'))
        .filter(|n| releases.join(n).is_dir())
        .collect();
    if names.is_empty() {
        return None;
    }
    if let Ok(wanted) = fs::read_to_string(pkg.join("auto-update-version")) {
        let wanted = wanted.trim().to_string();
        if names.iter().any(|n| n == &wanted) {
            return Some(wanted);
        }
    }
    names.sort();
    names.pop()
}

/// Create a directory JUNCTION at `path` pointing at the ABSOLUTE `target`
/// (no reparse-point privileges needed, unlike directory symlinks; the
/// layout is exactly what `mklink /J` and Codex itself write: the
/// substitute name carries `\??\<absolute target>` and the print name the
/// same path for display). Probed on Windows: the kernel resolves a mount
/// point from the substitute name alone, and a relative substitute name
/// stores fine but never resolves.
#[cfg(windows)]
fn create_dir_junction(path: &Path, target: &Path) -> std::io::Result<()> {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    };
    use windows_sys::Win32::System::IO::DeviceIoControl;
    use windows_sys::Win32::System::Ioctl::FSCTL_SET_REPARSE_POINT;
    use windows_sys::Win32::System::SystemServices::IO_REPARSE_TAG_MOUNT_POINT;

    std::fs::create_dir_all(path)?;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)?;

    // REPARSE_DATA_BUFFER (mount point): [ReparseTag u32][ReparseDataLength
    // u16][Reserved u16][MountPointReparseBuffer] where the buffer holds
    // [SubstOff u16][SubstLen u16][RelOff u16][RelLen u16][PathBuffer] and
    // the offsets are relative to PathBuffer. The layout is the same one
    // `mklink /J` writes (see `fsutil reparsepoint query`); the name
    // lengths exclude the null terminators.
    //
    // The substitute name is the NT device path of the absolute target
    // ("\\?\" + target, no trailing backslash) — that is what the kernel
    // actually resolves. The print name (reparse name) is the same
    // absolute path, for display in Explorer.
    let null = [0u8; 2];
    // The "\?\" kernel namespace does not normalize slashes: the target
    // string must carry pure backslashes.
    let target = target.to_string_lossy().replace('/', "\\");
    let subst: Vec<u8> = format!("\\??\\{target}")
        .encode_utf16()
        .flat_map(|u| u.to_le_bytes())
        .collect();
    let rel: Vec<u8> = target.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    let data_len = 8usize + subst.len() + null.len() + rel.len() + null.len();
    let mut data: Vec<u8> = Vec::with_capacity(8 + data_len);
    data.extend_from_slice(&IO_REPARSE_TAG_MOUNT_POINT.to_ne_bytes());
    data.extend_from_slice(&(data_len as u16).to_ne_bytes());
    data.extend_from_slice(&0u16.to_ne_bytes()); // Reserved
    data.extend_from_slice(&0u16.to_ne_bytes()); // SubstituteNameOffset
    data.extend_from_slice(&(subst.len() as u16).to_ne_bytes());
    data.extend_from_slice(&((subst.len() + null.len()) as u16).to_ne_bytes()); // RelOff
    data.extend_from_slice(&(rel.len() as u16).to_ne_bytes());
    data.extend_from_slice(&subst);
    data.extend_from_slice(&null);
    data.extend_from_slice(&rel);
    data.extend_from_slice(&null);

    let mut bytes_returned: u32 = 0;
    let rc = unsafe {
        DeviceIoControl(
            file.as_raw_handle() as _,
            FSCTL_SET_REPARSE_POINT,
            data.as_ptr().cast(),
            data.len() as u32,
            std::ptr::null_mut(),
            0,
            &mut bytes_returned,
            std::ptr::null_mut(),
        )
    };
    if rc == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Prepare ONE isolated home dir for a launch. Codex's generated
/// `config.toml` is rewritten from the profile on every call so the profile
/// stays the single source of routing truth; the `[projects]` trust table
/// Codex writes into that file is carried over (and the launch `workspace`
/// is marked trusted — see `codex::with_project_trust`), and so is the
/// user's TUI model-effort choice
/// (`codex::with_model_reasoning_effort`). Claude needs no config file — its
/// base URL, model and key travel as per-process environment variables at
/// launch (spec §9).
///
/// Unlike `create_runtime`, the dir is NOT derived from the profile's
/// current compact home — callers may pass a different home (a 0.4.9
/// `runtime/` leftover dir, or a pre-0.4.10 as-is dir name) when resuming a
/// session whose transcript lives there, so the CLI finds the conversation.
pub fn prepare_home_for_launch(
    dir: &Path,
    engine: Engine,
    profile: &Profile,
    workspace: &Path,
) -> Result<Runtime> {
    let home = dir.join(engine.home_dir_name());
    fs::create_dir_all(&home)?;
    if matches!(engine, Engine::Codex) {
        // A home moved by an older version (or by hand) may carry a
        // dangling daemon link even though nothing moves this launch.
        repair_codex_daemon_link(dir);
        // The model catalog gives the profile model real metadata (context
        // window, no reasoning params) and replaces the built-in OpenAI
        // model list — see codex::render_model_catalog.
        let catalog_path = home.join("model-catalog.json");
        let config_path = home.join("config.toml");
        let existing = fs::read_to_string(&config_path).ok();
        let config = crate::codex::generate_codex_config(profile, &catalog_path);
        let config = crate::codex::with_project_trust(config, existing.as_deref(), workspace);
        // The user's TUI effort choice survives the rewrite (see the
        // helper).
        let config = crate::codex::with_model_reasoning_effort(config, existing.as_deref());
        fs::write(&config_path, config)?;
        fs::write(&catalog_path, crate::codex::render_model_catalog(profile))?;
    } else {
        // Base pre-seed (theme + onboarding); the launch paths re-seed with
        // the workspace for the per-project trust entry. Best effort: a
        // seeding failure must not block the launch.
        let _ = seed_claude_home(&home, None);
    }
    Ok(Runtime {
        dir: dir.to_path_buf(),
        engine,
        home,
    })
}

/// Get (creating if needed) the profile's persistent isolated home.
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
    prepare_home_for_launch(&dir, engine, profile, workspace)
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
    fn prepare_home_for_launch_preps_any_dir_for_codex() {
        let dir = std::env::temp_dir().join(format!(
            "as-prep-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let ws = dir.join("ws");
        std::fs::create_dir_all(&ws).unwrap();
        let profile = Profile {
            id: "gpt".into(),
            name: "GPT".into(),
            description: String::new(),
            provider: crate::profile::ProviderConfig {
                provider_type: "openai-compatible".into(),
                base_url: "https://example.com/v1".into(),
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
                provider_name: "gpt".into(),
            },
            cli: "codex".into(),
        };
        // A home dir that is NOT the profile's current one (a 0.4.9
        // leftover) — the resume path prepares it instead.
        let legacy = dir.join("legacy-home");
        // A pre-existing effort choice must survive the regeneration.
        std::fs::create_dir_all(legacy.join(".codex")).unwrap();
        std::fs::write(
            legacy.join(".codex").join("config.toml"),
            "model_reasoning_effort = \"medium\"\n[model_providers.gpt]\nbase_url = \"http://stale.example/v1\"\n",
        )
        .unwrap();

        let rt = prepare_home_for_launch(&legacy, Engine::Codex, &profile, &ws).unwrap();
        assert_eq!(rt.dir, legacy);
        assert_eq!(rt.home, legacy.join(".codex"));
        let config = std::fs::read_to_string(rt.home.join("config.toml")).unwrap();
        assert!(
            config.contains("model_providers.gpt"),
            "config must be regenerated in the legacy home: {config}"
        );
        assert!(config.contains("https://example.com/v1"));
        assert!(
            config.contains("model_reasoning_effort = \"medium\""),
            "the user's effort choice must survive the rewrite: {config}"
        );
        assert!(rt.home.join("model-catalog.json").exists());
        let _ = std::fs::remove_dir_all(&dir);
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

    /// Scratch home with one installed daemon release; returns the home.
    fn daemon_home_with(base: &Path, release: &str, content: &str) -> PathBuf {
        let bin = base
            .join(".codex/packages/app-server-daemon/releases")
            .join(release)
            .join("bin");
        fs::create_dir_all(&bin).unwrap();
        fs::write(bin.join("codex.exe"), content).unwrap();
        base.to_path_buf()
    }

    #[test]
    fn daemon_link_missing_is_created_and_repaired_after_a_home_move() {
        let dir = std::env::temp_dir().join(format!(
            "as-daemon-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        // The home is a subdir of `dir` so it can be renamed onto a sibling.
        let home = daemon_home_with(&dir.join("home"), "0.166.0-x86_64-pc-windows-msvc", "daemon");
        repair_codex_daemon_link(&home);
        let cur = home.join(".codex/packages/app-server-daemon/current");
        assert!(
            fs::metadata(cur.join("bin/codex.exe")).is_ok(),
            "created link must resolve"
        );
        // The link is an absolute junction (as Codex writes it), so a home
        // move (0.4.9 → rt/, full-id → compact) dangles it — and the
        // launch-time repair must re-point it.
        let moved = dir.join("moved");
        fs::rename(&home, &moved).unwrap();
        let moved_cur = moved.join(".codex/packages/app-server-daemon/current");
        // Windows stores an absolute junction (dangles across the move);
        // Unix stores a relative symlink (survives it — the repair below
        // is a no-op there).
        #[cfg(windows)]
        assert!(
            fs::metadata(moved_cur.join("bin/codex.exe")).is_err(),
            "absolute link must dangle after the home moved"
        );
        repair_codex_daemon_link(&moved);
        assert!(
            fs::metadata(moved_cur.join("bin/codex.exe")).is_ok(),
            "launch-time repair must re-point the dangling link"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn daemon_link_dangling_is_repointed() {
        let dir = std::env::temp_dir().join(format!(
            "as-daemon-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        // The home is a subdir of `dir` so it can be renamed onto a sibling.
        let home = daemon_home_with(&dir.join("home"), "0.166.0-x86_64-pc-windows-msvc", "daemon");
        let cur = home.join(".codex/packages/app-server-daemon/current");
        // A dangling link: what an absolute junction looks like after the
        // home moved. (Directory symlinks need no privileges on Unix, so
        // on Windows we use a dangling junction instead.)
        #[cfg(windows)]
        create_dir_junction(&cur, &home.join("gone")).unwrap();
        #[cfg(not(windows))]
        std::os::unix::fs::symlink(&dir.join("gone.txt"), &cur).unwrap();
        repair_codex_daemon_link(&home);
        assert!(
            fs::metadata(cur.join("bin/codex.exe")).is_ok(),
            "dangling link must be repaired"
        );
        // And the repaired link must be re-pointable after yet another move.
        let moved = dir.join("moved");
        fs::rename(&home, &moved).unwrap();
        repair_codex_daemon_link(&moved);
        assert!(
            fs::metadata(moved.join(".codex/packages/app-server-daemon/current/bin/codex.exe"))
                .is_ok(),
            "repaired link must resolve again after the next move"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn daemon_link_prefers_the_recorded_auto_update_version() {
        let dir = std::env::temp_dir().join(format!(
            "as-daemon-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let pkg = dir.join(".codex/packages/app-server-daemon");
        let _ = daemon_home_with(&dir, "0.166.0-x86_64-pc-windows-msvc", "old");
        let _ = daemon_home_with(&dir, "0.167.1-x86_64-pc-windows-msvc", "new");
        // The auto-updater last installed 0.166.0 — the link must point
        // there, not at the lexicographically newest release.
        fs::write(pkg.join("auto-update-version"), "0.166.0-x86_64-pc-windows-msvc")
            .unwrap();
        repair_codex_daemon_link(&dir);
        let body = fs::read_to_string(
            dir.join(".codex/packages/app-server-daemon/current/bin/codex.exe"),
        )
        .unwrap();
        assert_eq!(body, "old");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn daemon_link_noop_without_releases() {
        let dir = std::env::temp_dir().join(format!(
            "as-daemon-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(dir.join(".codex")).unwrap();
        repair_codex_daemon_link(&dir);
        assert!(!dir.join(".codex/packages/app-server-daemon/current").exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
