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

/// Outcome of a model-list fetch.
pub struct ModelList {
    pub ok: bool,
    pub message: String,
    /// Model ids in server order, de-duplicated.
    pub models: Vec<String>,
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
            .timeout(std::time::Duration::from_secs(10));
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
                    let models = parse_model_ids(&body);
                    return Ok(ModelList {
                        ok: true,
                        message: format!("connected to {url}"),
                        models,
                    });
                }
                if status == 401 || status == 403 {
                    return Ok(ModelList {
                        ok: false,
                        message: "authentication failed".to_string(),
                        models: vec![],
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
    })
}

/// Extract model ids from a model-list response body. Tolerates the common
/// shapes: OpenAI `{"data": [{"id": ...}]}`, `{"models": [...]}`, or a bare
/// JSON array; items may be plain strings or objects with `id`/`name`.
pub fn parse_model_ids(body: &str) -> Vec<String> {
    let value: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
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
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    for item in items {
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
            if !out.iter().any(|m| m == &id) {
                out.push(id);
            }
        }
    }
    out
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
