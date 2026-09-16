//! Session history across the per-profile isolated homes.
//!
//! Claude Code stores transcripts under `<home>/projects/<project-slug>/*.jsonl`;
//! Codex stores rollouts under `<home>/sessions/YYYY/MM/DD/rollout-*.jsonl`.
//! Both are plain JSON lines. The parsing here is deliberately tolerant:
//! an unknown shape still yields a session row (id from the file name,
//! mtime for ordering) with best-effort cwd/preview fields.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

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
    for (path, engine, profile_id) in session_file_entries(store)? {
        if let Some(s) = session_from_file(&path, engine, &profile_id) {
            out.push(s);
        }
    }
    out.sort_by(|a, b| b.modified.cmp(&a.modified));
    Ok(out)
}

/// Find one session by id across all homes.
///
/// The id the UI reports is normally the transcript's file name (Claude:
/// `<session-id>.jsonl`; Codex: the trailing uuid of the rollout stem), so we
/// first narrow to the file whose NAME already matches and parse only that —
/// instead of re-reading up to 400 head lines of every session in the pool on
/// each pin / resume / title / delete. If no name-matching file parses to the
/// requested id (a rare content-derived-id mismatch) we fall back to the full
/// scan so the answer is identical to what the pool itself lists.
pub fn find_session(store: &ProfileStore, session_id: &str) -> Result<Session> {
    for (path, engine, profile_id) in session_file_entries(store)? {
        let name_matches = match engine {
            Engine::Claude => file_stem(&path) == session_id,
            Engine::Codex => rollout_session_id(&file_stem(&path)) == session_id,
        };
        if !name_matches {
            continue;
        }
        if let Some(s) = session_from_file(&path, engine, &profile_id) {
            if s.session_id == session_id {
                return Ok(s);
            }
        }
    }
    for s in list_sessions(store)? {
        if s.session_id == session_id {
            return Ok(s);
        }
    }
    Err(Error::Other(format!("session not found: {session_id}")))
}

/// Enumerate every session file as `(path, engine, profile_id)` without
/// reading any transcript content. `list_sessions` maps these through
/// `session_from_file`; `find_session` uses them to narrow to the one file
/// whose name matches before paying for a full parse.
fn session_file_entries(store: &ProfileStore) -> Result<Vec<(PathBuf, Engine, String)>> {
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
                out.push((path, engine, profile_id.clone()));
            }
        }
    }
    Ok(out)
}

/// Parse one transcript file into a `Session`, filling in the profile id,
/// engine and file mtime. Returns `None` for an unparseable file.
fn session_from_file(path: &Path, engine: Engine, profile_id: &str) -> Option<Session> {
    let values = read_head_lines(path);
    let session = match engine {
        Engine::Claude => parse_claude_session(path, &values),
        Engine::Codex => parse_codex_session(path, &values),
    }?;
    let mut s = session;
    s.profile_id = profile_id.to_string();
    s.engine = engine;
    s.modified = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH);
    Some(s)
}

// ---------------------------------------------------------------------------
// Session meta: user-side state (pin order + custom titles) for the pool.
// Kept in one small JSON file at the config root — the transcript files
// themselves are never rewritten by these operations.
// ---------------------------------------------------------------------------

/// Pinned session ids (most recently pinned first) and custom titles.
#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct SessionMeta {
    /// Pinned session ids, most recently pinned first.
    pub pinned: Vec<String>,
    /// Custom display titles keyed by session id.
    pub titles: BTreeMap<String, String>,
}

impl SessionMeta {
    pub fn is_pinned(&self, id: &str) -> bool {
        self.pinned.iter().any(|p| p == id)
    }
}

/// `<config-root>/sessions-meta.json`
pub fn meta_path(store: &ProfileStore) -> PathBuf {
    store.root.join("sessions-meta.json")
}

/// Load session meta; a missing or corrupt file yields the default.
pub fn load_meta(store: &ProfileStore) -> SessionMeta {
    std::fs::read_to_string(meta_path(store))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_meta(store: &ProfileStore, meta: &SessionMeta) -> Result<()> {
    let json = serde_json::to_string_pretty(meta)
        .map_err(|e| Error::Other(format!("failed to serialize session meta: {e}")))?;
    let path = meta_path(store);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| Error::Other(format!("failed to create config dir: {e}")))?;
    }
    std::fs::write(path, json)
        .map_err(|e| Error::Other(format!("failed to write session meta: {e}")))?;
    Ok(())
}

/// Pin (most recently pinned first) or unpin a session.
pub fn set_pinned(store: &ProfileStore, session_id: &str, pinned: bool) -> Result<()> {
    let mut meta = load_meta(store);
    meta.pinned.retain(|p| p != session_id);
    if pinned {
        meta.pinned.insert(0, session_id.to_string());
    }
    save_meta(store, &meta)
}

/// Set a session's custom title; an empty title clears it.
pub fn set_title(store: &ProfileStore, session_id: &str, title: &str) -> Result<()> {
    let mut meta = load_meta(store);
    let title = title.trim().to_string();
    if title.is_empty() {
        meta.titles.remove(session_id);
    } else {
        meta.titles.insert(session_id.to_string(), title);
    }
    save_meta(store, &meta)
}

// ---------------------------------------------------------------------------
// "Open" state: while the CLI process launched for a session is still
// running, the session is OPEN and must not be resumed again — a second
// process writing the same transcript would corrupt it.
//
// Detection: every launch that resumes a session carries the session id in
// the launched process command line (GUI resume writes a per-session
// `resume-<id>.ps1/.sh` start script, CLI resume passes the id as an
// argument). A lock file at `<root>/sessions-open/<id>.json` is written at
// resume time; the session counts as open while either a process command
// line still contains the id, or the lock is younger than OPEN_GRACE (the
// few seconds between the terminal spawn and the CLI actually starting).
// ---------------------------------------------------------------------------

/// A lock younger than this counts as open even before the process scan
/// can see the launched CLI.
const OPEN_GRACE: Duration = Duration::from_secs(60);

/// `<root>/sessions-open/<session-id>.json`
pub fn open_lock_path(store: &ProfileStore, session_id: &str) -> PathBuf {
    store.root.join("sessions-open").join(format!("{session_id}.json"))
}

/// Record that a session was just opened. Fails when the lock already
/// exists — the atomic create_new is what rejects a concurrent second
/// resume (the caller must have verified the session is closed first).
pub fn mark_session_open(store: &ProfileStore, session_id: &str) -> Result<()> {
    let path = open_lock_path(store, session_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let opened_at = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let json = serde_json::json!({ "opened_at": opened_at });
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|e| Error::Other(format!("session already open: {e}")))?;
    f.write_all(serde_json::to_string(&json).unwrap().as_bytes())?;
    Ok(())
}

/// Remove the open lock (best effort).
pub fn clear_open_lock(store: &ProfileStore, session_id: &str) {
    let _ = std::fs::remove_file(open_lock_path(store, session_id));
}

/// True while a launched process for this session is still running.
pub fn session_is_open(store: &ProfileStore, session_id: &str) -> bool {
    open_session_ids(store).contains(session_id)
}

/// Every session with a lock that is still live; dead locks are pruned.
pub fn open_session_ids(store: &ProfileStore) -> std::collections::HashSet<String> {
    let dir = store.root.join("sessions-open");
    let mut out: std::collections::HashSet<String> = std::collections::HashSet::new();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return out;
    };
    let mut candidates: Vec<(String, SystemTime)> = Vec::new();
    for e in entries.flatten() {
        let name = e.file_name();
        let Some(stem) = name.to_str() else {
            continue;
        };
        let Some(id) = stem.strip_suffix(".json") else {
            continue;
        };
        let mtime = e
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        candidates.push((id.to_string(), mtime));
    }
    if candidates.is_empty() {
        return out;
    }
    // One process scan answers for all candidates.
    let cmdlines = process_command_lines();
    let now = SystemTime::now();
    for (id, mtime) in candidates {
        let live = cmdlines.iter().any(|cl| cl.contains(&id))
            || now
                .duration_since(mtime)
                .map(|age| age < OPEN_GRACE)
                .unwrap_or(true);
        if live {
            out.insert(id.clone());
        } else {
            // The CLI is gone: release the lock so the session can reopen.
            let _ = std::fs::remove_file(open_lock_path(store, &id));
        }
    }
    out
}

/// Command line of every running process (empty when the scan fails —
/// the mtime grace rule then keeps the state safe, it just lags).
fn process_command_lines() -> Vec<String> {
    #[cfg(windows)]
    {
        proc_cmdlines::snapshot()
            .into_iter()
            .map(|(_, cl)| cl)
            .collect()
    }
    #[cfg(not(windows))]
    {
        let Ok(out) = std::process::Command::new("ps").args(["-eo", "args="]).output()
        else {
            return Vec::new();
        };
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(|l| l.to_string())
            .filter(|l| !l.trim().is_empty())
            .collect()
    }
}

#[cfg(windows)]
mod proc_cmdlines {
    //! Every readable (pid, command-line-ish string) pair: a toolhelp
    //! snapshot for the pids, then a PEB read per process
    //! (NtQueryInformationProcess → PEB → RTL_USER_PROCESS_PARAMETERS).
    //! A few ms, no subprocess. x86_64 offsets (the only supported
    //! Windows target).
    //!
    //! The UNICODE_STRING field that holds the full command line moved
    //! between Windows builds (verified: 0x60 on older builds, 0x70 on
    //! Windows 11 24H2+), so the neighbouring string fields are all read
    //! and returned — the caller matches a long, unique session id
    //! against any of them, which is what matters for the open-detection.

    use std::ffi::c_void;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::Diagnostics::Debug::ReadProcessMemory;
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ,
    };

    extern "system" {
        fn NtQueryInformationProcess(
            process_handle: HANDLE,
            process_information_class: u32,
            process_information: *mut c_void,
            return_length: u32,
            return_length_out: *mut u32,
        ) -> i32;
    }

    #[repr(C)]
    struct ProcessBasicInfo {
        reserved1: usize,
        peb_base_address: *mut c_void,
        reserved2: [usize; 2],
        unique_process_id: usize,
        reserved3: usize,
    }

    const PROCESS_BASIC_INFORMATION: u32 = 0;
    const PEB_PROCESS_PARAMETERS: usize = 0x20; // PEB->ProcessParameters
    /// The UNICODE_STRING fields inspected inside
    /// RTL_USER_PROCESS_PARAMETERS (x86_64). Across Windows builds the
    /// full command line lives at 0x60 or 0x70; 0x50 covers the older
    /// ImagePathName placement. Each entry is a different string on any
    /// build (image path / command line / dll path), so matching a
    /// session id against all of them is safe.
    const PP_STRING_FIELDS: &[usize] = &[0x50, 0x60, 0x70];

    pub(super) fn snapshot() -> Vec<(u32, String)> {
        let mut out = Vec::new();
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snap.is_null() {
                return out;
            }
            let mut entry: PROCESSENTRY32W = std::mem::zeroed();
            entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
            let mut first = true;
            loop {
                let ok = if first {
                    first = false;
                    Process32FirstW(snap, &mut entry)
                } else {
                    Process32NextW(snap, &mut entry)
                };
                if ok == 0 {
                    break;
                }
                let pid = entry.th32ProcessID;
                let h = OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, 0, pid);
                if !h.is_null() {
                    out.extend(command_lines_of(h).into_iter().map(|cl| (pid, cl)));
                    CloseHandle(h);
                }
            }
            CloseHandle(snap);
        }
        out
    }

    /// The readable command-line-ish strings of one process (usually 1-3:
    /// image path, full command line, and maybe a dll path).
    unsafe fn command_lines_of(h: HANDLE) -> Vec<String> {
        let mut out = Vec::new();
        let mut pbi: ProcessBasicInfo = std::mem::zeroed();
        if NtQueryInformationProcess(
            h,
            PROCESS_BASIC_INFORMATION,
            &mut pbi as *mut _ as *mut c_void,
            std::mem::size_of::<ProcessBasicInfo>() as u32,
            &mut 0u32,
        ) != 0
        {
            return out;
        }
        let peb = pbi.peb_base_address as usize;
        if peb == 0 {
            return out;
        }
        let mut pp_ptr: usize = 0;
        if read_mem(
            h,
            peb + PEB_PROCESS_PARAMETERS,
            &mut pp_ptr as *mut _ as *mut c_void,
            std::mem::size_of::<usize>(),
        ) == 0
        {
            return out;
        }
        if pp_ptr == 0 {
            return out;
        }
        for off in PP_STRING_FIELDS {
            if let Some(s) = unicode_string_of(h, pp_ptr + off) {
                out.push(s);
            }
        }
        out
    }

    /// Read the UNICODE_STRING at `addr` (u16 Length @+0, u16 MaxLength
    /// @+2, u32 pad @+4, *u16 Buffer @+8); None when unreadable, empty,
    /// or not printable ASCII (a misaligned field would decode to noise).
    unsafe fn unicode_string_of(h: HANDLE, addr: usize) -> Option<String> {
        let mut length: u16 = 0;
        let mut buffer: usize = 0;
        if read_mem(h, addr, &mut length as *mut _ as *mut c_void, std::mem::size_of::<u16>()) == 0
            || read_mem(
                h,
                addr + 8,
                &mut buffer as *mut _ as *mut c_void,
                std::mem::size_of::<usize>(),
            ) == 0
        {
            return None;
        }
        if length == 0 || buffer == 0 || (length as usize) > 0x4000 {
            return None;
        }
        let mut buf = vec![0u8; length as usize];
        if read_mem(h, buffer, buf.as_mut_ptr() as *mut c_void, buf.len()) == 0 {
            return None;
        }
        let wide: Vec<u16> =
            std::slice::from_raw_parts(buf.as_ptr() as *const u16, buf.len() / 2).to_vec();
        let s = String::from_utf16_lossy(&wide);
        if s.is_empty() || !s.chars().all(|c| c.is_ascii_graphic() || c.is_ascii_whitespace()) {
            return None;
        }
        Some(s)
    }

    /// Read `size` bytes at `addr`; returns bytes read (0 on failure).
    unsafe fn read_mem(h: HANDLE, addr: usize, out: *mut c_void, size: usize) -> usize {
        let mut read = 0usize;
        if ReadProcessMemory(h, addr as *const c_void, out, size, &mut read) == 0 {
            0
        } else {
            read
        }
    }
}

/// Delete a session's transcript file(s) from the profile homes and drop
/// its meta entries. Only files under `runtime/` whose name matches the
/// session id are ever removed; returns how many files were deleted.
pub fn delete_session(store: &ProfileStore, session_id: &str) -> Result<usize> {
    // A deleted session can never be resumed again, so drop its open lock
    // even when the transcript files are already gone.
    clear_open_lock(store, session_id);
    let root = store.runtime_dir();
    let mut removed = 0usize;
    if root.is_dir() {
        for entry in std::fs::read_dir(&root)?.flatten() {
            let home_root = entry.path();
            if !home_root.is_dir() {
                continue;
            }
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
                    let stem = file_stem(&path);
                    let matches = match engine {
                        // Claude transcripts are named exactly `<session-id>.jsonl`.
                        Engine::Claude => stem == session_id,
                        // Codex rollouts are named `rollout-…-<session-id>.jsonl`;
                        // a dashed uuid does not survive `rollout_session_id`
                        // (it takes the last '-' segment), so also match the
                        // `-{id}` suffix directly.
                        Engine::Codex => {
                            stem == session_id
                                || stem.ends_with(&format!("-{session_id}"))
                                || rollout_session_id(&stem) == session_id
                        }
                    };
                    if matches {
                        std::fs::remove_file(&path).map_err(|e| {
                            Error::Other(format!(
                                "failed to delete {}: {e}",
                                path.display()
                            ))
                        })?;
                        removed += 1;
                    }
                }
            }
        }
    }
    if removed == 0 {
        return Err(Error::Other(format!("session file not found: {session_id}")));
    }
    // Drop stale meta so the state file does not grow unbounded.
    let mut meta = load_meta(store);
    meta.pinned.retain(|p| p != session_id);
    meta.titles.remove(session_id);
    save_meta(store, &meta)?;
    Ok(removed)
}

/// Delete every session that is neither pinned nor currently open, in a
/// single pass over the runtime homes (unlike `delete_session`, which is
/// per-id and re-scans the homes each call). Pinned sessions and sessions
/// with a live open lock are skipped; individual removal failures (e.g. a
/// file locked by a process we could not see) are logged and skipped.
/// Returns how many sessions were removed.
pub fn clear_unpinned_sessions(store: &ProfileStore) -> Result<usize> {
    let meta = load_meta(store);
    let open = open_session_ids(store);
    let keep: std::collections::HashSet<&str> = meta.pinned.iter().map(|s| s.as_str()).collect();

    let root = store.runtime_dir();
    let mut removed: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut failed = 0usize;
    if root.is_dir() {
        for entry in std::fs::read_dir(&root)?.flatten() {
            let home_root = entry.path();
            if !home_root.is_dir() {
                continue;
            }
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
                    let id = match engine {
                        // Claude transcripts are named exactly `<id>.jsonl`.
                        Engine::Claude => file_stem(&path),
                        Engine::Codex => rollout_session_id(&file_stem(&path)),
                    };
                    if keep.contains(id.as_str()) || open.contains(&id) {
                        continue;
                    }
                    if let Err(e) = std::fs::remove_file(&path) {
                        failed += 1;
                        crate::logging::warn(&format!(
                            "clear_unpinned: failed to delete {}: {e}",
                            path.display()
                        ));
                        continue;
                    }
                    removed.insert(id);
                }
            }
        }
    }
    if failed > 0 {
        crate::logging::warn(&format!(
            "clear_unpinned: {failed} file(s) could not be deleted and were skipped"
        ));
    }
    // Drop the custom titles of the removed sessions; pinned ids can never
    // have been removed (they were kept), so the pin list is left alone.
    if !removed.is_empty() {
        let mut meta = load_meta(store);
        meta.titles.retain(|k, _| !removed.contains(k));
        save_meta(store, &meta)?;
    }
    Ok(removed.len())
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

    /// Create one claude transcript in a temp store; returns its id.
    fn seed_claude_session(store: &ProfileStore, profile: &str, id: &str) {
        let dir = store.runtime_dir().join(profile).join(".claude").join("projects");
        let proj = dir.join("C--Users-demo");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(
            proj.join(format!("{id}.jsonl")),
            format!(
                r#"{{"type":"user","sessionId":"{id}","cwd":"C:\\Users\\demo","message":{{"role":"user","content":"hi"}}}}"#
            ),
        )
        .unwrap();
    }

    #[test]
    fn session_meta_round_trip() {
        let root = temp_root();
        let store = ProfileStore { root: root.clone() };
        assert!(load_meta(&store).pinned.is_empty());

        set_title(&store, "s1", "  my title  ").unwrap();
        set_pinned(&store, "s1", true).unwrap();
        set_pinned(&store, "s2", true).unwrap(); // s2 most recently pinned
        let meta = load_meta(&store);
        assert_eq!(meta.titles.get("s1").map(String::as_str), Some("my title"));
        assert_eq!(meta.pinned, vec!["s2".to_string(), "s1".to_string()]);
        assert!(meta.is_pinned("s1"));

        set_pinned(&store, "s1", false).unwrap();
        set_title(&store, "s1", "   ").unwrap(); // empty = clear
        let meta = load_meta(&store);
        assert_eq!(meta.pinned, vec!["s2".to_string()]);
        assert!(!meta.titles.contains_key("s1"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn session_meta_survives_corrupt_file() {
        let root = temp_root();
        let store = ProfileStore { root: root.clone() };
        std::fs::create_dir_all(&store.root).unwrap();
        std::fs::write(meta_path(&store), "not json {").unwrap();
        let meta = load_meta(&store);
        assert!(meta.pinned.is_empty());
        // Saving over a corrupt file must succeed and produce valid JSON.
        set_pinned(&store, "x", true).unwrap();
        assert!(load_meta(&store).is_pinned("x"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn delete_session_removes_claude_transcript_and_meta() {
        let root = temp_root();
        let store = ProfileStore { root: root.clone() };
        seed_claude_session(&store, "p1", "aaa-111");
        set_pinned(&store, "aaa-111", true).unwrap();
        set_title(&store, "aaa-111", "keep me?").unwrap();

        let removed = delete_session(&store, "aaa-111").unwrap();
        assert_eq!(removed, 1);
        assert!(find_session(&store, "aaa-111").is_err());
        let meta = load_meta(&store);
        assert!(!meta.is_pinned("aaa-111"));
        assert!(!meta.titles.contains_key("aaa-111"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn delete_session_removes_codex_rollout() {
        let root = temp_root();
        let store = ProfileStore { root: root.clone() };
        let dir = store
            .runtime_dir()
            .join("gpt")
            .join(".codex")
            .join("sessions")
            .join("2026")
            .join("09")
            .join("10");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("rollout-2026-09-10T10-00-00-77777777-aaaa-bbbb-cccc-000000000000.jsonl"),
            r#"{"type":"session_meta","payload":{"id":"77777777-aaaa-bbbb-cccc-000000000000","cwd":"/w"}}"#,
        )
        .unwrap();
        // An unrelated rollout must survive.
        std::fs::write(dir.join("rollout-2026-09-10T11-00-00-88888888-aaaa-bbbb-cccc-000000000000.jsonl"), "{}")
            .unwrap();

        let removed = delete_session(&store, "77777777-aaaa-bbbb-cccc-000000000000").unwrap();
        assert_eq!(removed, 1);
        let remaining: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|f| f.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(remaining.len(), 1);
        assert!(remaining[0].contains("88888888"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn delete_session_unknown_id_errors() {
        let root = temp_root();
        let store = ProfileStore { root: root.clone() };
        std::fs::create_dir_all(&store.runtime_dir()).unwrap();
        assert!(delete_session(&store, "nope").is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn clear_unpinned_keeps_pinned_and_open() {
        let root = temp_root();
        let store = ProfileStore { root: root.clone() };
        seed_claude_session(&store, "p1", "keep-pinned");
        seed_claude_session(&store, "p1", "keep-open");
        seed_claude_session(&store, "p1", "gone-1");
        seed_claude_session(&store, "p1", "gone-2");
        set_pinned(&store, "keep-pinned", true).unwrap();
        set_title(&store, "gone-1", "a title").unwrap();
        mark_session_open(&store, "keep-open").unwrap();

        let removed = clear_unpinned_sessions(&store).unwrap();
        assert_eq!(removed, 2);
        assert!(find_session(&store, "keep-pinned").is_ok());
        assert!(find_session(&store, "keep-open").is_ok());
        assert!(find_session(&store, "gone-1").is_err());
        assert!(find_session(&store, "gone-2").is_err());
        let meta = load_meta(&store);
        assert!(!meta.titles.contains_key("gone-1"));
        assert!(meta.is_pinned("keep-pinned"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn clear_unpinned_nothing_to_clear() {
        let root = temp_root();
        let store = ProfileStore { root: root.clone() };
        std::fs::create_dir_all(&store.runtime_dir()).unwrap();
        assert_eq!(clear_unpinned_sessions(&store).unwrap(), 0);
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

    #[test]
    fn open_lock_lifecycle() {
        let root = temp_root();
        let store = ProfileStore { root: root.clone() };
        assert!(!session_is_open(&store, "abc"));
        mark_session_open(&store, "abc").unwrap();
        assert!(session_is_open(&store, "abc"));
        // The atomic create_new rejects a concurrent second open.
        assert!(mark_session_open(&store, "abc").is_err());
        assert!(open_session_ids(&store).contains("abc"));
        clear_open_lock(&store, "abc");
        assert!(!session_is_open(&store, "abc"));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Live Windows test: a process whose command line carries a session
    /// id makes that session "open" even when the lock is older than the
    /// grace window (the backdated lock proves the process scan — not the
    /// mtime rule — kept it open); a backdated lock with no matching
    /// process is pruned.
    #[cfg(windows)]
    #[test]
    fn open_scan_sees_live_process_and_prunes_dead() {
        let root = temp_root();
        let store = ProfileStore { root: root.clone() };
        let live_id = format!("as-live-{}", uuid::Uuid::new_v4());
        let scan_id = format!("as-scan-{}", uuid::Uuid::new_v4());
        let dead_id = format!("as-dead-{}", uuid::Uuid::new_v4());

        // A long-lived cmd.exe whose command line carries both markers.
        let mut child = std::process::Command::new("cmd.exe")
            .arg("/C")
            .arg(format!("ping -n 60 127.0.0.1 >nul & rem {live_id} {scan_id}"))
            .spawn()
            .expect("spawn cmd.exe");

        let dir = store.root.join("sessions-open");
        std::fs::create_dir_all(&dir).unwrap();
        // Fresh lock: open either way (cmdline OR grace window).
        std::fs::write(dir.join(format!("{live_id}.json")), "{}").unwrap();
        // Backdated locks: only the cmdline rule can keep them open.
        for id in [&scan_id, &dead_id] {
            let p = dir.join(format!("{id}.json"));
            std::fs::write(&p, "{}").unwrap();
            let f = std::fs::File::options().write(true).open(&p).unwrap();
            f.set_modified(SystemTime::now() - Duration::from_secs(120))
                .unwrap();
        }

        // The snapshot can lag the spawn: retry a few times.
        let mut open = open_session_ids(&store);
        for _ in 0..5 {
            if open.contains(&scan_id) {
                break;
            }
            std::thread::sleep(Duration::from_millis(500));
            open = open_session_ids(&store);
        }
        assert!(open.contains(&live_id), "live lock lost: {open:?}");
        assert!(
            open.contains(&scan_id),
            "backdated lock only stays open via the process scan: {open:?}"
        );
        assert!(!open.contains(&dead_id), "dead lock not pruned: {open:?}");
        assert!(!dir.join(format!("{dead_id}.json")).exists());

        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(&root);
    }


}
