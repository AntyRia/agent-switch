//! Fetch a provider's model list.
//!
//! Both protocol families expose a list endpoint, but the URL conventions
//! differ: the codex engine's base URL is the OpenAI root (usually ending in
//! `/v1`) → `GET <base>/models`; the claude engine's base URL is the
//! Anthropic server root (no `/v1`) → `GET <base>/v1/models` (the official
//! Anthropic endpoint, also served by vLLM's Anthropic-compatible mode).
//! Each engine tries the other convention as a fallback so a slightly off
//! base URL still works.

use crate::engine::Engine;
use crate::error::Result;
use crate::profile::Profile;
use crate::profile_store::ProfileStore;

/// Outcome of a model-list fetch.
pub struct ModelList {
    pub ok: bool,
    pub message: String,
    /// Model ids in server order, de-duplicated.
    pub models: Vec<String>,
    /// Context window in tokens, when the server advertises one: vLLM
    /// extends its /v1/models items with `max_model_len`. OpenAI and relay
    /// servers do not expose this — None in that case.
    pub max_model_len: Option<u32>,
}

/// Candidate model-list endpoints, in try order.
fn candidate_urls(engine: Engine, base_url: &str) -> Vec<String> {
    let base = base_url.trim().trim_end_matches('/');
    match engine {
        Engine::Codex => vec![format!("{base}/models"), format!("{base}/v1/models")],
        Engine::Claude => vec![format!("{base}/v1/models"), format!("{base}/models")],
    }
}

/// Fetch the model list. The first candidate returning 2xx wins; 401/403
/// fails fast (the key is wrong, the second URL would fail too).
pub fn fetch_models(
    engine: Engine,
    base_url: &str,
    api_key: &str,
    auth_mode: Option<&str>,
) -> Result<ModelList> {
    let base = base_url.trim();
    if base.is_empty() {
        return Ok(ModelList {
            ok: false,
            message: "no base URL given".to_string(),
            models: vec![],
            max_model_len: None,
        });
    }

    // Claude honours auth_mode (x-api-key vs Bearer); codex is always Bearer.
    let use_api_key_header = engine == Engine::Claude
        && matches!(
            auth_mode,
            Some(m) if m.trim().eq_ignore_ascii_case("api_key")
        );

    let mut last_status: Option<u16> = None;
    for url in candidate_urls(engine, base) {
        let mut request = ureq::get(&url)
            .set("accept", "application/json")
            .timeout(std::time::Duration::from_secs(5));
        if use_api_key_header {
            request = request.set("x-api-key", api_key);
        } else {
            request = request.set("Authorization", &format!("Bearer {api_key}"));
        }
        match request.call() {
            Ok(response) => {
                let status = response.status();
                if (200..300).contains(&status) {
                    let body = response.into_string().unwrap_or_default();
                    // One JSON parse serves both the ids and the length.
                    let (models, max_model_len) = parse_model_list(&body);
                    return Ok(ModelList {
                        ok: true,
                        message: format!("connected to {url}"),
                        max_model_len,
                        models,
                    });
                }
                if status == 401 || status == 403 {
                    return Ok(ModelList {
                        ok: false,
                        message: "authentication failed".to_string(),
                        models: vec![],
                        max_model_len: None,
                    });
                }
                last_status = Some(status as u16);
            }
            Err(ureq::Error::Status(status, _)) => {
                if status == 401 || status == 403 {
                    return Ok(ModelList {
                        ok: false,
                        message: "authentication failed".to_string(),
                        models: vec![],
                        max_model_len: None,
                    });
                }
                last_status = Some(status as u16);
            }
            // Connection-level failure: both candidates share the host, so
            // there is no point trying the other URL.
            Err(e) => {
                return Ok(ModelList {
                    ok: false,
                    message: format!("unreachable: {e}"),
                    models: vec![],
                    max_model_len: None,
                });
            }
        }
    }

    let message = match last_status {
        Some(s) => format!("server returned status {s}"),
        None => "no model list endpoint found".to_string(),
    };
    Ok(ModelList {
        ok: false,
        message,
        models: vec![],
        max_model_len: None,
    })
}

/// Strict-sync merge: replace the stored model list with what the provider
/// currently serves, while keeping the profile's default model (it is what
/// every launch starts on, and relays regularly omit it from /models —
/// e.g. hyperroute.cc serves `gpt-5.6-sol` while /v1/models lists only 4
/// of its models). `codex-auto-*` ids are excluded: they are OpenAI's
/// built-in "auto modes", they change the Codex TUI's /model layout and
/// have no meaning for a third-party profile.
///
/// Returns `(merged list, added, removed)` so the caller can report the
/// delta.
pub fn merge_model_list(
    current: &[String],
    upstream: &[String],
    default_model: &str,
) -> (Vec<String>, usize, usize) {
    let default = default_model.trim();
    let mut merged: Vec<String> = Vec::new();
    // Membership set keeps the de-dup and the delta counts O(n), not O(n²).
    let mut in_merged: std::collections::HashSet<&str> = std::collections::HashSet::new();
    if !default.is_empty() {
        merged.push(default.to_string());
        in_merged.insert(default);
    }
    for m in upstream {
        if m.starts_with("codex-auto-") || m.trim().is_empty() {
            continue;
        }
        if in_merged.insert(m.as_str()) {
            merged.push(m.clone());
        }
    }
    let in_current: std::collections::HashSet<&str> =
        current.iter().map(|s| s.as_str()).collect();
    let added = merged.iter().filter(|m| !in_current.contains(m.as_str())).count();
    let removed = current
        .iter()
        .filter(|c| c.trim() != default && !in_merged.contains(c.as_str()))
        .count();
    (merged, added, removed)
}

/// Refresh `profile.model.models` from the provider right before a NEW
/// conversation is started (called by the GUI launch flow and the CLI
/// `run`; resume flows skip it).
///
/// The sync is STRICT: the stored list is replaced by the upstream list
/// (plus the kept default model, minus `codex-auto-*`), so users are never
/// offered a model the provider no longer serves. A failed or empty fetch
/// keeps the stored list untouched — a sync failure must never block the
/// launch. The profile is persisted in place only when the list changed.
///
/// Returns the (possibly updated) profile plus an optional note for the
/// UI: "model sync failed: …" when the fetch failed, "model list synced:
/// N added, M removed" when the list changed.
///
/// Official profiles are skipped: with a subscription login there is no
/// key to call the list endpoint with, and a key against the vendor API
/// would return the vendor's FULL catalog — a strict sync would replace
/// the user's curated model list with hundreds of irrelevant ids.
pub fn sync_profile_models(
    store: &ProfileStore,
    profile: &Profile,
) -> Result<(Profile, Option<String>)> {
    if profile.is_official() {
        return Ok((profile.clone(), None));
    }
    let key = crate::launcher::resolve_api_key(profile)?;
    let list = fetch_models(
        profile.engine(),
        &profile.provider.base_url,
        &key,
        profile.provider.auth_mode.as_deref(),
    )?;
    if !list.ok || list.models.is_empty() {
        let note = if !list.ok {
            Some(format!("model sync failed: {}", list.message))
        } else {
            None
        };
        return Ok((profile.clone(), note));
    }
    let (merged, added, removed) =
        merge_model_list(&profile.model.models, &list.models, &profile.model.default);
    if added == 0 && removed == 0 {
        return Ok((profile.clone(), None));
    }
    let mut updated = profile.clone();
    updated.model.models = merged;
    store.save(&updated, Some(&profile.id))?;
    crate::logging::info(&format!(
        "model list synced for '{}': {added} added, {removed} removed",
        profile.id
    ));
    Ok((
        updated,
        Some(format!("model list synced: {added} added, {removed} removed")),
    ))
}

/// Parse a model-list response body once into (de-duplicated ids in server
/// order, context window). Tolerates the common shapes: OpenAI
/// `{"data": [{"id": ...}]}`, `{"models": [...]}`, or a bare JSON array;
/// items may be plain strings or objects with `id`/`name`. vLLM extends its
/// items with `max_model_len` (the context window in tokens) — the first
/// value found wins; OpenAI/relay servers do not carry the field, so that
/// half is None for them.
fn parse_model_list(body: &str) -> (Vec<String>, Option<u32>) {
    let value: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(_) => return (Vec::new(), None),
    };
    let items = if value.is_array() {
        value.as_array()
    } else {
        value
            .get("data")
            .or_else(|| value.get("models"))
            .and_then(|v| v.as_array())
    };
    let Some(items) = items else {
        return (Vec::new(), None);
    };
    let mut out: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut max_model_len: Option<u32> = None;
    for item in items {
        if max_model_len.is_none() {
            if let Some(v) = item.get("max_model_len").and_then(|v| v.as_u64()) {
                max_model_len = v.try_into().ok();
            }
        }
        let id = match item {
            serde_json::Value::String(s) if !s.trim().is_empty() => {
                Some(s.trim().to_string())
            }
            obj => obj
                .get("id")
                .or_else(|| obj.get("name"))
                .and_then(|v| v.as_str())
                .filter(|s| !s.trim().is_empty())
                .map(|s| s.trim().to_string()),
        };
        if let Some(id) = id {
            // insert-before-push de-dupes in O(1); the cloned value in the
            // set outlives the loop, so a borrow of `id` is not needed.
            if seen.insert(id.clone()) {
                out.push(id);
            }
        }
    }
    (out, max_model_len)
}

/// Extract model ids from a model-list response body (de-duplicated, server
/// order). See `parse_model_list` for the accepted shapes.
pub fn parse_model_ids(body: &str) -> Vec<String> {
    parse_model_list(body).0
}

/// vLLM extends its /v1/models items with `max_model_len` (the context
/// window in tokens). First value found wins; OpenAI/relay servers do not
/// carry the field, so this returns None for them.
pub fn parse_max_model_len(body: &str) -> Option<u32> {
    parse_model_list(body).1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_openai_shape_and_dedupes() {
        let body = r#"{"object":"list","data":[{"id":"gpt-5.6"},{"id":"gpt-5.6-sol"},{"id":"gpt-5.6"}]}"#;
        assert_eq!(parse_model_ids(body), vec!["gpt-5.6", "gpt-5.6-sol"]);
    }

    #[test]
    fn parses_models_key_with_name_field() {
        let body = r#"{"models":[{"name":"claude-sonnet-4-5"},{"id":"claude-haiku-4-5"}]}"#;
        assert_eq!(
            parse_model_ids(body),
            vec!["claude-sonnet-4-5", "claude-haiku-4-5"]
        );
    }

    #[test]
    fn parses_bare_array_of_strings() {
        assert_eq!(parse_model_ids(r#"["a","b"]"#), vec!["a", "b"]);
    }

    #[test]
    fn rejects_garbage_and_missing_ids() {
        assert!(parse_model_ids("not json").is_empty());
        assert!(parse_model_ids(r#"{"data":[{"foo":1}]}"#).is_empty());
        assert!(parse_model_ids(r#"{}"#).is_empty());
    }

    #[test]
    fn parses_vllm_max_model_len() {
        let body = r#"{"object":"list","data":[{"id":"Qwen3.8-27B","max_model_len":262144}]}"#;
        assert_eq!(parse_max_model_len(body), Some(262_144));
        // OpenAI/relay shape: no field at all.
        assert_eq!(parse_max_model_len(r#"{"data":[{"id":"gpt-5.6"}]}"#), None);
        assert_eq!(parse_max_model_len("not json"), None);
    }

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn merge_replaces_and_keeps_default() {
        let (merged, added, removed) =
            merge_model_list(&s(&["a", "b", "stale"]), &s(&["b", "c"]), "stale");
        // The stale non-default model is dropped, the new one added, and
        // the default model is kept even though upstream omits it.
        assert_eq!(merged, s(&["stale", "b", "c"]));
        assert_eq!(added, 1);
        assert_eq!(removed, 1);
    }

    #[test]
    fn merge_excludes_codex_auto_ids() {
        // First sync (empty stored list): the default model lands in the
        // list, so it counts as one addition; codex-auto-* never does.
        let (merged, added, removed) =
            merge_model_list(&s(&[]), &s(&["codex-auto-latest", "gpt-5.6"]), "gpt-5.6");
        assert_eq!(merged, s(&["gpt-5.6"]));
        assert_eq!(added, 1);
        assert_eq!(removed, 0);
        // Already in sync: a re-fetch adds nothing.
        let (merged, added, removed) =
            merge_model_list(&s(&["gpt-5.6"]), &s(&["codex-auto-latest", "gpt-5.6"]), "gpt-5.6");
        assert_eq!(merged, s(&["gpt-5.6"]));
        assert_eq!(added, 0);
        assert_eq!(removed, 0);
    }

    #[test]
    fn merge_no_change_is_a_noop() {
        let (merged, added, removed) =
            merge_model_list(&s(&["d", "x"]), &s(&["x", "y"]), "d");
        // "y" is new, so this is not a noop after all — verify the
        // pure-noop case below instead.
        assert_eq!(merged, s(&["d", "x", "y"]));
        assert_eq!(added, 1);
        assert_eq!(removed, 0);
        let (merged, added, removed) =
            merge_model_list(&s(&["d", "x"]), &s(&["x", "d"]), "d");
        assert_eq!(merged, s(&["d", "x"]));
        assert_eq!(added, 0);
        assert_eq!(removed, 0);
    }

    #[test]
    fn merge_first_sync_counts_everything_added() {
        let (merged, added, removed) = merge_model_list(&s(&[]), &s(&["a", "b"]), "a");
        assert_eq!(merged, s(&["a", "b"]));
        assert_eq!(added, 2);
        assert_eq!(removed, 0);
    }

    #[test]
    fn candidate_url_order_depends_on_engine() {
        assert_eq!(
            candidate_urls(Engine::Codex, "https://r.example.com/v1/"),
            vec![
                "https://r.example.com/v1/models",
                "https://r.example.com/v1/v1/models"
            ]
        );
        assert_eq!(
            candidate_urls(Engine::Claude, "https://r.example.com"),
            vec![
                "https://r.example.com/v1/models",
                "https://r.example.com/models"
            ]
        );
    }
}
