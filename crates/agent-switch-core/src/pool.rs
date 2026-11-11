//! Session pool migration: when the user changes the pool path, stop the
//! sessions agent-switch has running (agents + the terminals hosting them)
//! and move every profile home to the new location. Homes are moved, never
//! copied-then-deleted unless a rename cannot cross the volume boundary.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::error::Result;
use crate::profile_store::ProfileStore;

#[derive(Debug, Clone, Serialize)]
pub struct PoolEntryMoved {
    pub name: String,
    pub from: String,
    pub to: String,
    /// `moved` = renamed in place (same volume); `merged` = copied and
    /// merged (cross-volume, or the name already existed in the target).
    pub how: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct StoppedProcess {
    pub pid: u32,
    pub name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct StoppedSession {
    /// Session id; empty = a pool process not tied to a session (a daemon,
    /// a terminal whose tab closed, …).
    pub session_id: String,
    pub activity: String,
    pub processes: Vec<StoppedProcess>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct PoolMigration {
    pub from: String,
    pub to: String,
    pub moved: Vec<PoolEntryMoved>,
    pub stopped: Vec<StoppedSession>,
    pub failures: Vec<String>,
}

/// Stop every session agent-switch has open (and the processes that hold
/// the given home dirs: their terminal hosts, the CLIs, and any daemon
/// whose command line references a home). Best effort — returns what was
/// actually stopped; a process that survived is left for the caller to
/// report through the moved entries' failures.
pub fn stop_pool_sessions(store: &ProfileStore, homes: &[PathBuf]) -> Vec<StoppedSession> {
    let activities = crate::sessions::session_activities(store);
    // Deterministic report order (open_ids returns a HashSet).
    let open: BTreeSet<String> = activities.open_ids().into_iter().collect();
    let home_markers: Vec<String> = homes
        .iter()
        .filter(|h| h.is_dir())
        .map(|h| normalized_marker(&h.to_string_lossy()))
        .filter(|m| !m.is_empty())
        .collect();

    let mut by_session: BTreeMap<String, StoppedSession> = BTreeMap::new();
    let mut killed: BTreeSet<u32> = BTreeSet::new();

    // Two passes: Windows needs a moment after TerminateProcess before the
    // handles (and the dirs) are released; the second pass catches
    // stragglers and processes spawned in the gap.
    for _ in 0..2 {
        let snapshot = crate::session_process::snapshot_all();
        let mut pending: BTreeMap<u32, (String, String)> = BTreeMap::new(); // pid -> (session id, name)
        for process in &snapshot.processes {
            if killed.contains(&process.pid) {
                continue;
            }
            let command = process.command.as_deref().unwrap_or_default();
            let id = {
                let claimed = crate::session_process::claimed_sessions(process)
                    .iter()
                    .filter(|id| open.contains(id.as_str()))
                    .next()
                    .cloned();
                if claimed.is_some() {
                    claimed
                } else if crate::session_process::is_terminal_host(&process.name) {
                    open.iter().find(|id| command.contains(id.as_str())).cloned()
                } else {
                    None
                }
                .or_else(|| {
                    home_markers
                        .iter()
                        .any(|m| normalized_marker(command).contains(m.as_str()))
                        .then(|| String::new())
                })
            };
            if let Some(id) = id {
                pending.insert(process.pid, (id, process.name.clone()));
            }
        }
        for (pid, (id, name)) in pending {
            if crate::session_process::kill_process(pid) {
                killed.insert(pid);
                let entry = by_session.entry(id.clone()).or_insert_with(|| StoppedSession {
                    session_id: id.clone(),
                    activity: if id.is_empty() {
                        "daemon".into()
                    } else {
                        activities.get(&id).label().into()
                    },
                    processes: Vec::new(),
                });
                entry.processes.push(StoppedProcess { pid, name });
            }
        }
    }
    by_session.into_values().collect()
}

/// Move every entry of every source pool root into `to`. A source whose
/// canonical path equals `to` is skipped (the target root is also a pool
/// root once the settings are saved). `extra_roots` covers pool dirs the
/// settings no longer remember (the previous custom pool path). Entries
/// are processed one by one; a failed entry is recorded, never aborting
/// the rest. Codex daemon junctions are repaired in every moved home.
pub fn migrate_pool_homes(
    store: &ProfileStore,
    to: &Path,
    extra_roots: &[PathBuf],
) -> Result<PoolMigration> {
    let mut roots: Vec<PathBuf> = store.pool_roots();
    for root in extra_roots {
        let canonical = std::fs::canonicalize(root).ok();
        let known = roots.iter().any(|r| {
            r == root
                || Some(r) == canonical.as_ref()
                || std::fs::canonicalize(r)
                    .ok()
                    .as_ref()
                    == canonical.as_ref()
        });
        if !known && root.is_dir() {
            roots.push(root.clone());
        }
    }
    let from = roots
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");

    let to = std::fs::create_dir_all(to).and_then(|()| std::fs::canonicalize(to))?;
    let mut moved = Vec::new();
    let mut failures = Vec::new();
    for root in roots {
        let Ok(root) = std::fs::canonicalize(&root) else {
            continue;
        };
        if root == to {
            continue;
        }
        // Collect first: moving an entry invalidates a live read_dir
        // iterator on Windows (FindNextFile may skip or stop early).
        let mut entries: Vec<PathBuf> = std::fs::read_dir(&root)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .collect();
        entries.sort();
        for src in entries {
            let Some(name) = src.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let dest = to.join(name);
            match migrate_entry(&src, &dest) {
                Ok(how) => {
                    crate::runtime::repair_codex_daemon_link(&dest);
                    moved.push(PoolEntryMoved {
                        name: name.to_string(),
                        from: src.display().to_string(),
                        to: dest.display().to_string(),
                        how,
                    });
                }
                Err(e) => failures.push(format!("{}: {e}", src.display())),
            }
        }
        if std::fs::read_dir(&root)
            .map(|mut d| d.next().is_none())
            .unwrap_or(false)
        {
            let _ = std::fs::remove_dir(&root);
        }
    }
    Ok(PoolMigration {
        from,
        to: to.display().to_string(),
        moved,
        stopped: Vec::new(),
        failures,
    })
}

/// Move one entry into the target root. Returns how it got there.
fn migrate_entry(from: &Path, dest: &Path) -> Result<String> {
    if !dest.exists() && std::fs::rename(from, dest).is_ok() {
        return Ok("moved".into());
    }
    // Cross-volume (EXDEV), a vanished-then-recreated target, or a name
    // that already exists: copy (merging, never clobbering) then delete.
    copy_tree(from, dest)?;
    if from.is_dir() {
        std::fs::remove_dir_all(from)?;
    } else {
        std::fs::remove_file(from)?;
    }
    Ok("merged".into())
}

/// Copy a file or directory tree into `dest`, merging when the name
/// already exists (both dirs recursively; files renamed to
/// `<name>.legacyN`), so a half-migrated layout never loses data.
fn copy_tree(from: &Path, dest: &Path) -> Result<()> {
    if from.is_dir() {
        std::fs::create_dir_all(dest)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            let src = entry.path();
            let dst = dest.join(entry.file_name());
            if src.is_dir() && dst.is_dir() {
                copy_tree(&src, &dst)?;
            } else if !dst.exists() {
                copy_tree(&src, &dst)?;
            } else {
                let base = entry.file_name().to_string_lossy().to_string();
                let mut i = 1u32;
                let mut renamed = dst.with_file_name(format!("{base}.legacy{i}"));
                while renamed.exists() {
                    i += 1;
                    renamed = dst.with_file_name(format!("{base}.legacy{i}"));
                }
                std::fs::copy(&src, &renamed)?;
            }
        }
        Ok(())
    } else {
        let dst = if dest.exists() {
            let base = from.file_name().unwrap_or_default().to_string_lossy().to_string();
            let mut i = 1u32;
            let mut p = dest.with_file_name(format!("{base}.legacy{i}"));
            while p.exists() {
                i += 1;
                p = dest.with_file_name(format!("{base}.legacy{i}"));
            }
            p
        } else {
            dest.to_path_buf()
        };
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(from, &dst)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = std::fs::metadata(from) {
                let _ = std::fs::set_permissions(
                    &dst,
                    std::fs::Permissions::from_mode(meta.permissions().mode()),
                );
            }
        }
        Ok(())
    }
}

/// Lowercase, slashes unified, no trailing slash — for comparing paths
/// that may appear with different separators in a command line.
fn normalized_marker(s: &str) -> String {
    s.replace('\\', "/").trim_end_matches('/').to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store() -> (ProfileStore, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "as-pool-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        (ProfileStore { root: dir.clone() }, dir)
    }

    #[test]
    fn migrate_moves_entries_and_merges_same_names() {
        let (store, dir) = temp_store();
        let rt = store.runtime_dir();
        std::fs::create_dir_all(rt.join("a").join(".codex")).unwrap();
        std::fs::write(rt.join("a/.codex/config.toml"), "a").unwrap();
        std::fs::create_dir_all(rt.join("b")).unwrap();
        std::fs::write(rt.join("b/file"), "b").unwrap();
        let to = dir.join("pool");
        // `b` already exists in the target: its children are merged, never clobbered.
        std::fs::create_dir_all(to.join("b")).unwrap();
        std::fs::write(to.join("b/other"), "other").unwrap();
        std::fs::write(rt.join("b/other"), "b-other").unwrap();

        let result = migrate_pool_homes(&store, &to, &[]).unwrap();
        assert!(to.join("a/.codex/config.toml").exists());
        assert!(to.join("b/file").exists());
        assert_eq!(std::fs::read_to_string(to.join("b/other")).unwrap(), "other");
        assert!(to.join("b/other.legacy1").exists());
        assert!(!rt.join("a").exists());
        assert!(!rt.join("b").exists());
        assert!(result.failures.is_empty());
        assert!(result.moved.iter().any(|m| m.name == "a"));
        assert!(result
            .moved
            .iter()
            .any(|m| m.name == "b" && m.how == "merged"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn migrate_picks_up_extra_roots_and_skips_the_target() {
        let (store, dir) = temp_store();
        // A previous custom pool dir the settings no longer remember.
        let old = dir.join("old-pool");
        std::fs::create_dir_all(old.join("gpt")).unwrap();
        std::fs::write(old.join("gpt/file"), "gpt").unwrap();
        let to = dir.join("new-pool");

        let result = migrate_pool_homes(&store, &to, &[old.clone()]).unwrap();
        assert!(to.join("gpt/file").exists());
        assert!(!old.join("gpt").exists());
        assert!(result.moved.iter().any(|m| m.name == "gpt"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn normalized_marker_unifies_separators() {
        assert_eq!(
            normalized_marker("C:\\Users\\a\\rt\\b"),
            normalized_marker("c:/users/a/rt/b/")
        );
        assert!(normalized_marker("C:\\pool").contains("c:/pool"));
    }
}
