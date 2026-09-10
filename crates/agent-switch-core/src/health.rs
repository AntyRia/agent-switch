use crate::engine::Engine;
use crate::error::Result;

/// Result of a provider health check.
pub struct TestResult {
    pub ok: bool,
    pub status: u16,
    pub model_count: Option<u32>,
    pub message: String,
}

/// Health-check the given fields for the given engine: the codex engine
/// uses the OpenAI-style `GET /models`, the claude engine uses a real
/// 1-token Anthropic Messages request (an empty model is auto-detected
/// from the provider's list first).
pub fn test_engine(
    engine: Engine,
    base_url: &str,
    api_key: &str,
    model: &str,
    auth_mode: Option<&str>,
) -> Result<TestResult> {
    match engine {
        Engine::Codex => test_connection(base_url, api_key),
        Engine::Claude => test_anthropic(base_url, api_key, model, auth_mode),
    }
}

/// Human-readable failure for a non-2xx response: 401/403 say
/// "authentication failed"; anything else carries a short snippet of the
/// server's error body (that is what tells the user *why* — e.g. "The model
/// `claude-sonnet-4-5` does not exist").
fn failure_message(status: u16, body: &str) -> String {
    if status == 401 || status == 403 {
        return "authentication failed".to_string();
    }
    let flat: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
    let snippet: String = flat.chars().take(200).collect();
    if snippet.trim().is_empty() {
        format!("server returned status {status}")
    } else {
        format!("server returned status {status}: {snippet}")
    }
}

/// POST `<base_url>/v1/messages` with a 1-token request. `auth_mode` of
/// "api_key" sends `x-api-key`; anything else sends `Authorization: Bearer`
/// (the relay convention and the default).
///
/// An empty `model` is the "not picked yet" state: the first model from the
/// provider's list is used instead, because relays and vLLM reject an empty
/// model name with a 400.
pub fn test_anthropic(
    base_url: &str,
    api_key: &str,
    model: &str,
    auth_mode: Option<&str>,
) -> Result<TestResult> {
    let trimmed = model.trim();
    let (model, auto) = if trimmed.is_empty() {
        match crate::models::fetch_models(Engine::Claude, base_url, api_key, auth_mode) {
            Ok(list) if !list.models.is_empty() => (list.models[0].clone(), true),
            _ => {
                return Ok(TestResult {
                    ok: false,
                    status: 0,
                    model_count: None,
                    message: "no model set, and the model list could not be auto-fetched — set a model or check the base URL".to_string(),
                })
            }
        }
    } else {
        (trimmed.to_string(), false)
    };

    let url = format!("{}/v1/messages", base_url.trim_end_matches('/'));
    let body = serde_json::json!({
        "model": model,
        "max_tokens": 1,
        "messages": [{ "role": "user", "content": "ping" }],
    })
    .to_string();
    let mut request = ureq::post(&url)
        .set("content-type", "application/json")
        .set("anthropic-version", "2023-06-01")
        .timeout(std::time::Duration::from_secs(10));
    match auth_mode {
        Some(m) if m.trim().eq_ignore_ascii_case("api_key") => {
            request = request.set("x-api-key", api_key)
        }
        _ => request = request.set("Authorization", &format!("Bearer {api_key}")),
    }
    match request.send_string(&body) {
        Ok(response) => {
            let status = response.status() as u16;
            let ok = (200..300).contains(&status);
            Ok(TestResult {
                ok,
                status,
                model_count: None,
                message: if ok {
                    let note = if auto { " (model auto-detected)" } else { "" };
                    format!("connected to {url}{note}")
                } else {
                    failure_message(status, &response.into_string().unwrap_or_default())
                },
            })
        }
        Err(ureq::Error::Status(status, response)) => {
            let status = status as u16;
            let body = response.into_string().unwrap_or_default();
            Ok(TestResult {
                ok: false,
                status,
                model_count: None,
                message: failure_message(status, &body),
            })
        }
        Err(e) => Ok(TestResult {
            ok: false,
            status: 0,
            model_count: None,
            message: format!("unreachable: {e}"),
        }),
    }
}

/// GET `<base_url>/models` with the given bearer key (10s timeout).
pub fn test_connection(base_url: &str, api_key: &str) -> Result<TestResult> {
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    match ureq::get(&url)
        .set("Authorization", &format!("Bearer {api_key}"))
        .timeout(std::time::Duration::from_secs(10))
        .call()
    {
        Ok(response) => {
            let status = response.status() as u16;
            let body = response.into_string().unwrap_or_default();
            let model_count = serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .and_then(|v| v.get("data")?.as_array()?.len().try_into().ok());
            let ok = (200..300).contains(&status);
            Ok(TestResult {
                ok,
                status,
                model_count,
                message: if ok {
                    format!("connected to {url}")
                } else {
                    failure_message(status, &body)
                },
            })
        }
        Err(ureq::Error::Status(status, response)) => {
            let status = status as u16;
            let body = response.into_string().unwrap_or_default();
            Ok(TestResult {
                ok: false,
                status,
                model_count: None,
                message: failure_message(status, &body),
            })
        }
        Err(e) => Ok(TestResult {
            ok: false,
            status: 0,
            model_count: None,
            message: format!("unreachable: {e}"),
        }),
    }
}
