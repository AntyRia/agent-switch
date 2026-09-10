use std::fs;
use std::path::PathBuf;

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
pub struct ProfileStore {
    pub root: PathBuf,
}

impl ProfileStore {
    pub fn new() -> Self {
        Self {
            root: config_root(),
        }
    }

    /// Create the directory layout (idempotent): profiles/, runtime/ and the
    /// spec's root settings.toml placeholder.
    pub fn init(&self) -> Result<()> {
        fs::create_dir_all(self.profiles_dir())?;
        fs::create_dir_all(self.runtime_dir())?;
        let settings = self.root.join("settings.toml");
        if !settings.exists() {
            fs::write(&settings, "# agent-switch settings (reserved; no settings are read yet)\n")?;
        }
        Ok(())
    }

    pub fn profiles_dir(&self) -> PathBuf {
        self.root.join("profiles")
    }

    pub fn runtime_dir(&self) -> PathBuf {
        self.root.join("runtime")
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
                Some(p) => profiles.push(p),
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
        toml::from_str(&content).map_err(Error::Toml)
    }

    /// Validate and write the profile to `<id>.toml` (atomic: temp + rename).
    /// If `old_id` is given and differs from the profile id, the old file is
    /// removed (id rename).
    pub fn save(&self, profile: &Profile, old_id: Option<&str>) -> Result<PathBuf> {
        let errors = validation::validate_profile(profile);
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
    fn delete_missing_errors() {
        let (store, dir) = temp_store();
        assert!(matches!(
            store.delete("nope"),
            Err(Error::ProfileNotFound(_))
        ));
        let _ = fs::remove_dir_all(&dir);
    }
}
