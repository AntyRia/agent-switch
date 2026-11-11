use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::profile::Profile;
use crate::validation;

/// Configuration root: `$AGENT_SWITCH_HOME` when set (tests/sandboxing),
/// else `%APPDATA%/agent-switch` on Windows and `$HOME/.agent-switch` elsewhere.
pub fn config_root() -> PathBuf {
    if let Ok(custom) = std::env::var("AGENT_SWITCH_HOME") {
        if !custom.is_empty() {
            return PathBuf::from(custom);
        }
    }
    if cfg!(windows) {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("agent-switch")
    } else {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".agent-switch")
    }
}

/// File-backed profile store: one TOML file per profile in `profiles/`.
#[derive(Clone)]
pub struct ProfileStore {
    pub root: PathBuf,
}

impl ProfileStore {
    pub fn new() -> Self {
        Self {
            root: config_root(),
        }
    }

    /// Create the directory layout (idempotent): profiles/ and rt/.
    /// (settings.toml is created on demand by the settings module.)
    pub fn init(&self) -> Result<()> {
        fs::create_dir_all(self.profiles_dir())?;
        self.ensure_runtime_root()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&self.profiles_dir(), fs::Permissions::from_mode(0o700))?;
            fs::set_permissions(&self.runtime_dir(), fs::Permissions::from_mode(0o700))?;
        }
        Ok(())
    }

    pub fn profiles_dir(&self) -> PathBuf {
        self.root.join("profiles")
    }

    /// Root of the per-profile isolated homes.
    ///
    /// Honours the user's pool path: `Settings.pool_dir` when set (any
    /// directory on this machine), else the default `<root>/rt`.
    ///
    /// The default name `rt` was chosen in 0.4.10 (from `runtime`): Codex's
    /// background app-server daemon (≥ v0.166) binds an AF_UNIX socket
    /// inside `CODEX_HOME`, and Windows only accepts socket paths < 108
    /// bytes (`SUN_LEN`). The 0.4.9 layout (`runtime/<36-char-uuid>/.codex/
    /// ...`) pushed the socket path past that limit, so the daemon failed
    /// to start. Every byte saved in the root widens the budget — see
    /// `runtime::compact_home_name`.
    pub fn runtime_dir(&self) -> PathBuf {
        let custom = self.settings().pool_dir.trim().to_string();
        if custom.is_empty() {
            self.root.join("rt")
        } else {
            PathBuf::from(custom)
        }
    }

    /// The default pool root (`<root>/rt`).
    pub fn default_runtime_dir(&self) -> PathBuf {
        self.root.join("rt")
    }

    /// The settings file under this store's root — the same file
    /// `Settings::path()` addresses when the store root is the real config
    /// root (a temp-rooted test store simply has no settings file).
    pub fn settings(&self) -> crate::settings::Settings {
        std::fs::read_to_string(self.root.join("settings.toml"))
            .ok()
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Every root that may hold per-profile homes, active root first: the
    /// configured/default pool dir plus the 0.4.9 `runtime/` directory
    /// while it still carries entries that could not be migrated (a home a
    /// still-running session holds cannot be renamed on Windows).
    pub fn pool_roots(&self) -> Vec<PathBuf> {
        let primary = self.runtime_dir();
        let mut roots = vec![primary.clone()];
        let legacy = self.root.join("runtime");
        if legacy.is_dir() {
            let dup = primary == legacy
                || std::fs::canonicalize(&legacy)
                    .ok()
                    .zip(std::fs::canonicalize(&primary).ok())
                    .is_some_and(|(a, b)| a == b);
            if !dup {
                roots.push(legacy);
            }
        }
        roots
    }

    /// Ensure `rt/` exists; on first use, migrate a 0.4.9 install's
    /// `runtime/` directory into `rt/` (entries are MOVED, never deleted).
    /// Idempotent and cheap once the legacy dir is gone.
    ///
    /// The entry moves are best effort: a home that a still-running
    /// session holds (a terminal left open, a background daemon, a file
    /// the session is writing) cannot be renamed on Windows (access
    /// denied) — the current launch must still proceed, and the entry
    /// migrates on a later start once the handle is gone. A failed move
    /// never blocks `create_runtime`, which runs this on every launch.
    pub fn ensure_runtime_root(&self) -> Result<()> {
        let rt = self.runtime_dir();
        fs::create_dir_all(&rt)?;
        let legacy = self.root.join("runtime");
        if legacy.is_dir() {
            // Collect first: moving an entry invalidates a live read_dir
            // iterator on Windows (FindNextFile may skip or stop early).
            let mut pending = Vec::new();
            if let Ok(entries) = fs::read_dir(&legacy) {
                for entry in entries.flatten() {
                    pending.push((entry.file_name(), entry.path()));
                }
            }
            for (name, from) in pending {
                let to = rt.join(&name);
                if !to.exists() {
                    match fs::rename(&from, &to) {
                        Ok(()) => crate::runtime::repair_codex_daemon_link(&to),
                        Err(e) => crate::logging::warn(&format!(
                            "could not migrate {} to {} (in use? it will migrate on a later start): {e}",
                            from.display(),
                            to.display()
                        )),
                    }
                } else if from.is_dir() && to.is_dir() {
                    if let Err(e) = Self::merge_dir(&from, &to) {
                        crate::logging::warn(&format!(
                            "could not merge {} into {}: {e}",
                            from.display(),
                            to.display()
                        ));
                    }
                }
            }
            let empty = fs::read_dir(&legacy).map(|mut d| d.next().is_none()).unwrap_or(false);
            if empty {
                let _ = fs::remove_dir(&legacy);
            }
        }
        Ok(())
    }

    /// Move `from`'s children into `into`. Children whose name already
    /// exists in `into` are merged recursively (both dirs) or renamed to
    /// `<name>.legacy` (files), so a half-migrated layout never loses data.
    /// `from` is removed when it becomes empty.
    pub(crate) fn merge_dir(from: &Path, into: &Path) -> Result<()> {
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            let src = entry.path();
            let name = entry.file_name();
            let dst = into.join(&name);
            if !dst.exists() {
                fs::rename(&src, &dst)?;
            } else if src.is_dir() && dst.is_dir() {
                Self::merge_dir(&src, &dst)?;
            } else {
                let mut i = 1u32;
                let mut renamed =
                    into.join(format!("{}.legacy{}", name.to_string_lossy(), i));
                while renamed.exists() {
                    i += 1;
                    renamed = into.join(format!("{}.legacy{}", name.to_string_lossy(), i));
                }
                fs::rename(&src, renamed)?;
            }
        }
        if fs::read_dir(from)?.next().is_none() {
            fs::remove_dir(from)?;
        }
        Ok(())
    }

    pub fn path_for(&self, id: &str) -> PathBuf {
        self.profiles_dir().join(format!("{id}.toml"))
    }

    /// All parseable profiles, sorted by id. Unparseable files are skipped
    /// with a warning instead of crashing the whole list.
    pub fn list(&self) -> Result<Vec<Profile>> {
        let dir = self.profiles_dir();
        let mut profiles = Vec::new();
        if !dir.exists() {
            return Ok(profiles);
        }
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("toml") {
                continue;
            }
            match fs::read_to_string(&path)
                .ok()
                .and_then(|s| toml::from_str::<Profile>(&s).ok())
            {
                Some(mut p) => {
                    p.provider.normalize_provider_type();
                    profiles.push(p)
                }
                None => eprintln!(
                    "warning: skipping unparseable profile file: {}",
                    path.display()
                ),
            }
        }
        profiles.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(profiles)
    }

    pub fn get(&self, id: &str) -> Result<Profile> {
        let path = self.path_for(id);
        if !path.exists() {
            return Err(Error::ProfileNotFound(id.to_string()));
        }
        let content = fs::read_to_string(&path)?;
        let mut p: Profile = toml::from_str(&content).map_err(Error::Toml)?;
        p.provider.normalize_provider_type();
        Ok(p)
    }

    /// Validate and write the profile to `<id>.toml` (atomic: temp + rename).
    /// If `old_id` is given and differs from the profile id, the old file is
    /// removed (id rename).
    pub fn save(&self, profile: &Profile, old_id: Option<&str>) -> Result<PathBuf> {
        // Legacy provider types (e.g. "vllm") are folded into the current
        // values so the file on disk is always up to date.
        let mut profile = profile.clone();
        profile.provider.normalize_provider_type();
        let errors = validation::validate_profile(&profile);
        if !errors.is_empty() {
            return Err(Error::Validation(errors.join("; ")));
        }
        self.init()?;
        if let Some(old) = old_id {
            if !old.is_empty() && old != profile.id {
                let old_path = self.path_for(old);
                if old_path.exists() {
                    fs::remove_file(&old_path)?;
                }
            }
        }
        let path = self.path_for(&profile.id);
        let tmp = path.with_extension("toml.tmp");
        fs::write(&tmp, profile.to_toml())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600))?;
        }
        fs::rename(&tmp, &path)?;
        Ok(path)
    }

    pub fn delete(&self, id: &str) -> Result<()> {
        let path = self.path_for(id);
        if !path.exists() {
            return Err(Error::ProfileNotFound(id.to_string()));
        }
        fs::remove_file(&path)?;
        Ok(())
    }
}

impl Default for ProfileStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::{CodexConfig, ModelConfig, ProviderConfig};

    fn profile(id: &str) -> Profile {
        Profile {
            id: id.into(),
            name: format!("Test {id}"),
            description: String::new(),
            provider: ProviderConfig {
                provider_type: "openai-compatible".into(),
                base_url: "http://127.0.0.1:8000/v1".into(),
                api_key: Some("EMPTY".into()),
                api_key_env: None,
                auth_mode: None,
            },
            model: ModelConfig {
                default: "m".into(),
                effort: None,
                context_window: None,
                models: Vec::new(),
            },
            codex: CodexConfig {
                provider_name: id.into(),
            },
            cli: String::new(),
        }
    }

    fn temp_store() -> (ProfileStore, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "as-store-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        (ProfileStore { root: dir.clone() }, dir)
    }

    #[test]
    fn save_list_get_delete_roundtrip() {
        let (store, dir) = temp_store();
        store.init().unwrap();
        let p = profile("local-vllm");
        store.save(&p, None).unwrap();
        assert!(store.path_for("local-vllm").exists());

        let list = store.list().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "local-vllm");

        let got = store.get("local-vllm").unwrap();
        assert_eq!(got.name, p.name);
        assert_eq!(got.model.default, "m");
        assert_eq!(got.provider.api_key.as_deref(), Some("EMPTY"));

        store.delete("local-vllm").unwrap();
        assert!(!store.path_for("local-vllm").exists());
        assert!(store.list().unwrap().is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_rejects_invalid_profiles() {
        let (store, dir) = temp_store();
        let mut p = profile("Bad ID");
        assert!(store.save(&p, None).is_err());
        p.id = "ok-id".into();
        p.provider.base_url = "ftp://x".into();
        assert!(store.save(&p, None).is_err());
        p.provider.base_url = "http://127.0.0.1/v1".into();
        assert!(store.save(&p, None).is_ok());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_renames_old_file() {
        let (store, dir) = temp_store();
        let mut p = profile("old-id");
        store.save(&p, None).unwrap();
        p.id = "new-id".into();
        p.codex.provider_name = "new-id".into();
        store.save(&p, Some("old-id")).unwrap();
        assert!(!store.path_for("old-id").exists());
        assert!(store.path_for("new-id").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn ensure_runtime_root_migrates_legacy_runtime_dir() {
        let (store, dir) = temp_store();
        // 0.4.9 layout: homes under root/runtime/.
        let legacy = dir.join("runtime").join("abc").join(".codex");
        fs::create_dir_all(&legacy).unwrap();
        fs::write(legacy.join("config.toml"), "data").unwrap();

        store.ensure_runtime_root().unwrap();
        assert!(store.runtime_dir().join("abc/.codex/config.toml").exists());
        assert!(!dir.join("runtime").exists(), "legacy dir must be removed");
        // Idempotent: a second call must be a no-op.
        store.ensure_runtime_root().unwrap();
        assert!(store.runtime_dir().join("abc/.codex/config.toml").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn ensure_runtime_root_merges_when_both_sides_exist() {
        let (store, dir) = temp_store();
        let legacy = dir.join("runtime").join("abc");
        let fresh = store.runtime_dir().join("abc");
        fs::create_dir_all(&legacy).unwrap();
        fs::create_dir_all(&fresh).unwrap();
        fs::write(legacy.join("old-file"), "old").unwrap();
        fs::write(fresh.join("new-file"), "new").unwrap();
        // Same name on both sides: kept (never clobbered), source renamed.
        fs::write(legacy.join("same"), "old").unwrap();
        fs::write(fresh.join("same"), "new").unwrap();

        store.ensure_runtime_root().unwrap();
        let merged = store.runtime_dir().join("abc");
        assert!(merged.join("old-file").exists());
        assert!(merged.join("new-file").exists());
        assert_eq!(fs::read_to_string(merged.join("same")).unwrap(), "new");
        assert!(merged.join("same.legacy1").exists());
        assert!(!dir.join("runtime").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn ensure_runtime_root_moves_mixed_entries_and_removes_empty_legacy_root() {
        let (store, dir) = temp_store();
        let legacy = dir.join("runtime");
        fs::create_dir_all(legacy.join("dir-a").join(".codex")).unwrap();
        fs::write(legacy.join("dir-a/config.toml"), "old").unwrap();
        // A same-named dir already on the rt side: children are merged,
        // never clobbered.
        fs::create_dir_all(legacy.join("dir-b")).unwrap();
        fs::write(legacy.join("dir-b/old-file"), "old").unwrap();
        let compact = store.runtime_dir().join("dir-b");
        fs::create_dir_all(&compact).unwrap();
        fs::write(compact.join("new-file"), "new").unwrap();
        fs::write(legacy.join("stray-file"), "x").unwrap();

        store.ensure_runtime_root().unwrap();
        assert!(store.runtime_dir().join("dir-a/config.toml").exists());
        assert!(store.runtime_dir().join("dir-a/.codex").is_dir());
        assert!(compact.join("old-file").exists());
        assert!(compact.join("new-file").exists());
        assert!(store.runtime_dir().join("stray-file").exists());
        // Every entry moved → the legacy root is removed.
        assert!(!dir.join("runtime").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    #[cfg(windows)]
    fn ensure_runtime_root_survives_a_locked_legacy_entry() {
        // A 0.4.9 home whose files are still open (a session running in
        // it) cannot be renamed on Windows: the move must be skipped
        // with a warning, not abort the launch.
        let (store, dir) = temp_store();
        let legacy = dir.join("runtime");
        let locked = legacy.join("locked");
        fs::create_dir_all(&locked).unwrap();
        let held = locked.join("held.txt");
        fs::write(&held, "x").unwrap();
        let free = legacy.join("free");
        fs::create_dir_all(&free).unwrap();
        let _held_file = std::fs::File::open(&held).unwrap(); // no share-delete

        store.ensure_runtime_root().unwrap();
        assert!(store.runtime_dir().join("free").is_dir(), "unlocked entry migrates");
        assert!(locked.exists(), "locked entry stays for a later attempt");
        assert!(dir.join("runtime").exists(), "legacy root kept while entries remain");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn pool_roots_add_the_legacy_runtime_dir_and_honour_pool_dir() {
        let (store, dir) = temp_store();
        fs::create_dir_all(dir.join("runtime")).unwrap();
        assert_eq!(
            store.pool_roots(),
            vec![dir.join("rt"), dir.join("runtime")]
        );
        // A configured pool path becomes the primary root; the legacy dir
        // (still holding entries a running session kept locked) is still
        // reported, so its sessions stay visible.
        fs::write(dir.join("settings.toml"), "pool_dir = \"D:\\\\pool\"\n").unwrap();
        assert_eq!(
            store.pool_roots(),
            vec![PathBuf::from("D:\\pool"), dir.join("runtime")]
        );
        assert_eq!(store.default_runtime_dir(), dir.join("rt"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn delete_missing_errors() {
        let (store, dir) = temp_store();
        assert!(matches!(
            store.delete("nope"),
            Err(Error::ProfileNotFound(_))
        ));
        let _ = fs::remove_dir_all(&dir);
    }
}
