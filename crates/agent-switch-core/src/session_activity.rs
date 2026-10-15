//! Session ownership comes from agent arguments and Claude's live registry,
//! never from a terminal host. Locks cover only the short startup race.

use crate::{
    error::{Error, Result},
    profile_store::ProfileStore,
    session_process::{self, Snapshot},
};
use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionActivity {
    Closed,
    Starting,
    Running,
    Background { attach_id: String },
    Unknown,
}

impl SessionActivity {
    pub fn is_open(&self) -> bool {
        !matches!(self, Self::Closed)
    }
    pub fn label(&self) -> &'static str {
        match self {
            Self::Closed => "closed",
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Background { .. } => "background",
            Self::Unknown => "unknown",
        }
    }
}

pub struct SessionActivities {
    states: BTreeMap<String, SessionActivity>,
    complete: bool,
}

impl SessionActivities {
    pub fn get(&self, id: &str) -> SessionActivity {
        self.states.get(id).cloned().unwrap_or(if self.complete {
            SessionActivity::Closed
        } else {
            SessionActivity::Unknown
        })
    }
    pub fn open_ids(&self) -> HashSet<String> {
        self.states
            .iter()
            .filter(|(_, state)| state.is_open())
            .map(|(id, _)| id.clone())
            .collect()
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == b'-')
}

pub fn open_lock_path(store: &ProfileStore, id: &str) -> PathBuf {
    store.root.join("sessions-open").join(format!("{id}.json"))
}

/// Ownership token prevents an old launcher or terminal from removing a lock
/// belonging to a later launch of the same session.
pub struct SessionOpenGuard {
    path: PathBuf,
    token: String,
    retained: bool,
}

impl SessionOpenGuard {
    pub fn acquire(store: &ProfileStore, id: &str) -> Result<Self> {
        if !valid_id(id) {
            return Err(Error::Other("invalid session id".into()));
        }
        let path = open_lock_path(store, id);
        std::fs::create_dir_all(path.parent().unwrap())?;
        let guard = Self {
            path,
            token: uuid::Uuid::new_v4().to_string(),
            retained: false,
        };
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&guard.path)
            .map_err(|_| Error::Other("session already open or starting".into()))?;
        use std::io::Write;
        let written = file.write_all(serde_json::json!({"opened_at": SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_secs(), "launch_id": guard.token}).to_string().as_bytes());
        if let Err(error) = written {
            drop(file);
            let _ = std::fs::remove_file(&guard.path);
            return Err(error.into());
        }
        Ok(guard)
    }

    /// The terminal owns cleanup after a successful spawn; failed CLI startup
    /// releases the lock even though PowerShell itself remains open.
    pub(crate) fn wrap_script(&self, path: &std::path::Path) -> Result<()> {
        let script = std::fs::read_to_string(path)?;
        let script = script.trim_start_matches('\u{feff}');
        let lock = self.path.to_string_lossy();
        let wrapped = if cfg!(windows) {
            let lock = lock.replace('\'', "''");
            format!("\u{feff}$ErrorActionPreference = 'Stop'\ntry {{\n{script}\n}} finally {{\n  $asLock = '{lock}'\n  if (Test-Path -LiteralPath $asLock) {{\n    try {{\n      $asOwner = Get-Content -LiteralPath $asLock -Raw | ConvertFrom-Json\n      if ($asOwner.launch_id -eq '{}') {{ Remove-Item -LiteralPath $asLock -ErrorAction SilentlyContinue }}\n    }} catch {{ }}\n  }}\n}}\n", self.token)
        } else {
            let lock = lock.replace('\'', "'\\''");
            let body = script
                .trim_start_matches("#!/bin/sh\n")
                .replace("\nexec '", "\n'");
            format!("#!/bin/sh\nAS_SESSION_LOCK='{lock}'\ntrap 'if grep -q \"{}\" \"$AS_SESSION_LOCK\" 2>/dev/null; then rm -f -- \"$AS_SESSION_LOCK\"; fi' EXIT\n{body}", self.token)
        };
        std::fs::write(path, wrapped)?;
        Ok(())
    }

    pub fn retain(mut self) {
        self.retained = true;
    }
}

impl Drop for SessionOpenGuard {
    fn drop(&mut self) {
        if !self.retained {
            let owner = std::fs::read_to_string(&self.path)
                .ok()
                .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok());
            if owner.as_ref().and_then(|v| v["launch_id"].as_str()) == Some(self.token.as_str()) {
                let _ = std::fs::remove_file(&self.path);
            }
        }
    }
}

pub fn mark_session_open(store: &ProfileStore, id: &str) -> Result<()> {
    SessionOpenGuard::acquire(store, id)?.retain();
    Ok(())
}

pub fn clear_open_lock(store: &ProfileStore, id: &str) {
    if valid_id(id) {
        let _ = std::fs::remove_file(open_lock_path(store, id));
    }
}

pub fn session_activities(store: &ProfileStore) -> SessionActivities {
    inspect(store, session_process::snapshot())
}

fn inspect(store: &ProfileStore, snapshot: Snapshot) -> SessionActivities {
    let mut states = BTreeMap::new();
    for process in &snapshot.processes {
        for id in session_process::claimed_sessions(process) {
            if valid_id(&id) {
                states.insert(id, SessionActivity::Running);
            }
        }
    }
    // Read only session ownership metadata. Never read job providerEnv or keys.
    if let Ok(profiles) = std::fs::read_dir(store.runtime_dir()) {
        for profile in profiles.flatten() {
            let Ok(records) = std::fs::read_dir(profile.path().join(".claude/sessions")) else {
                continue;
            };
            for record in records.flatten() {
                if record.path().extension().and_then(|e| e.to_str()) != Some("json") {
                    continue;
                }
                let Some(value) = std::fs::read_to_string(record.path())
                    .ok()
                    .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                else {
                    continue;
                };
                let Some(id) = value["sessionId"].as_str().filter(|id| valid_id(id)) else {
                    continue;
                };
                let Some(pid) = value["pid"].as_u64().and_then(|p| u32::try_from(p).ok()) else {
                    continue;
                };
                let Some(process) = snapshot
                    .processes
                    .iter()
                    .find(|p| p.pid == pid && session_process::is_agent(&p.name))
                else {
                    continue;
                };
                let claimed = session_process::claimed_sessions(process);
                if !claimed.is_empty() && !claimed.iter().any(|claimed| claimed == id) {
                    continue;
                }
                let identity = match (value["procStart"].as_str(), process.start.as_deref()) {
                    (Some(recorded), Some(actual)) => recorded == actual,
                    _ => claimed.iter().any(|claimed| claimed == id),
                };
                if !identity {
                    if process.start.is_none() {
                        states.entry(id.into()).or_insert(SessionActivity::Unknown);
                    }
                    continue;
                }
                let background = matches!(value["kind"].as_str(), Some("bg" | "background"));
                let state = if background {
                    let attach_id = value["jobId"]
                        .as_str()
                        .filter(|id| valid_id(id))
                        .unwrap_or(id)
                        .to_string();
                    SessionActivity::Background { attach_id }
                } else {
                    SessionActivity::Running
                };
                states.insert(id.to_string(), state);
            }
        }
    }
    if let Ok(locks) = std::fs::read_dir(store.root.join("sessions-open")) {
        for entry in locks.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Some(id) = path
                .file_stem()
                .and_then(|s| s.to_str())
                .filter(|id| valid_id(id))
            else {
                continue;
            };
            if states.contains_key(id) {
                continue;
            }
            let state = if !snapshot.complete {
                SessionActivity::Unknown
            } else {
                let modified = entry
                    .metadata()
                    .and_then(|m| m.modified())
                    .unwrap_or(SystemTime::UNIX_EPOCH);
                if SystemTime::now()
                    .duration_since(modified)
                    .unwrap_or_default()
                    < Duration::from_secs(60)
                {
                    SessionActivity::Starting
                } else {
                    // A concurrently replaced lock must survive this stale snapshot.
                    if path.metadata().and_then(|m| m.modified()).ok() == Some(modified) {
                        let _ = std::fs::remove_file(&path);
                    }
                    SessionActivity::Closed
                }
            };
            states.insert(id.to_string(), state);
        }
    }
    SessionActivities {
        states,
        complete: snapshot.complete,
    }
}

pub fn session_is_open(store: &ProfileStore, id: &str) -> bool {
    session_activities(store).get(id).is_open()
}
pub fn open_session_ids(store: &ProfileStore) -> HashSet<String> {
    session_activities(store).open_ids()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_process::Process;
    fn store() -> ProfileStore {
        ProfileStore {
            root: std::env::temp_dir().join(format!("as-activity-{}", uuid::Uuid::new_v4())),
        }
    }
    fn record(store: &ProfileStore, start: &str) {
        let dir = store.runtime_dir().join("p/.claude/sessions");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("42.json"), serde_json::json!({"pid":42,"procStart":start,"sessionId":"current","kind":"bg","jobId":"current"}).to_string()).unwrap();
    }
    fn snapshot() -> Snapshot {
        Snapshot {
            complete: true,
            processes: vec![Process {
                pid: 42,
                name: "claude.exe".into(),
                start: Some("100".into()),
                command: Some("claude --session-id current --fork-session --resume source".into()),
            }],
        }
    }
    #[test]
    fn background_without_an_agent_switch_lock_is_attachable_and_source_is_closed() {
        let store = store();
        record(&store, "100");
        let activity = inspect(&store, snapshot());
        assert_eq!(
            activity.get("current"),
            SessionActivity::Background {
                attach_id: "current".into()
            }
        );
        assert_eq!(activity.get("source"), SessionActivity::Closed);
        let _ = std::fs::remove_dir_all(store.root);
    }
    #[test]
    fn reused_pid_does_not_make_a_stale_background_record_attachable() {
        let store = store();
        record(&store, "99");
        assert_eq!(
            inspect(&store, snapshot()).get("current"),
            SessionActivity::Running
        );
        let mut unrelated = snapshot();
        unrelated.processes[0].command = Some("claude --session-id unrelated".into());
        assert_eq!(
            inspect(&store, unrelated).get("current"),
            SessionActivity::Closed
        );
        let _ = std::fs::remove_dir_all(store.root);
    }
    #[test]
    fn failed_scan_is_unknown_and_never_prunes_or_allows_double_resume() {
        let store = store();
        mark_session_open(&store, "test").unwrap();
        let file = std::fs::File::options()
            .write(true)
            .open(open_lock_path(&store, "test"))
            .unwrap();
        file.set_modified(SystemTime::now() - Duration::from_secs(120))
            .unwrap();
        let result = inspect(
            &store,
            Snapshot {
                complete: false,
                processes: vec![],
            },
        );
        assert_eq!(result.get("test"), SessionActivity::Unknown);
        assert_eq!(result.get("no-lock"), SessionActivity::Unknown);
        assert!(open_lock_path(&store, "test").exists());
        let _ = std::fs::remove_dir_all(store.root);
    }
    #[test]
    fn failed_launch_rolls_back_and_an_old_guard_cannot_remove_a_new_lock() {
        let store = store();
        let first = SessionOpenGuard::acquire(&store, "test").unwrap();
        assert!(SessionOpenGuard::acquire(&store, "test").is_err());
        clear_open_lock(&store, "test");
        let second = SessionOpenGuard::acquire(&store, "test").unwrap();
        drop(first);
        assert!(open_lock_path(&store, "test").exists());
        drop(second);
        assert!(!open_lock_path(&store, "test").exists());
        assert!(SessionOpenGuard::acquire(&store, "../escape").is_err());
        let _ = std::fs::remove_dir_all(store.root);
    }
    #[test]
    fn failed_cli_releases_its_lock_even_if_the_terminal_remains_open() {
        let store = store();
        let guard = SessionOpenGuard::acquire(&store, "failed").unwrap();
        #[cfg(windows)]
        let (path, contents) = (
            store.root.join("failure.ps1"),
            "& $env:ComSpec /D /C 'exit 23'\n",
        );
        #[cfg(not(windows))]
        let (path, contents) = (
            store.root.join("failure.sh"),
            "#!/bin/sh\nexec '/bin/sh' '-c' 'exit 23'\n",
        );
        std::fs::write(&path, contents).unwrap();
        guard.wrap_script(&path).unwrap();
        guard.retain();
        #[cfg(windows)]
        let result = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
            .arg(&path)
            .status()
            .unwrap();
        #[cfg(not(windows))]
        let result = std::process::Command::new("sh")
            .arg(&path)
            .status()
            .unwrap();
        let _ = result;
        assert!(!open_lock_path(&store, "failed").exists());
        #[cfg(windows)]
        {
            // PowerShell terminating errors must also run the finally block.
            let guard = SessionOpenGuard::acquire(&store, "failed").unwrap();
            std::fs::write(&path, "throw 'simulated startup error'\n").unwrap();
            guard.wrap_script(&path).unwrap();
            guard.retain();
            let result = std::process::Command::new("powershell.exe")
                .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
                .arg(&path)
                .stderr(std::process::Stdio::null())
                .status()
                .unwrap();
            assert!(!result.success());
            assert!(!open_lock_path(&store, "failed").exists());
        }
        let _ = std::fs::remove_dir_all(store.root);
    }
}
