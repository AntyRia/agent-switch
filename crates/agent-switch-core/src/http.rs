//! Shared, proxy-aware HTTP plumbing for the app's own requests: the
//! model-list fetch, the connection test, and the release check/download.
//!
//! The launched CLIs get their proxy from the launcher's env-var
//! injection; ureq (this crate's HTTP client) does not read any proxy
//! configuration by itself, so every request site must wire the same
//! proxy explicitly — otherwise a probe can fail direct while the CLI
//! launched seconds later reaches the very same server fine through the
//! proxy.

use std::time::Duration;

use crate::settings::Settings;

/// Timeout for app-side probes (model-list fetch, connection test). 20 s:
/// overseas relays behind Cloudflare routinely take 2–6 s on a direct
/// connection, and a 5 s budget turned flaky-but-reachable relays into
/// `unreachable (os error 10060)`.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(20);

/// Pause between a failed probe attempt and its single retry.
const RETRY_PAUSE: Duration = Duration::from_millis(300);

/// The proxy for `url`, if any, in priority order:
///
/// 1. the app's own Settings proxy — the same value the launcher injects
///    into every launched CLI,
/// 2. explicit environment (`HTTPS_PROXY` / `ALL_PROXY` / `HTTP_PROXY`,
///    either case — what a shell-launched CLI inherits),
/// 3. the Windows Internet Options "system proxy" — the value a proxy
///    tool such as Clash writes in "System Proxy" mode, and the only
///    thing a Start-menu-launched GUI can see.
///
/// Loopback and `NO_PROXY` hosts are always direct, so a local vLLM /
/// SGLang profile is never tunneled through the proxy.
pub(crate) fn proxy_for(url: &str) -> Option<String> {
    if let Some(host) = url_host(url) {
        if is_direct_host(&host) {
            return None;
        }
    }
    if let Some(p) = Settings::load().proxy_url() {
        return Some(p);
    }
    env_proxy()
}

/// A proxy-aware client for `url`, plus the proxy URL actually in play
/// (None = direct connection) so error messages can say which route was
/// attempted.
pub(crate) fn client_for(url: &str) -> (ureq::Agent, Option<String>) {
    let proxy = proxy_for(url);
    (proxy_agent(proxy.clone()), proxy)
}

/// The agent half of [`client_for`] (for callers that do not need the
/// proxy URL for messaging).
pub(crate) fn agent_for(url: &str) -> ureq::Agent {
    client_for(url).0
}

/// Suffix for `unreachable: …` probe messages: which route was attempted,
/// so the user can tell "no proxy in play" from "proxy did not reach".
pub(crate) fn proxy_note(proxy: &Option<String>) -> String {
    match proxy {
        Some(p) => format!(" (via proxy {p})"),
        None => " (direct connection, no proxy)".to_string(),
    }
}

/// Run `attempt`, retrying ONCE (after a short pause) when it fails at
/// the transport level — a flaky direct connection must not fail the
/// whole probe. A server answer (status error) is returned as-is:
/// retrying a 401 changes nothing.
pub(crate) fn with_retry<T>(
    mut attempt: impl FnMut() -> Result<T, ureq::Error>,
) -> Result<T, ureq::Error> {
    match attempt() {
        Ok(v) => Ok(v),
        Err(e) => {
            if matches!(e, ureq::Error::Transport(_)) {
                std::thread::sleep(RETRY_PAUSE);
                return attempt();
            }
            Err(e)
        }
    }
}

/// Proxy from the environment only (see [`proxy_for`] for the full order).
fn env_proxy() -> Option<String> {
    const VARS: &[&str] = &[
        "HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy", "HTTP_PROXY", "http_proxy",
    ];
    for var in VARS {
        if let Ok(v) = std::env::var(var) {
            let v = v.trim();
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    #[cfg(windows)]
    {
        windows_system_proxy()
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// The Windows "system proxy" from Internet Settings, as an http proxy URL.
#[cfg(windows)]
fn windows_system_proxy() -> Option<String> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;
    let base = RegKey::predef(HKEY_CURRENT_USER);
    let key = base
        .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Internet Settings")
        .ok()?;
    let enabled: u32 = key.get_value("ProxyEnable").ok()?;
    if enabled != 1 {
        return None;
    }
    let server: String = key.get_value("ProxyServer").ok()?;
    normalize_proxy_server(&server)
}

/// Turn a raw Windows `ProxyServer` value into a usable http proxy URL. The
/// common form is a bare `host:port` (e.g. Clash's mixed port), which gets a
/// scheme; a comma-separated per-host list yields its first entry; a value
/// that is already a URL is passed through.
#[cfg_attr(not(windows), allow(dead_code))]
fn normalize_proxy_server(server: &str) -> Option<String> {
    let first = server.split(',').next()?.trim();
    if first.is_empty() {
        return None;
    }
    if first.contains("://") {
        return Some(first.to_string());
    }
    Some(format!("http://{first}"))
}

/// A proxy-enabled client; falls back to a plain one if the proxy URL is
/// bad. A bare `host:port` (no scheme) is treated as an HTTP proxy.
fn proxy_agent(proxy: Option<String>) -> ureq::Agent {
    let Some(proxy) = proxy else {
        return ureq::Agent::new();
    };
    let url = if proxy.contains("://") {
        proxy
    } else {
        format!("http://{proxy}")
    };
    match ureq::Proxy::new(url) {
        Ok(proxy) => ureq::AgentBuilder::new().proxy(proxy).build(),
        Err(_) => ureq::Agent::new(),
    }
}

/// The (lowercased) host of a URL, without scheme, userinfo, port or path.
fn url_host(url: &str) -> Option<String> {
    let after_scheme = url.split_once("://")?.1;
    let authority = after_scheme.split(['/', '?', '#']).next()?;
    let hostport = authority.rsplit_once('@').map(|(_, h)| h).unwrap_or(authority);
    let colons = hostport.matches(':').count();
    let host = if hostport.starts_with('[') {
        hostport.split(']').next().unwrap_or("")
    } else if colons == 1 {
        // Ordinary `host:port` — the part before the colon.
        hostport.split_once(':').unwrap().0
    } else if colons >= 3 {
        // Bracketless IPv6 WITH a port (https://::1:9000): everything up
        // to the last colon is the host.
        hostport.rsplit_once(':').unwrap().0
    } else {
        // No port, or bracketless IPv6 without a port (https://::1): the
        // whole thing is the host.
        hostport
    };
    let host = host.trim().trim_start_matches('[').to_ascii_lowercase();
    (!host.is_empty()).then_some(host)
}

/// True when `host` must connect directly, never through a proxy: loopback
/// addresses and anything matching the `NO_PROXY` env var (`*` disables proxy).
fn is_direct_host(host: &str) -> bool {
    is_loopback(host) || matches_no_proxy(host)
}

fn is_loopback(host: &str) -> bool {
    matches!(host, "localhost" | "::1") || host == "127.0.0.1" || host.starts_with("127.")
}

fn matches_no_proxy(host: &str) -> bool {
    for var in ["NO_PROXY", "no_proxy"] {
        let Ok(v) = std::env::var(var) else {
            continue;
        };
        if v.trim() == "*" {
            return true;
        }
        for entry in v.split(',') {
            let entry = entry.trim().trim_start_matches('.');
            if entry.is_empty() {
                continue;
            }
            if host == entry || host.ends_with(&format!(".{entry}")) {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_host_parses_variants() {
        assert_eq!(
            url_host("https://r.example.com/v1/models"),
            Some("r.example.com".into())
        );
        assert_eq!(url_host("https://r.example.com:8443/x"), Some("r.example.com".into()));
        assert_eq!(url_host("http://user:pass@R.Example.com/"), Some("r.example.com".into()));
        assert_eq!(url_host("http://[::1]:9000/"), Some("::1".into()));
        assert_eq!(url_host("https://::1:9000/x"), Some("::1".into()));
        assert_eq!(url_host("https://::1/"), Some("::1".into()));
        assert_eq!(url_host("nonsense"), None);
    }

    use crate::settings::ENV_LOCK;

    #[test]
    fn loopback_hosts_are_always_direct() {
        // No Settings/env lookup happens on the direct-host gate, so this
        // assertion cannot race with the other config-root tests.
        for h in ["localhost", "127.0.0.1", "127.3.4.5", "::1"] {
            assert!(is_direct_host(h), "{h}");
            assert_eq!(proxy_for(&format!("https://{h}/v1/models")), None);
        }
        assert!(!is_direct_host("192.168.1.10"));
    }

    #[test]
    fn no_proxy_wins_over_configured_proxies() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "as-http-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("AGENT_SWITCH_HOME", &dir);
        std::env::set_var("NO_PROXY", "example.com");
        std::fs::write(
            dir.join("settings.toml"),
            "proxy_host = \"127.0.0.1\"\nproxy_port = 7897\n",
        )
        .unwrap();
        assert_eq!(proxy_for("https://example.com/v1/models"), None);
        assert_eq!(
            proxy_for("https://other.com/v1/models"),
            Some("http://127.0.0.1:7897".into())
        );
        std::env::remove_var("NO_PROXY");
        std::env::remove_var("AGENT_SWITCH_HOME");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn settings_proxy_is_honoured() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "as-http2-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("AGENT_SWITCH_HOME", &dir);
        std::fs::write(
            dir.join("settings.toml"),
            "proxy_host = \"10.0.0.1\"\nproxy_port = 1080\n",
        )
        .unwrap();
        assert_eq!(
            proxy_for("https://hyperroute.cc/v1/models"),
            Some("http://10.0.0.1:1080".into())
        );
        // Once the file is gone the settings proxy is out of play (the
        // env/system-proxy fallback may still answer on machines that have
        // one — e.g. Clash's "System Proxy" — so compare the value, not
        // the route).
        std::fs::remove_file(dir.join("settings.toml")).unwrap();
        assert_ne!(
            proxy_for("https://hyperroute.cc/v1/models"),
            Some("http://10.0.0.1:1080".into())
        );
        // Loopback stays direct no matter what is configured.
        assert_eq!(proxy_for("https://127.0.0.1/v1/models"), None);
        std::env::remove_var("AGENT_SWITCH_HOME");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn normalize_proxy_server_forms() {
        // Bare host:port (Clash's mixed port) gets a scheme.
        assert_eq!(
            normalize_proxy_server("127.0.0.1:7897").as_deref(),
            Some("http://127.0.0.1:7897")
        );
        // Comma-separated per-host list: the first entry wins.
        assert_eq!(
            normalize_proxy_server("127.0.0.1:7897,corp.proxy:8080").as_deref(),
            Some("http://127.0.0.1:7897")
        );
        // A value that is already a URL passes through untouched.
        assert_eq!(
            normalize_proxy_server("http://10.0.0.5:1080").as_deref(),
            Some("http://10.0.0.5:1080")
        );
        assert_eq!(normalize_proxy_server(""), None);
        assert_eq!(normalize_proxy_server("   "), None);
    }

    #[test]
    fn url_host_variants_moved_from_update() {
        // Kept from update.rs where the helper lived before the shared
        // module.
        assert_eq!(
            url_host("https://api.github.com/repos/x/releases").as_deref(),
            Some("api.github.com")
        );
        assert_eq!(
            url_host("http://127.0.0.1:8080/a/b?c=1").as_deref(),
            Some("127.0.0.1")
        );
        assert_eq!(url_host("https://user:pass@example.com/").as_deref(), Some("example.com"));
    }

    #[test]
    fn loopback_detection_edge_cases() {
        assert!(is_loopback("127.9.9.9"));
        // "1270.0.0.1" is NOT loopback — only the 127.x/8 network is.
        assert!(!is_loopback("1270.0.0.1"));
        assert!(!is_loopback("10.0.0.1"));
    }

    #[test]
    fn proxy_note_describes_the_route() {
        assert_eq!(proxy_note(&None), " (direct connection, no proxy)");
        assert_eq!(
            proxy_note(&Some("http://127.0.0.1:7897".into())),
            " (via proxy http://127.0.0.1:7897)"
        );
    }

    /// A real transport-level error: TCP port 1 on loopback refuses
    /// instantly, so no test hangs on a slow route.
    fn transport_error() -> ureq::Error {
        ureq::get("http://127.0.0.1:1/transport-error")
            .timeout(Duration::from_secs(1))
            .call()
            .unwrap_err()
    }

    #[test]
    fn retry_retries_once_then_succeeds() {
        let mut calls = 0u32;
        let out = with_retry(|| {
            calls += 1;
            if calls == 1 {
                Err(transport_error())
            } else {
                Ok(42_u32)
            }
        })
        .unwrap();
        assert_eq!(out, 42);
        assert_eq!(calls, 2);
    }

    #[test]
    fn retry_gives_up_after_one_attempt() {
        let mut calls = 0u32;
        let res: std::result::Result<u32, ureq::Error> = with_retry(|| {
            calls += 1;
            Err(transport_error())
        });
        assert!(matches!(res, Err(ureq::Error::Transport(_))));
        assert_eq!(calls, 2);
    }

    #[test]
    fn retry_reruns_a_real_failed_request() {
        // Port 1 (TCP discard) refuses instantly — a real transport error.
        let mut calls = 0u32;
        let res = with_retry(|| {
            calls += 1;
            ureq::get("http://127.0.0.1:1/x")
                .timeout(Duration::from_millis(500))
                .call()
        });
        assert!(matches!(res, Err(ureq::Error::Transport(_))));
        assert_eq!(calls, 2);
    }
}
