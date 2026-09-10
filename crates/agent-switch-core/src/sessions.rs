//! Session history across the per-profile isolated homes.
//!
//! Claude Code stores transcripts under `<home>/projects/<project-slug>/*.jsonl`;
//! Codex stores rollouts under `<home>/sessions/YYYY/MM/DD/rollout-*.jsonl`.
//! Both are plain JSON lines. The parsing here is deliberately tolerant:
//! an unknown shape still yields a session row (id from the file name,
//! mtime for ordering) with best-effort cwd/preview fields.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::engine::Engine;
use crate::error::{Error, Result};
use crate::profile_store::ProfileStore;

/// One resumable conversation found in a profile's isolated home.
pub struct Session {
    /// Session id (Claude transcript uuid / Codex session_meta id; falls
    /// back to the file name).
    pub session_id: String,
    /// Profile (home dir) the session belongs to.
    pub profile_id: String,
    pub engine: Engine,
    /// Working directory the session was run in (best effort).
    pub cwd: Option<String>,
    /// First user message, flattened and truncated (best effort).
    pub preview: String,
    /// Last activity (file mtime).
    pub modified: SystemTime,
}

impl Session {
    /// CLI arguments that resume this session inside its home + cwd:
    /// Claude → `claude --resume <id>`, Codex → `codex resume <id>`.
    pub fn resume_args(&self) -> Vec<String> {
        match self.engine {
            Engine::Claude => vec!["--resume".to_string(), self.session_id.clone()],
            Engine::Codex => vec!["resume".to_string(), self.session_id.clone()],
        }
    }
}

/// How many head lines of a transcript are inspected for metadata.
const MAX_LINES: usize = 400;
/// Preview length cap (chars).
const PREVIEW_CAP: usize = 120;

/// All sessions across every `runtime/<profile-id>` home, newest first.
pub fn list_sessions(store: &ProfileStore) -> Result<Vec<Session>> {
    let mut out = Vec::new();
    let root = store.runtime_dir();
    if !root.is_dir() {
        return Ok(out);
    }
    for entry in std::fs::read_dir(&root)?.flatten() {
        let home_root = entry.path();
        if !home_root.is_dir() {
            continue;
        }
        let Some(profile_id) = home_root
            .file_name()
            .and_then(|n| n.to_str())
            .map(|s| s.to_string())
        else {
            continue;
        };
        for engine in [Engine::Claude, Engine::Codex] {
            let home = home_root.join(engine.home_dir_name());
            if !home.is_dir() {
                continue;
            }
            let files = match engine {
                Engine::Claude => session_files_claude(&home),
                Engine::Codex => session_files_codex(&home),
            };
            for path in files {
                let values = read_head_lines(&path);
                let session = match engine {
                    Engine::Claude => parse_claude_session(&path, &values),
                    Engine::Codex => parse_codex_session(&path, &values),
                };
                if let Some(mut s) = session {
                    s.profile_id = profile_id.clone();
                    s.engine = engine;
                    s.modified = std::fs::metadata(&path)
                        .and_then(|m| m.modified())
                        .unwrap_or(SystemTime::UNIX_EPOCH);
                    out.push(s);
                }
            }
        }
    }
    out.sort_by(|a, b| b.modified.cmp(&a.modified));
    Ok(out)
}

/// Find one session by id across all homes.
pub fn find_session(store: &ProfileStore, session_id: &str) -> Result<Session> {
    for s in list_sessions(store)? {
        if s.session_id == session_id {
            return Ok(s);
        }
    }
    Err(Error::Other(format!("session not found: {session_id}")))
}

/// `<home>/projects/<project-slug>/*.jsonl`
fn session_files_claude(home: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let projects = home.join("projects");
    if let Ok(dirs) = std::fs::read_dir(&projects) {
        for d in dirs.flatten() {
            if d.path().is_dir() {
                jsonl_files(&d.path(), &mut out);
            }
        }
    }
    out
}

/// `<home>/sessions/YYYY/MM/DD/rollout-*.jsonl` (three nested date dirs).
fn session_files_codex(home: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let sessions = home.join("sessions");
    if let Ok(years) = std::fs::read_dir(&sessions) {
        for y in years.flatten() {
            if !y.path().is_dir() {
                continue;
            }
            if let Ok(months) = std::fs::read_dir(y.path()) {
                for m in months.flatten() {
                    if !m.path().is_dir() {
                        continue;
                    }
                    if let Ok(days) = std::fs::read_dir(m.path()) {
                        for d in days.flatten() {
                            if d.path().is_dir() {
                                jsonl_files(&d.path(), &mut out);
                            }
                        }
                    }
                }
            }
        }
    }
    out
}

/// Push every `*.jsonl` file directly inside `dir` (non-recursive).
fn jsonl_files(dir: &Path, out: &mut Vec<PathBuf>) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for f in entries.flatten() {
            let p = f.path();
            if p.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                out.push(p);
            }
        }
    }
}

/// First N lines of a .jsonl file as JSON values (bad lines are skipped).
fn read_head_lines(path: &Path) -> Vec<serde_json::Value> {
    let Ok(file) = std::fs::File::open(path) else {
        return Vec::new();
    };
    use std::io::BufRead;
    std::io::BufReader::new(file)
        .lines()
        .take(MAX_LINES)
        .filter_map(|l| l.ok())
        .filter_map(|l| serde_json::from_str(&l).ok())
        .collect()
}

/// Claude transcript: each record carries top-level `sessionId`/`cwd`; the
/// first `type:"user"` record's message is the preview.
fn parse_claude_session(path: &Path, values: &[serde_json::Value]) -> Option<Session> {
    let fallback_id = file_stem(path);
    let mut session_id = fallback_id.clone();
    let mut cwd: Option<String> = None;
    let mut preview = String::new();
    for v in values {
        if session_id == fallback_id {
            if let Some(sid) = v.get("sessionId").and_then(|x| x.as_str()) {
                if !sid.is_empty() {
                    session_id = sid.to_string();
                }
            }
        }
        if cwd.is_none() {
            cwd = v.get("cwd").and_then(|x| x.as_str()).map(|s| s.to_string());
        }
        if preview.is_empty()
            && v.get("type").and_then(|x| x.as_str()) == Some("user")
        {
            if let Some(msg) = v.get("message") {
                if let Some(text) = extract_message_text(msg) {
                    preview = text;
                }
            }
        }
        if cwd.is_some() && !preview.is_empty() {
            break;
        }
    }
    Some(Session {
        session_id,
        profile_id: String::new(),
        engine: Engine::Claude,
        cwd,
        preview,
        modified: SystemTime::UNIX_EPOCH,
    })
}

/// Codex rollout: the first line is `session_meta` (payload.id / payload.cwd);
/// user messages arrive as `response_item` with payload.type "message".
fn parse_codex_session(path: &Path, values: &[serde_json::Value]) -> Option<Session> {
    let fallback_id = rollout_session_id(&file_stem(path));
    let mut session_id = fallback_id.clone();
    let mut cwd: Option<String> = None;
    let mut preview = String::new();
    for v in values {
        match v.get("type").and_then(|x| x.as_str()) {
            Some("session_meta") => {
                let p = v.get("payload");
                if let Some(id) = p.and_then(|p| p.get("id")).and_then(|x| x.as_str()) {
                    if !id.is_empty() {
                        session_id = id.to_string();
                    }
                }
                if cwd.is_none() {
                    cwd = p
                        .and_then(|p| p.get("cwd"))
                        .and_then(|x| x.as_str())
                        .map(|s| s.to_string());
                }
            }
            Some("response_item") if preview.is_empty() => {
                if let Some(p) = v.get("payload") {
                    let is_user_message = p
                        .get("type")
                        .and_then(|x| x.as_str())
                        == Some("message")
                        && p.get("role").and_then(|x| x.as_str()) == Some("user");
                    if is_user_message {
                        if let Some(text) = extract_message_text(p) {
                            preview = text;
                        }
                    }
                }
            }
            _ => {}
        }
        if cwd.is_some() && !preview.is_empty() {
            break;
        }
    }
    Some(Session {
        session_id,
        profile_id: String::new(),
        engine: Engine::Codex,
        cwd,
        preview,
        modified: SystemTime::UNIX_EPOCH,
    })
}

/// `message.content` may be a string or an array of blocks (Claude:
/// `{type:"text",text}`; Codex: `{type:"input_text",text}`).
fn extract_message_text(message: &serde_json::Value) -> Option<String> {
    let content = message.get("content")?;
    let raw = match content {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(blocks) => blocks
            .iter()
            .filter_map(|b| {
                b.get("text")
                    .or_else(|| b.get("input_text"))
                    .and_then(|t| t.as_str())
                    .map(|t| t.to_string())
            })
            .collect::<Vec<_>>()
            .join(" "),
        _ => return None,
    };
    flatten_preview(&raw)
}

/// Collapse whitespace and cap the length (char-safe).
fn flatten_preview(raw: &str) -> Option<String> {
    let flat: String = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = flat.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.chars().take(PREVIEW_CAP).collect())
    }
}

/// File stem without extension.
fn file_stem(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_string()
}

/// `rollout-2026-09-10T10-00-00-<uuid>` → the trailing uuid when present,
/// otherwise the whole stem.
fn rollout_session_id(stem: &str) -> String {
    match stem.rsplit('-').next() {
        Some(last) if last.len() >= 32 => last.to_string(),
        _ => stem.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root() -> PathBuf {
        std::env::temp_dir().join(format!(
            "as-sess-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ))
    }

    #[test]
    fn claude_session_parsed() {
        let root = temp_root();
        let store = ProfileStore { root: root.clone() };
        let dir = store.runtime_dir().join("myprofile").join(".claude").join("projects");
        let proj = dir.join("C--Users-demo");
        std::fs::create_dir_all(&proj).unwrap();
        let file = proj.join("11111111-2222-3333-4444-555555555555.jsonl");
        std::fs::write(
            &file,
            r#"{"type":"user","sessionId":"11111111-2222-3333-4444-555555555555","cwd":"C:\\Users\\demo","message":{"role":"user","content":"hello there  how are you?"}}
{"type":"assistant","sessionId":"11111111-2222-3333-4444-555555555555","cwd":"C:\\Users\\demo","message":{"role":"assistant","content":[{"type":"text","text":"hi"}]}}"#,
        )
        .unwrap();

        let sessions = list_sessions(&store).unwrap();
        assert_eq!(sessions.len(), 1);
        let s = &sessions[0];
        assert_eq!(s.session_id, "11111111-2222-3333-4444-555555555555");
        assert_eq!(s.profile_id, "myprofile");
        assert!(matches!(s.engine, Engine::Claude));
        assert_eq!(s.cwd.as_deref(), Some("C:\\Users\\demo"));
        assert!(s.preview.starts_with("hello there"));
        assert_eq!(
            s.resume_args(),
            vec!["--resume", "11111111-2222-3333-4444-555555555555"]
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn codex_session_parsed() {
        let root = temp_root();
        let store = ProfileStore { root: root.clone() };
        let dir = store
            .runtime_dir()
            .join("gptprof")
            .join(".codex")
            .join("sessions")
            .join("2026")
            .join("09")
            .join("10");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("rollout-2026-09-10T10-00-00-99999999-8888-7777-6666-555555555555.jsonl");
        std::fs::write(
            &file,
            r#"{"type":"session_meta","payload":{"id":"99999999-8888-7777-6666-555555555555","cwd":"/home/dev/proj"}}
{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"fix the build"}]}}"#,
        )
        .unwrap();

        let sessions = list_sessions(&store).unwrap();
        assert_eq!(sessions.len(), 1);
        let s = &sessions[0];
        assert_eq!(s.session_id, "99999999-8888-7777-6666-555555555555");
        assert_eq!(s.profile_id, "gptprof");
        assert!(matches!(s.engine, Engine::Codex));
        assert_eq!(s.cwd.as_deref(), Some("/home/dev/proj"));
        assert_eq!(s.preview, "fix the build");
        assert_eq!(s.resume_args(), vec!["resume", "99999999-8888-7777-6666-555555555555"]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn unknown_shapes_still_listed() {
        let root = temp_root();
        let store = ProfileStore { root: root.clone() };
        let dir = store.runtime_dir().join("p1").join(".claude").join("projects");
        let proj = dir.join("X");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(proj.join("abc123.jsonl"), "not json at all\n").unwrap();
        let sessions = list_sessions(&store).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, "abc123");
        assert!(sessions[0].preview.is_empty());
        assert!(sessions[0].cwd.is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn find_session_across_homes() {
        let root = temp_root();
        let store = ProfileStore { root: root.clone() };
        let dir = store.runtime_dir().join("p2").join(".claude").join("projects");
        let proj = dir.join("Y");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(
            proj.join("s-id-42.jsonl"),
            r#"{"sessionId":"s-id-42","cwd":"/w","type":"user","message":{"role":"user","content":"hi"}}"#,
        )
        .unwrap();
        let s = find_session(&store, "s-id-42").unwrap();
        assert_eq!(s.profile_id, "p2");
        assert!(find_session(&store, "nope").is_err());
        let _ = std::fs::remove_dir_all(&root);
    }
}
