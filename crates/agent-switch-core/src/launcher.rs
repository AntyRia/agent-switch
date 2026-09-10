use std::path::{Path, PathBuf};

use crate::claude::find_claude;
use crate::codex::{find_codex, needs_cmd_wrap};
use crate::engine::Engine;
use crate::error::{Error, Result};
use crate::profile::Profile;
use crate::profile_store::ProfileStore;
use crate::runtime::{create_runtime, Runtime};

/// Resolve the API key for a profile. Precedence (spec §9):
/// 1. the environment variable named by `api_key_env` (when set, non-empty)
/// 2. the `api_key` stored in the profile
pub fn resolve_api_key(p: &Profile) -> Result<String> {
    if let Some(env_name) = p
        .provider
        .api_key_env
        .as_deref()
        .filter(|n| !n.is_empty())
    {
        if let Ok(value) = std::env::var(env_name) {
            if !value.is_empty() {
                return Ok(value);
            }
        }
    }
    if let Some(key) = p
        .provider
        .api_key
        .as_deref()
        .filter(|k| !k.is_empty())
    {
        return Ok(key.to_string());
    }
    // No key configured (e.g. a local vLLM server): launch with an empty
    // key env var rather than failing.
    Ok(String::new())
}

/// Locate the CLI binary for an engine.
pub fn find_engine_binary(engine: Engine) -> Result<PathBuf> {
    match engine {
        Engine::Codex => find_codex(),
        Engine::Claude => find_claude(),
    }
}

/// Ambient env vars a launched CLI must never inherit.
///
/// `CLAUDE_CODE_*` / `CLAUDECODE` / `CLAUDE_PID` / `CLAUDE_EFFORT` are
/// Claude Code's nested-session markers: when present, the launched Claude
/// Code treats itself as a subprocess of another Claude Code session — it
/// turns transcript saving off and drops itself from --resume, --continue
/// and history. (That is exactly what happens when the GUI is itself
/// started from inside a Claude Code session.) Ambient `ANTHROPIC_*` /
/// `OPENAI_*` provider vars are dropped as well: the profile is the single
/// source of routing truth, and its own values are re-applied explicitly.
pub fn is_ambient_env_var(key: &str) -> bool {
    let k = key.to_ascii_uppercase();
    k.starts_with("CLAUDE_CODE_")
        || k.starts_with("CLAUDE_SESSION")
        || k == "CLAUDECODE"
        || k == "CLAUDE_PID"
        || k == "CLAUDE_EFFORT"
        || k.starts_with("ANTHROPIC_")
        || k == "OPENAI_API_KEY"
        || k == "OPENAI_BASE_URL"
}

/// The caller's environment minus the ambient session/provider vars above.
/// The profile's own vars are layered on top by the caller.
pub fn sanitized_child_env() -> Vec<(String, String)> {
    std::env::vars()
        .into_iter()
        .filter(|(k, _)| !is_ambient_env_var(k))
        .collect()
}

/// Run the profile's CLI in the caller's terminal with an isolated config
/// home and a per-process API key. Waits for the CLI and returns its exit
/// code.
pub fn run_profile(
    store: &ProfileStore,
    profile: &Profile,
    workspace: &Path,
    extra_args: &[String],
) -> Result<i32> {
    let runtime = create_runtime(store, profile)?;
    let key = resolve_api_key(profile)?;
    launch_in_runtime(&runtime, profile, key, workspace, extra_args)
}

/// Spawn the profile's CLI against an already-created isolated runtime,
/// with the API key injected as a per-process environment variable. Waits
/// for the CLI and returns its exit code.
pub fn launch_in_runtime(
    runtime: &Runtime,
    profile: &Profile,
    api_key: String,
    workspace: &Path,
    extra_args: &[String],
) -> Result<i32> {
    let engine = runtime.engine;
    if matches!(engine, Engine::Claude) {
        // Mark the workspace trusted in the isolated home (idempotent
        // merge; see seed_claude_home). Best effort — never blocks launch.
        let _ = crate::runtime::seed_claude_home(&runtime.home, Some(workspace));
    }
    let bin = find_engine_binary(engine)?;
    let (program, initial) = if needs_cmd_wrap(&bin) {
        (
            "cmd.exe".to_string(),
            vec!["/C".to_string(), bin.to_string_lossy().into_owned()],
        )
    } else {
        (bin.to_string_lossy().into_owned(), Vec::new())
    };
    let mut cmd = std::process::Command::new(&program);
    for arg in initial {
        cmd.arg(arg);
    }
    // The caller (CLI or GUI) may itself be running inside a Claude Code
    // session or a shell exporting provider vars. The launched CLI gets a
    // clean environment plus ONLY the profile's own vars — that is what
    // keeps transcript saving on and routing independent of the parent.
    cmd.env_clear();
    cmd.envs(sanitized_child_env());
    cmd.current_dir(workspace)
        .env(runtime.engine.home_env(), &runtime.home);
    match engine {
        Engine::Codex => {
            cmd.env("OPENAI_API_KEY", api_key);
        }
        // Anthropic protocol: base URL + model travel as env vars (the
        // isolated .claude dir holds no config at all). The small/fast
        // model is pinned to the same model so relays without a haiku do
        // not break background requests.
        Engine::Claude => {
            cmd.env("ANTHROPIC_BASE_URL", &profile.provider.base_url)
                .env("ANTHROPIC_MODEL", &profile.model.default)
                .env("ANTHROPIC_SMALL_FAST_MODEL", &profile.model.default)
                .env(engine.key_env(profile.provider.auth_mode.as_deref()), api_key)
                // Belt-and-braces: force transcript persistence even if a
                // nested-session marker slips through (requires Claude
                // Code >= 2.1.172).
                .env("CLAUDE_CODE_FORCE_SESSION_PERSISTENCE", "1");
            // Optional per-profile reasoning effort (see ModelConfig.effort).
            if let Some(effort) = profile
                .model
                .effort
                .as_deref()
                .filter(|e| !e.trim().is_empty())
            {
                cmd.env("CLAUDE_CODE_EFFORT_LEVEL", effort);
            }
        }
    }
    for arg in extra_args {
        cmd.arg(arg);
    }
    let status = cmd.status()?;
    Ok(status.code().unwrap_or(1))
}

/// Result of preparing a terminal launch (used by the GUI).
pub struct TerminalLaunch {
    pub runtime_id: String,
    pub script_path: PathBuf,
    pub runtime: Runtime,
    /// Secret (name, value) env pairs injected into the terminal process at
    /// launch time. Kept in-process only, never written to the start script
    /// (spec §9); hand them to `open_in_system_terminal`.
    pub env: Vec<(String, String)>,
}

/// Non-secret runtime env a start script may write to disk: the isolated
/// config home, and for Claude the base URL + model (the key is NOT one of
/// these — it is injected per process at launch, spec §9).
fn non_secret_env(runtime: &Runtime, profile: &Profile) -> Vec<(&'static str, String)> {
    match runtime.engine {
        Engine::Codex => vec![(Engine::Codex.home_env(), runtime.home.to_string_lossy().into_owned())],
        Engine::Claude => {
            let mut v = vec![
                (
                    Engine::Claude.home_env(),
                    runtime.home.to_string_lossy().into_owned(),
                ),
                (
                    "ANTHROPIC_BASE_URL",
                    profile.provider.base_url.clone(),
                ),
                ("ANTHROPIC_MODEL", profile.model.default.clone()),
                (
                    "ANTHROPIC_SMALL_FAST_MODEL",
                    profile.model.default.clone(),
                ),
                // Forces transcript persistence even if the terminal the
                // user launches from still carries nested-session markers.
                ("CLAUDE_CODE_FORCE_SESSION_PERSISTENCE", "1".to_string()),
            ];
            if let Some(effort) = profile
                .model
                .effort
                .as_deref()
                .filter(|e| !e.trim().is_empty())
            {
                v.push(("CLAUDE_CODE_EFFORT_LEVEL", effort.to_string()));
            }
            v
        }
    }
}

/// Create the isolated runtime plus a start script a system terminal can run.
pub fn prepare_terminal_launch(
    store: &ProfileStore,
    profile: &Profile,
    workspace: &Path,
    extra_args: &[String],
) -> Result<TerminalLaunch> {
    let runtime = create_runtime(store, profile)?;
    if matches!(runtime.engine, Engine::Claude) {
        // Best effort: pre-seed the isolated home so the first launch
        // skips onboarding (theme picker, per-project trust dialog).
        let _ = crate::runtime::seed_claude_home(&runtime.home, Some(workspace));
    }
    let key = resolve_api_key(profile)?;
    let runtime_id = runtime
        .dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();

    let env = non_secret_env(&runtime, profile);
    let binary = runtime.engine.binary();
    let args: Vec<String> = if cfg!(windows) {
        extra_args
            .iter()
            .map(|a| format!("'{}'", ps_quote(a)))
            .collect()
    } else {
        extra_args
            .iter()
            .map(|a| format!("'{}'", sh_quote(a)))
            .collect()
    };
    let arg_suffix = if args.is_empty() {
        String::new()
    } else {
        format!(" {}", args.join(" "))
    };

    let script_path = if cfg!(windows) {
        let mut s = String::new();
        for (k, v) in &env {
            s.push_str(&format!("$env:{k} = '{}'\n", ps_quote(&v)));
        }
        // The API key is injected into the terminal process by
        // open_in_system_terminal at launch; it must never be written to
        // this file (spec §9).
        s.push_str(&format!(
            "Set-Location -LiteralPath '{}'\n",
            ps_quote(workspace.to_string_lossy().as_ref())
        ));
        s.push_str(&format!("{binary}{arg_suffix}\n"));
        let path = runtime.dir.join("start.ps1");
        std::fs::write(&path, s)?;
        path
    } else {
        let mut s = String::new();
        for (k, v) in &env {
            s.push_str(&format!("export {k}='{}'\n", sh_quote(&v)));
        }
        // The API key is exported by the terminal launch command itself;
        // it must never be written to this file (spec §9).
        s.push_str(&format!(
            "cd '{}' || exit 1\n",
            sh_quote(workspace.to_string_lossy().as_ref())
        ));
        s.push_str(&format!("exec {binary}{arg_suffix}\n"));
        let path = runtime.dir.join("start.sh");
        std::fs::write(&path, s)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
        }
        path
    };

    Ok(TerminalLaunch {
        runtime_id,
        script_path,
        runtime,
        env: vec![(
            profile
                .engine()
                .key_env(profile.provider.auth_mode.as_deref())
                .to_string(),
            key,
        )],
    })
}

/// Open the start script in the user's system terminal. The secret env
/// pairs (name, value) are injected into the terminal process (env or
/// launch command) so they never land in the start script on disk
/// (spec §9).
pub fn open_in_system_terminal(
    script: &Path,
    workspace: &Path,
    env: &[(String, String)],
) -> Result<()> {
    if cfg!(windows) {
        // wt / cmd.exe pass their own environment through to the spawned
        // shell — and this process's environment may carry Claude Code
        // nested-session markers (GUI started from inside a Claude Code
        // session). Rebuild it: sanitized parent env + the launch pairs.
        fn launch_env(env: &[(String, String)]) -> Vec<(String, String)> {
            let mut pairs = sanitized_child_env();
            pairs.extend(env.iter().cloned());
            pairs
        }
        let ws = workspace.to_string_lossy().into_owned();
        if which::which("wt").is_ok() {
            let mut cmd = std::process::Command::new("wt");
            cmd.arg("-d").arg(ws);
            cmd.env_clear();
            cmd.envs(launch_env(env));
            cmd.arg("powershell")
                .arg("-NoExit")
                .arg("-ExecutionPolicy")
                .arg("Bypass")
                .arg("-File")
                .arg(script);
            cmd.spawn()
                .map_err(|e| Error::Other(format!("failed to open Windows Terminal: {e}")))?;
        } else {
            let mut cmd = std::process::Command::new("cmd.exe");
            cmd.arg("/C").arg("start").arg("");
            cmd.env_clear();
            cmd.envs(launch_env(env));
            cmd.arg("powershell")
                .arg("-NoExit")
                .arg("-ExecutionPolicy")
                .arg("Bypass")
                .arg("-File")
                .arg(script);
            cmd.spawn()
                .map_err(|e| Error::Other(format!("failed to open a terminal: {e}")))?;
        }
        Ok(())
    } else if cfg!(target_os = "macos") {
        // `open -a Terminal` does not pass the caller's environment through,
        // so run the script via Terminal's AppleScript with the secrets
        // exported inline (in-memory only, never written to the script file).
        let payload = format!(
            "{}; source '{}'",
            sh_env_exports(env),
            sh_quote(script.to_string_lossy().as_ref())
        );
        std::process::Command::new("osascript")
            .arg("-e")
            .arg(format!(
                "tell application \"Terminal\" to do script \"{}\"",
                applescript_quote(&payload)
            ))
            .spawn()
            .map_err(|e| Error::Other(format!("failed to open Terminal: {e}")))?;
        Ok(())
    } else {
        for (term, use_dash_dash) in [
            ("x-terminal-emulator", false),
            ("gnome-terminal", true),
            ("konsole", false),
            ("xterm", false),
        ] {
            if which::which(term).is_ok() {
                let mut cmd = std::process::Command::new(term);
                if use_dash_dash {
                    cmd.arg("--");
                }
                // Terminal emulators do not inherit the caller's environment,
                // so the secrets are exported inline in the launch command.
                cmd.arg("sh")
                    .arg("-c")
                    .arg(format!(
                        "{}; exec '{}'",
                        sh_env_exports(env),
                        sh_quote(script.to_string_lossy().as_ref())
                    ));
                cmd.spawn()
                    .map_err(|e| Error::Other(format!("failed to open {term}: {e}")))?;
                return Ok(());
            }
        }
        Err(Error::Other(format!(
            "no terminal emulator found; run this script manually: {}",
            script.display()
        )))
    }
}

/// Shell `export K='V'` statements joined with "; " (one line).
fn sh_env_exports(env: &[(String, String)]) -> String {
    env.iter()
        .map(|(k, v)| format!("export {k}='{}'", sh_quote(v)))
        .collect::<Vec<_>>()
        .join("; ")
}

/// PowerShell single-quote escaping: double the quote.
fn ps_quote(s: &str) -> String {
    s.replace('\'', "''")
}

/// Shell single-quote escaping: `'` -> `'\''`.
fn sh_quote(s: &str) -> String {
    s.replace('\'', "'\\''")
}

/// Escape a string for inclusion in an AppleScript string literal.
#[allow(dead_code)]
fn applescript_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::{CodexConfig, ModelConfig, ProviderConfig};

    fn profile(
        cli: &str,
        api_key: Option<String>,
        api_key_env: Option<String>,
        auth_mode: Option<String>,
    ) -> Profile {
        Profile {
            id: "t".into(),
            name: "T".into(),
            description: String::new(),
            provider: ProviderConfig {
                provider_type: if cli == "claude" {
                    "relay".into()
                } else {
                    "openai-compatible".into()
                },
                base_url: if cli == "claude" {
                    "https://relay.example.com".into()
                } else {
                    "http://127.0.0.1/v1".into()
                },
                api_key,
                api_key_env,
                auth_mode,
            },
            model: ModelConfig {
                default: if cli == "claude" {
                    "claude-sonnet-4-5".into()
                } else {
                    "gpt-5.6".into()
                },
                effort: None,
            },
            codex: CodexConfig {
                provider_name: "t".into(),
            },
            cli: cli.into(),
        }
    }

    #[test]
    fn stored_key_used_when_no_env() {
        let p = profile("codex", Some("stored-key".into()), None, None);
        assert_eq!(resolve_api_key(&p).unwrap(), "stored-key");
    }

    #[test]
    fn env_var_wins_over_stored_key() {
        std::env::set_var("AS_TEST_KEY_A6F1", "from-env");
        let p = profile(
            "codex",
            Some("stored-key".into()),
            Some("AS_TEST_KEY_A6F1".into()),
            None,
        );
        assert_eq!(resolve_api_key(&p).unwrap(), "from-env");
        std::env::remove_var("AS_TEST_KEY_A6F1");
        assert_eq!(resolve_api_key(&p).unwrap(), "stored-key");
    }

    #[test]
    fn missing_key_resolves_to_empty() {
        // A profile with no key at all (local server) resolves to "".
        let p = profile("codex", None, None, None);
        assert_eq!(resolve_api_key(&p).unwrap(), "");
        // Same when the named env var is unset.
        let p = profile("codex", None, Some("AS_TEST_KEY_DOES_NOT_EXIST_X9".into()), None);
        assert_eq!(resolve_api_key(&p).unwrap(), "");
    }

    #[test]
    fn start_script_never_contains_key() {
        // Spec §9: the key is injected into the terminal process at launch
        // time and must never be written to the start script on disk.
        let dir = std::env::temp_dir().join(format!(
            "as-script-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let store = ProfileStore { root: dir.clone() };
        let p = profile("codex", Some("sk-script-secret".into()), None, None);
        let launch = prepare_terminal_launch(&store, &p, &dir, &[]).unwrap();
        let content = std::fs::read_to_string(&launch.script_path).unwrap();
        assert!(!content.contains("sk-script-secret"));
        // The key is still available in-process for terminal injection.
        assert_eq!(
            launch.env,
            vec![("OPENAI_API_KEY".to_string(), "sk-script-secret".to_string())]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn claude_start_script_sets_anthropic_env_and_never_contains_key() {
        let dir = std::env::temp_dir().join(format!(
            "as-script-claude-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let store = ProfileStore { root: dir.clone() };
        let p = profile(
            "claude",
            Some("sk-claude-secret".into()),
            None,
            Some("auth_token".into()),
        );
        let launch = prepare_terminal_launch(&store, &p, &dir, &[]).unwrap();
        let content = std::fs::read_to_string(&launch.script_path).unwrap();
        // Non-secret runtime env is written to the script (both ps1 and sh
        // spell the variable names the same way).
        assert!(content.contains("CLAUDE_CONFIG_DIR"));
        assert!(content.contains("ANTHROPIC_BASE_URL"));
        assert!(content.contains("ANTHROPIC_MODEL"));
        assert!(content.contains("ANTHROPIC_SMALL_FAST_MODEL"));
        assert!(!content.contains("sk-claude-secret"));
        // The secret env pair uses the auth-token variable by default.
        assert_eq!(
            launch.env,
            vec![(
                "ANTHROPIC_AUTH_TOKEN".to_string(),
                "sk-claude-secret".to_string()
            )]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn start_script_carries_resume_args_and_never_contains_key() {
        // The session pool's resume flow passes the engine's resume args as
        // extra_args; they must land in the start script verbatim.
        let dir = std::env::temp_dir().join(format!(
            "as-script-resume-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let store = ProfileStore { root: dir.clone() };
        // claude → `claude --resume <id>`
        let p = profile("claude", Some("sk-resume-secret".into()), None, None);
        let launch = prepare_terminal_launch(
            &store,
            &p,
            &dir,
            &[
                "--resume".to_string(),
                "aaaa1111-2222-3333-4444-555566667777".to_string(),
            ],
        )
        .unwrap();
        let content = std::fs::read_to_string(&launch.script_path).unwrap();
        assert!(content.contains("--resume"));
        assert!(content.contains("aaaa1111-2222-3333-4444-555566667777"));
        assert!(!content.contains("sk-resume-secret"));
        // codex → `codex resume <id>`
        let pc = profile("codex", None, None, None);
        let launch = prepare_terminal_launch(
            &store,
            &pc,
            &dir,
            &[
                "resume".to_string(),
                "bbbb2222-3333-4444-5555-666677778888".to_string(),
            ],
        )
        .unwrap();
        let content = std::fs::read_to_string(&launch.script_path).unwrap();
        assert!(content.contains("resume"));
        assert!(content.contains("bbbb2222-3333-4444-5555-666677778888"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ambient_session_and_provider_vars_are_recognized() {
        for k in [
            "CLAUDE_CODE_CHILD_SESSION",
            "CLAUDECODE",
            "CLAUDE_PID",
            "CLAUDE_EFFORT",
            "CLAUDE_CODE_SESSION_ID",
            "CLAUDE_SESSION_ANY",
            "ANTHROPIC_BASE_URL",
            "ANTHROPIC_AUTH_TOKEN",
            "anthropic_model", // case-insensitive on Windows
            "OPENAI_API_KEY",
            "OPENAI_BASE_URL",
        ] {
            assert!(is_ambient_env_var(k), "{k} should be stripped");
        }
        for k in [
            "PATH",
            "USERPROFILE",
            "HOME",
            "TERM",
            "ANTHROPICISH",
            "MY_OPENAI_KEY",
            "CODEX_SOMETHING_ELSE",
        ] {
            assert!(!is_ambient_env_var(k), "{k} should be kept");
        }
    }

    #[test]
    fn sanitized_child_env_drops_markers_but_keeps_path() {
        std::env::set_var("CLAUDE_CODE_CHILD_SESSION", "1");
        std::env::set_var("ANTHROPIC_AUTH_TOKEN", "must-not-leak");
        let vars = sanitized_child_env();
        let keys: std::collections::HashSet<String> =
            vars.iter().map(|(k, _)| k.to_ascii_uppercase()).collect();
        assert!(!keys.contains("CLAUDE_CODE_CHILD_SESSION"));
        assert!(!keys.contains("ANTHROPIC_AUTH_TOKEN"));
        // PATH must survive — the child needs it to find its binary.
        assert!(keys.contains("PATH"));
        std::env::remove_var("CLAUDE_CODE_CHILD_SESSION");
        std::env::remove_var("ANTHROPIC_AUTH_TOKEN");
    }

    #[test]
    fn claude_start_script_forces_session_persistence() {
        let dir = std::env::temp_dir().join(format!(
            "as-script-force-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let store = ProfileStore { root: dir.clone() };
        let p = profile("claude", Some("sk-force".into()), None, None);
        let launch = prepare_terminal_launch(&store, &p, &dir, &[]).unwrap();
        let content = std::fs::read_to_string(&launch.script_path).unwrap();
        assert!(content.contains("CLAUDE_CODE_FORCE_SESSION_PERSISTENCE"));
        assert!(content.contains("= '1'") || content.contains("=1"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn claude_api_key_auth_mode_selects_x_api_key_env() {
        let dir = std::env::temp_dir().join(format!(
            "as-script-claude2-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let store = ProfileStore { root: dir.clone() };
        let p = profile(
            "claude",
            Some("sk-claude-2".into()),
            None,
            Some("api_key".into()),
        );
        let launch = prepare_terminal_launch(&store, &p, &dir, &[]).unwrap();
        assert_eq!(
            launch.env,
            vec![("ANTHROPIC_API_KEY".to_string(), "sk-claude-2".to_string())]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
