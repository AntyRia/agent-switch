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
    /// Renamed `runtime` → `rt` in 0.4.10: Codex's background app-server
    /// daemon (≥ v0.166) binds an AF_UNIX socket inside `CODEX_HOME`, and
    /// Windows only accepts socket paths < 108 bytes (`SUN_LEN`). The 0.4.9
    /// layout (`runtime/<36-char-uuid>/.codex/...`) pushed the socket path
    /// past that limit, so the daemon failed to start. Every byte saved in
    /// the root widens the budget — see `runtime::compact_home_name`.
    pub fn runtime_dir(&self) -> PathBuf {
        self.root.join("rt")
    }

    /// Ensure `rt/` exists; on first use, migrate a 0.4.9 install's
    /// `runtime/` directory into `rt/` (entries are MOVED, never deleted).
    /// Idempotent and cheap once the legacy dir is gone.
    pub fn ensure_runtime_root(&self) -> Result<()> {
        let rt = self.runtime_dir();
        fs::create_dir_all(&rt)?;
        let legacy = self.root.join("runtime");
        if legacy.is_dir() {
            for entry in fs::read_dir(&legacy)? {
                let entry = entry?;
                let from = entry.path();
                let to = rt.join(entry.file_name());
                if !to.exists() {
                    fs::rename(&from, &to)?;
                } else if from.is_dir() && to.is_dir() {
                    Self::merge_dir(&from, &to)?;
                }
            }
            if fs::read_dir(&legacy)?.next().is_none() {
                fs::remove_dir(&legacy)?;
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
    fn delete_missing_errors() {
        let (store, dir) = temp_store();
        assert!(matches!(
            store.delete("nope"),
            Err(Error::ProfileNotFound(_))
        ));
        let _ = fs::remove_dir_all(&dir);
    }
}
