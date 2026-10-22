//! Token usage and prefix-cache statistics for the session pool.
//!
//! Claude Code writes one JSONL line per assistant message; every line
//! carries \`message.usage\` (input / output / cache read / cache write).
//! Streaming can split one API response over several lines (one per content
//! block), so lines are de-duplicated by \`message.id\` — without that, a
//! 90 MB transcript reports ~9x the real token count.
//!
//! Codex rollouts record cumulative \`token_count\` events; the last event is
//! the session total and each event's \`last_token_usage\` is one turn.
//!
//! A process-wide cache keyed by (path, mtime, size) keeps the 4 s Sessions
//! poll and the Usage page from re-reading multi-megabyte transcripts.

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::Value;

use crate::engine::Engine;
use crate::error::Result;
use crate::profile_store::ProfileStore;

/// Only the newest N turns travel to the UI (the totals still cover all).
const MAX_TURNS: usize = 200;

#[derive(Debug, Clone, Serialize)]
pub struct TurnUsage {
    /// Unix seconds (0 when the record has no parseable timestamp).
    pub timestamp: i64,
    /// Prompt tokens that were not served from cache (Anthropic semantics).
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    /// True when the gateway actually included cache fields (even if zero).
    pub cache_reported: bool,
    pub model: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionUsage {
    pub session_id: String,
    pub profile_id: String,
    pub engine: String,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    /// True when at least one turn carried cache fields.
    pub cache_reported: bool,
    /// input + cache_creation + cache_read (Claude) or input (Codex).
    pub total_prompt_tokens: u64,
    /// cache_read / total_prompt_tokens, 0..1.
    pub cache_hit_rate: f64,
    pub turns: u32,
    /// File mtime, unix seconds.
    pub modified: i64,
    /// Newest turns, oldest first.
    pub per_turn: Vec<TurnUsage>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct ServerCacheStats {
    pub ok: bool,
    pub message: String,
    pub endpoint: String,
    pub hits: f64,
    pub queries: f64,
    /// hits / queries, 0..1.
    pub hit_rate: f64,
    pub kv_usage: f64,
    pub running: f64,
}

struct CacheEntry {
    mtime: SystemTime,
    size: u64,
    usage: SessionUsage,
}

fn file_cache() -> &'static Mutex<HashMap<PathBuf, CacheEntry>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, CacheEntry>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Usage for every transcript in the per-profile runtime homes, newest first.
/// Files without any assistant usage are skipped (the pool still lists them).
pub fn list_usage(store: &ProfileStore) -> Vec<SessionUsage> {
    let entries = crate::sessions::session_file_entries(store).unwrap_or_default();
    let mut out = Vec::new();
    for (path, engine, profile_id) in entries {
        if let Some(usage) = usage_for_file(&path, engine, &profile_id) {
            out.push(usage);
        }
    }
    out.sort_by(|a, b| b.modified.cmp(&a.modified));
    out
}

fn usage_for_file(path: &Path, engine: Engine, profile_id: &str) -> Option<SessionUsage> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta.modified().unwrap_or(UNIX_EPOCH);
    let size = meta.len();

    if let Ok(cache) = file_cache().lock() {
        if let Some(entry) = cache.get(path) {
            if entry.mtime == mtime && entry.size == size {
                return Some(entry.usage.clone());
            }
        }
    }

    let mut usage = match engine {
        Engine::Claude => scan_claude(path, Some(MAX_TURNS)),
        Engine::Codex => scan_codex(path, Some(MAX_TURNS)),
    }?;
    if usage.turns == 0 {
        return None;
    }
    usage.profile_id = profile_id.to_string();
    usage.engine = engine.value().to_string();
    usage.modified = mtime
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    if let Ok(mut cache) = file_cache().lock() {
        cache.insert(
            path.to_path_buf(),
            CacheEntry {
                mtime,
                size,
                usage: usage.clone(),
            },
        );
    }
    Some(usage)
}

#[derive(Debug, Serialize)]
pub struct CsvExport {
    pub path: String,
    pub rows: usize,
    pub sessions: usize,
}

/// Write one row per assistant request / Codex token_count event across
/// every session. Unlike the UI payload this keeps every turn (the page only
/// ships the newest MAX_TURNS for latency reasons).
pub fn export_csv(store: &ProfileStore, path: &Path) -> Result<CsvExport> {
    let entries = crate::sessions::session_file_entries(store)?;
    let meta = crate::sessions::load_meta(store);
    let profiles = store.list().unwrap_or_default();
    let names: HashMap<&str, &str> = profiles
        .iter()
        .map(|p| (p.id.as_str(), p.name.as_str()))
        .collect();

    let mut lines: Vec<(i64, String)> = Vec::new();
    let mut sessions = 0usize;
    for (file, engine, profile_id) in entries {
        let usage = match engine {
            Engine::Claude => scan_claude(&file, None),
            Engine::Codex => scan_codex(&file, None),
        };
        let Some(usage) = usage else { continue };
        if usage.per_turn.is_empty() {
            continue;
        }
        sessions += 1;
        let title = meta
            .titles
            .get(&usage.session_id)
            .cloned()
            .unwrap_or_default();
        let profile_name = names
            .get(profile_id.as_str())
            .copied()
            .unwrap_or("")
            .to_string();
        for (index, turn) in usage.per_turn.iter().enumerate() {
            let total =
                turn.input_tokens + turn.cache_creation_tokens + turn.cache_read_tokens;
            let rate = if total > 0 {
                turn.cache_read_tokens as f64 / total as f64
            } else {
                0.0
            };
            let line = format!(
                "{},{},{},{},{},{},{},{},{},{},{},{},{},{:.4}",
                csv_escape(&iso_ts(turn.timestamp)),
                csv_escape(&usage.session_id),
                csv_escape(&title),
                csv_escape(&profile_id),
                csv_escape(&profile_name),
                csv_escape(engine.value()),
                csv_escape(&turn.model),
                index + 1,
                turn.input_tokens,
                turn.cache_read_tokens,
                turn.cache_creation_tokens,
                turn.output_tokens,
                total,
                rate
            );
            lines.push((turn.timestamp, line));
        }
    }
    lines.sort_by_key(|(ts, _)| *ts);

    let mut out = String::from(
        "\u{FEFF}timestamp,session_id,session_title,profile_id,profile_name,engine,model,request_index,input_tokens,cache_read_tokens,cache_creation_tokens,output_tokens,total_prompt_tokens,cache_hit_rate\n",
    );
    for (_, line) in &lines {
        out.push_str(line);
        out.push('\n');
    }
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    std::fs::write(path, out)?;
    Ok(CsvExport {
        path: path.to_string_lossy().into_owned(),
        rows: lines.len(),
        sessions,
    })
}

fn iso_ts(ts: i64) -> String {
    if ts <= 0 {
        return String::new();
    }
    chrono::DateTime::from_timestamp(ts, 0)
        .map(|d| d.to_rfc3339())
        .unwrap_or_default()
}

fn csv_escape(value: &str) -> String {
    if value.chars().any(|c| matches!(c, ',' | '"' | '\n' | '\r')) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

fn scan_claude(path: &Path, max_turns: Option<usize>) -> Option<SessionUsage> {
    let file = std::fs::File::open(path).ok()?;
    let reader = BufReader::new(file);

    let mut seen: HashMap<String, TurnUsage> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    let mut model_counts: HashMap<String, u32> = HashMap::new();
    let mut session_id = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut session_id_seen = false;

    for (n, line) in reader.lines().enumerate() {
        let Ok(line) = line else { continue };
        // Cheap pre-filter: only assistant records carry usage.
        if !line.contains("\"usage\"") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        if v.get("type").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        if !session_id_seen {
            if let Some(sid) = v.get("sessionId").and_then(Value::as_str) {
                session_id = sid.to_string();
                session_id_seen = true;
            }
        }
        let Some(msg) = v.get("message") else { continue };
        let Some(u) = msg.get("usage").filter(|u| u.is_object()) else {
            continue;
        };

        let id = msg
            .get("id")
            .and_then(Value::as_str)
            .or_else(|| v.get("requestId").and_then(Value::as_str))
            .or_else(|| v.get("uuid").and_then(Value::as_str))
            .map(str::to_string)
            .unwrap_or_else(|| format!("{session_id}-{n}"));
        let ts = v
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(parse_ts)
            .unwrap_or(0);
        let model = msg
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if !model.is_empty() {
            *model_counts.entry(model.clone()).or_insert(0) += 1;
        }

        let cache_reported = u.get("cache_read_input_tokens").is_some()
            || u.get("cache_creation_input_tokens").is_some();
        let turn = TurnUsage {
            timestamp: ts,
            input_tokens: u64_field(u, "input_tokens"),
            output_tokens: u64_field(u, "output_tokens"),
            cache_read_tokens: u64_field(u, "cache_read_input_tokens"),
            cache_creation_tokens: u64_field(u, "cache_creation_input_tokens"),
            cache_reported,
            model,
        };
        if turn.input_tokens == 0
            && turn.output_tokens == 0
            && turn.cache_read_tokens == 0
            && turn.cache_creation_tokens == 0
        {
            continue;
        }
        if !seen.contains_key(&id) {
            order.push(id.clone());
        }
        // Streaming duplicates the same message id with identical usage;
        // the last copy wins.
        seen.insert(id, turn);
    }

    let mut turns: Vec<TurnUsage> = order
        .into_iter()
        .filter_map(|id| seen.remove(&id))
        .collect();
    if turns.is_empty() {
        return None;
    }
    turns.sort_by_key(|t| t.timestamp);

    let input_tokens: u64 = turns.iter().map(|t| t.input_tokens).sum();
    let output_tokens: u64 = turns.iter().map(|t| t.output_tokens).sum();
    let cache_read_tokens: u64 = turns.iter().map(|t| t.cache_read_tokens).sum();
    let cache_creation_tokens: u64 = turns.iter().map(|t| t.cache_creation_tokens).sum();
    let total_prompt_tokens = input_tokens + cache_creation_tokens + cache_read_tokens;
    let cache_hit_rate = if total_prompt_tokens > 0 {
        cache_read_tokens as f64 / total_prompt_tokens as f64
    } else {
        0.0
    };
    let model = model_counts
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .map(|(m, _)| m)
        .unwrap_or_default();
    let turn_count = turns.len() as u32;
    let cache_reported = turns.iter().any(|t| t.cache_reported);
    let per_turn = match max_turns {
        Some(max) if turns.len() > max => turns[turns.len() - max..].to_vec(),
        _ => turns,
    };

    Some(SessionUsage {
        session_id,
        profile_id: String::new(),
        engine: String::new(),
        model,
        input_tokens,
        output_tokens,
        cache_read_tokens,
        cache_creation_tokens,
        cache_reported,
        total_prompt_tokens,
        cache_hit_rate,
        turns: turn_count,
        modified: 0,
        per_turn,
    })
}

fn scan_codex(path: &Path, max_turns: Option<usize>) -> Option<SessionUsage> {
    let file = std::fs::File::open(path).ok()?;
    let reader = BufReader::new(file);

    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    // rollout-<timestamp>-<uuid>: keep the trailing uuid when present.
    let mut session_id = if stem.len() >= 36 {
        stem.get(stem.len() - 36..).unwrap_or(&stem).to_string()
    } else {
        stem
    };
    let mut model = String::new();
    let mut total: Option<(u64, u64, u64)> = None;
    let mut turns: Vec<TurnUsage> = Vec::new();

    for line in reader.lines().flatten() {
        if !line.contains("token_count")
            && !line.contains("turn_context")
            && !line.contains("session_meta")
        {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        let ty = v.get("type").and_then(Value::as_str).unwrap_or("");
        if ty == "session_meta" {
            if let Some(id) = v.pointer("/payload/id").and_then(Value::as_str) {
                session_id = id.to_string();
            }
        }
        if ty == "turn_context" {
            if let Some(m) = v.pointer("/payload/model").and_then(Value::as_str) {
                model = m.to_string();
            }
        }
        if ty != "event_msg" {
            continue;
        }
        if v.pointer("/payload/type").and_then(Value::as_str) != Some("token_count") {
            continue;
        }
        let info = &v["payload"]["info"];
        let total_usage = &info["total_token_usage"];
        if total_usage.is_object() {
            total = Some((
                u64_field(total_usage, "input_tokens"),
                u64_field(total_usage, "cached_input_tokens"),
                u64_field(total_usage, "output_tokens"),
            ));
        }
        let last = &info["last_token_usage"];
        if last.is_object() {
            let ts = v
                .get("timestamp")
                .and_then(Value::as_str)
                .and_then(parse_ts)
                .unwrap_or(0);
            // OpenAI-style input_tokens include the cached part; store the
            // uncached remainder so the UI can add cache_read consistently
            // with the Claude parser.
            let last_input = u64_field(last, "input_tokens");
            let last_cached = u64_field(last, "cached_input_tokens");
            let turn = TurnUsage {
                timestamp: ts,
                input_tokens: last_input.saturating_sub(last_cached),
                output_tokens: u64_field(last, "output_tokens"),
                cache_read_tokens: last_cached,
                cache_creation_tokens: 0,
                cache_reported: last.get("cached_input_tokens").is_some(),
                model: model.clone(),
            };
            if turn.input_tokens == 0
                && turn.output_tokens == 0
                && turn.cache_read_tokens == 0
                && turn.cache_creation_tokens == 0
            {
                continue;
            }
            turns.push(turn);
        }
    }

    let (input, cached, output) = total?;
    // OpenAI-style input_tokens already include the cached part.
    let total_prompt_tokens = input.max(cached);
    let cache_hit_rate = if total_prompt_tokens > 0 {
        cached as f64 / total_prompt_tokens as f64
    } else {
        0.0
    };
    let turn_count = turns.len().max(1) as u32;
    let cache_reported = turns.iter().any(|t| t.cache_reported);
    if let Some(max) = max_turns {
        if turns.len() > max {
            turns = turns[turns.len() - max..].to_vec();
        }
    }

    Some(SessionUsage {
        session_id,
        profile_id: String::new(),
        engine: String::new(),
        model,
        input_tokens: input.saturating_sub(cached),
        output_tokens: output,
        cache_read_tokens: cached,
        cache_creation_tokens: 0,
        cache_reported,
        total_prompt_tokens,
        cache_hit_rate,
        turns: turn_count,
        modified: 0,
        per_turn: turns,
    })
}

fn u64_field(v: &Value, key: &str) -> u64 {
    v.get(key).and_then(Value::as_u64).unwrap_or(0)
}

fn parse_ts(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.timestamp())
}

// ---------------------------------------------------------------------------
// vLLM /metrics: the only trustworthy cache number for a local gateway.
// The Anthropic-compatible endpoint does not return cache_read tokens, so
// the transcript-derived rate is 0 even when the server cache is at 98%.
// ---------------------------------------------------------------------------

fn metrics_url(base_url: &str) -> String {
    let mut base = base_url.trim().trim_end_matches('/').to_string();
    if let Some(stripped) = base.strip_suffix("/v1") {
        base = stripped.to_string();
    }
    format!("{base}/metrics")
}

fn parse_metric(line: &str) -> Option<(&str, f64)> {
    let (left, right) = line.rsplit_once(' ')?;
    let name = left.split('{').next()?;
    let value = right.parse::<f64>().ok()?;
    Some((name, value))
}

fn server_cache_store() -> &'static Mutex<HashMap<String, (Instant, ServerCacheStats)>> {
    static CACHE: OnceLock<Mutex<HashMap<String, (Instant, ServerCacheStats)>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Fetch \`<base>/metrics\` and extract vLLM's prefix-cache counters.
/// Results are cached for 5 s per endpoint; failures cache for 5 s too so a
/// dead endpoint does not slow the page down.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_dedupes_streaming_rows() {
        let dir = std::env::temp_dir().join(format!("agent-switch-usage-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("abc.jsonl");
        let line = r#"{"type":"assistant","timestamp":"2026-10-22T03:00:00Z","sessionId":"abc","message":{"id":"msg_1","model":"test-model","usage":{"input_tokens":100,"output_tokens":10,"cache_read_input_tokens":900,"cache_creation_input_tokens":0}}}"#;
        std::fs::write(&path, format!("{}\n{}\n", line, line)).unwrap();
        let usage = scan_claude(&path, Some(MAX_TURNS)).unwrap();
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 10);
        assert_eq!(usage.cache_read_tokens, 900);
        assert_eq!(usage.turns, 1);
        assert!((usage.cache_hit_rate - 0.9).abs() < 1e-9);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Manual diagnostic against the local profile store (reads real
    /// transcripts). Run with:
    /// cargo test -p agent-switch-core dump_real_usage -- --ignored --nocapture
    #[test]
    #[ignore]
    fn dump_real_usage() {
        let store = ProfileStore::new();
        for usage in list_usage(&store) {
            println!(
                "{} | {} | {} | turns={} prompt={} cache_read={} hit={:.3}",
                usage.session_id,
                usage.engine,
                usage.model,
                usage.turns,
                usage.total_prompt_tokens,
                usage.cache_read_tokens,
                usage.cache_hit_rate
            );
        }
    }

    /// Manual diagnostic: export the full per-request history to a temp CSV.
    /// cargo test -p agent-switch-core dump_csv -- --ignored --nocapture
    #[test]
    #[ignore]
    fn dump_csv() {
        let store = ProfileStore::new();
        let path = std::env::temp_dir().join("agent-switch-usage.csv");
        let result = export_csv(&store, &path).unwrap();
        println!(
            "rows={} sessions={} path={}",
            result.rows, result.sessions, result.path
        );
        if let Ok(text) = std::fs::read_to_string(&path) {
            for line in text.lines().take(4) {
                println!("{line}");
            }
        }
    }
}

pub fn fetch_server_cache(base_url: &str, api_key: Option<&str>) -> ServerCacheStats {
    let endpoint = metrics_url(base_url);
    if endpoint == "/metrics" {
        return ServerCacheStats {
            ok: false,
            message: "profile has no base_url".to_string(),
            endpoint,
            ..Default::default()
        };
    }
    if let Ok(cache) = server_cache_store().lock() {
        if let Some((at, stats)) = cache.get(&endpoint) {
            if at.elapsed() < Duration::from_secs(5) {
                return stats.clone();
            }
        }
    }

    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(4))
        .build();
    let mut request = agent.get(&endpoint);
    if let Some(key) = api_key.filter(|k| !k.is_empty()) {
        request = request.set("Authorization", &format!("Bearer {key}"));
    }

    let stats = match request.call() {
        Ok(response) => {
            let body = response.into_string().unwrap_or_default();
            let mut hits = 0.0;
            let mut queries = 0.0;
            let mut kv_usage = 0.0;
            let mut running = 0.0;
            for line in body.lines() {
                if line.starts_with('#') {
                    continue;
                }
                let Some((name, value)) = parse_metric(line) else {
                    continue;
                };
                match name {
                    "vllm:prefix_cache_hits_total" => hits += value,
                    "vllm:prefix_cache_queries_total" => queries += value,
                    "vllm:kv_cache_usage_perc" => kv_usage = value,
                    "vllm:num_requests_running" => running = value,
                    _ => {}
                }
            }
            let hit_rate = if queries > 0.0 { hits / queries } else { 0.0 };
            ServerCacheStats {
                ok: true,
                message: String::new(),
                endpoint,
                hits,
                queries,
                hit_rate,
                kv_usage,
                running,
            }
        }
        Err(e) => ServerCacheStats {
            ok: false,
            message: e.to_string(),
            endpoint,
            ..Default::default()
        },
    };

    if let Ok(mut cache) = server_cache_store().lock() {
        cache.insert(stats.endpoint.clone(), (Instant::now(), stats.clone()));
    }
    stats
}
