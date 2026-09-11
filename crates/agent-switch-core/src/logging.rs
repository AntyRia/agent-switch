//! Minimal file logging for troubleshooting: one log file per day under
//! `<config_root>/logs/` (`agent-switch-YYYY-MM-DD.log`), capped at 5 MB
//! (a rotated `.old.log` is kept), tail-readable by the GUI's log viewer
//! and the CLI's `logs` command.
//!
//! Deliberately dependency-light (chrono only, for local timestamps): the
//! logger is a mutex-guarded append writer with one flush per line.
//!
//! SECURITY: call sites must never pass API keys (or other secrets) to
//! `log`/`info`/`warn`/`error` — the log file is plain text in the user's
//! config directory.

use std::fs;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::Local;

static LOGGER: Mutex<Option<BufWriter<fs::File>>> = Mutex::new(None);
static LOG_PATH: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Cap for today's file before it is rotated to `<name>.old.log`.
const MAX_FILE_BYTES: u64 = 5 * 1024 * 1024;

/// Today's log file name, e.g. `agent-switch-2026-09-10.log`.
fn log_file_name() -> String {
    format!("agent-switch-{}.log", Local::now().format("%Y-%m-%d"))
}

/// Open (creating if needed) today's log file and start writing to it.
/// Idempotent: a second call in the same process re-opens the same file.
/// Failures are silent — logging must never block the application.
pub fn init(root: &Path) {
    let logs_dir = root.join("logs");
    if fs::create_dir_all(&logs_dir).is_err() {
        return;
    }
    let name = log_file_name();
    let path = logs_dir.join(&name);
    // Rotate a bloated file (the .old copy is overwritten on the next
    // rotation); a fresh day starts a fresh file anyway.
    if let Ok(meta) = fs::metadata(&path) {
        if meta.len() > MAX_FILE_BYTES {
            let _ = fs::rename(&path, logs_dir.join(format!("{name}.old")));
        }
    }
    let file = match fs::OpenOptions::new().create(true).append(true).open(&path) {
        Ok(f) => f,
        Err(_) => return,
    };
    if let Ok(mut guard) = LOGGER.lock() {
        *guard = Some(BufWriter::new(file));
    }
    if let Ok(mut guard) = LOG_PATH.lock() {
        *guard = Some(path);
    }
}

/// Append one line: `[YYYY-MM-DD HH:MM:SS.mmm] [LEVEL] message`.
pub fn log(level: &str, msg: &str) {
    let Ok(mut guard) = LOGGER.lock() else {
        return;
    };
    let Some(writer) = guard.as_mut() else {
        return;
    };
    let stamp = Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
    if writeln!(writer, "[{stamp}] [{level}] {msg}").is_ok() {
        let _ = writer.flush();
    }
}

pub fn info(msg: &str) {
    log("INFO", msg);
}

pub fn warn(msg: &str) {
    log("WARN", msg);
}

pub fn error(msg: &str) {
    log("ERROR", msg);
}

/// Path of the log file this process writes to (None before `init` or if
/// the open failed).
pub fn path() -> Option<PathBuf> {
    LOG_PATH.lock().ok().and_then(|g| g.clone())
}

/// Last `n` lines of the current log file (falls back to the rotated
/// `.old` file when today's file is missing). Used by the GUI log viewer
/// and `agent-switch logs`.
pub fn recent_lines(n: usize) -> Vec<String> {
    let path = match path() {
        Some(p) if p.exists() => p,
        Some(p) => {
            let mut old = p.clone();
            let stem = p
                .file_name()
                .and_then(|f| f.to_str())
                .and_then(|f| f.strip_suffix(".log"))
                .map(|s| format!("{s}.old.log"));
            if let Some(name) = stem {
                old.set_file_name(name);
            }
            if old.exists() {
                old
            } else {
                return Vec::new();
            }
        }
        None => return Vec::new(),
    };
    let content = match fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    content
        .lines()
        .rev()
        .take(n)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(|l| l.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_log_recent_roundtrip() {
        // The logger is process-global and tests run in parallel, so
        // concurrent tests may interleave their lines — assert on content,
        // not on the exact count.
        let dir = std::env::temp_dir().join(format!(
            "as-logging-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        init(&dir);
        assert!(path().is_some());
        info("hello log line");
        warn("warning line");
        let lines = recent_lines(100);
        let joined = lines.join("\n");
        assert!(joined.contains("[INFO] hello log line"));
        assert!(joined.contains("[WARN] warning line"));
        // Lines are timestamped first.
        assert!(lines.iter().all(|l| l.starts_with('[')));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn recent_lines_is_empty_without_init() {
        // A fresh process without init() logs nowhere.
        // (Skips the assertion when another test in this process already
        // initialized the logger — the static is shared per process.)
        if path().is_none() {
            assert!(recent_lines(5).is_empty());
        }
    }
}
