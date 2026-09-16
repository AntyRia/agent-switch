use std::path::{Path, PathBuf};

use crate::claude::find_claude;
use crate::codex::{find_codex, needs_cmd_wrap};
use crate::engine::Engine;
use crate::error::Result;
use crate::profile::Profile;
use crate::profile_store::ProfileStore;
use crate::runtime::{create_runtime, Runtime};
use crate::settings::Settings;
use crate::terminal;

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

/// The global proxy setting (settings.toml) as env pairs for a launched
/// CLI. Empty when no proxy is configured (direct connection).
///
/// All-case spellings are set so both old-style and new-style HTTP stacks
/// see it, and `NO_PROXY` keeps `localhost`/`127.0.0.1` on the direct path
/// so a local vLLM / SGLang endpoint keeps working while the proxy is on.
pub fn proxy_env_pairs(settings: &Settings) -> Vec<(String, String)> {
    let Some(url) = settings.proxy_url() else {
        return Vec::new();
    };
    const LOOPBACK: &str = "localhost,127.0.0.1";
    vec![
        ("HTTP_PROXY".into(), url.clone()),
        ("HTTPS_PROXY".into(), url.clone()),
        ("ALL_PROXY".into(), url.clone()),
        ("http_proxy".into(), url.clone()),
        ("https_proxy".into(), url.clone()),
        ("all_proxy".into(), url.clone()),
        ("NO_PROXY".into(), LOOPBACK.into()),
        ("no_proxy".into(), LOOPBACK.into()),
    ]
}

/// The CLI bypass-mode flag plus any caller-provided extra args, in launch
/// order (the flag comes first so it is always applied).
///
/// The flag is added only when the caller did not already supply it: both
/// CLIs abort at argument parsing with `the argument … cannot be used
/// multiple times`, and the flag can reach the command line twice — the
/// launcher appends it (dangerous mode) AND the user's own shell
/// alias/function already carries it.
fn launch_args(settings: &Settings, engine: Engine, extra_args: &[String]) -> Vec<String> {
    let mut args = Vec::new();
    if let Some(flag) = settings.dangerous_flag(engine) {
        if !extra_args.iter().any(|a| a == flag) {
            args.push(flag.to_string());
        }
    }
    args.extend(extra_args.iter().cloned());
    args
}

/// The environment variable that carries the profile's key at launch.
/// Official profiles always use the vendor's own convention
/// (`OPENAI_API_KEY` / `ANTHROPIC_API_KEY`) regardless of `auth_mode`;
/// everything else follows the engine + auth_mode rules.
pub fn key_env_for(profile: &Profile) -> String {
    if profile.is_official() {
        return match profile.engine() {
            Engine::Codex => "OPENAI_API_KEY".to_string(),
            Engine::Claude => "ANTHROPIC_API_KEY".to_string(),
        };
    }
    profile
        .engine()
        .key_env(profile.provider.auth_mode.as_deref())
        .to_string()
}

/// Run the profile's CLI in the caller's terminal with an isolated config
/// home and a per-process API key. Waits for the CLI and returns its exit
/// code.
pub fn run_profile(
    store: &ProfileStore,
    profile: &Profile,
    settings: &Settings,
    workspace: &Path,
    extra_args: &[String],
) -> Result<i32> {
    let runtime = create_runtime(store, profile, workspace)?;
    let key = resolve_api_key(profile)?;
    launch_in_runtime(&runtime, profile, settings, key, workspace, extra_args)
}

/// Spawn the profile's CLI against an already-created isolated runtime,
/// with the API key injected as a per-process environment variable. Waits
/// for the CLI and returns its exit code.
pub fn launch_in_runtime(
    runtime: &Runtime,
    profile: &Profile,
    settings: &Settings,
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
    // Official profiles WITHOUT a key run on the logged-in subscription
    // account (credentials in the isolated home). An empty key env var
    // must NOT be set in that case — it would put the CLI into API-key
    // mode with no key and shadow the login state.
    let inject_key = !(profile.is_official() && api_key.is_empty());
    match engine {
        Engine::Codex => {
            if inject_key {
                cmd.env("OPENAI_API_KEY", api_key);
            }
            cmd.env("RUST_BACKTRACE", "1");
        }
        // Anthropic protocol: base URL + model travel as env vars (the
        // isolated .claude dir holds no config at all). The small/fast
        // model is pinned to the same model so relays without a haiku do
        // not break background requests. Official profiles get NO
        // ANTHROPIC_BASE_URL: the CLI's built-in official endpoint is
        // exactly the point of the type.
        Engine::Claude => {
            if !profile.is_official() {
                cmd.env("ANTHROPIC_BASE_URL", &profile.provider.base_url);
            }
            cmd.env("ANTHROPIC_MODEL", &profile.model.default)
                .env("ANTHROPIC_SMALL_FAST_MODEL", &profile.model.default)
                // Belt-and-braces: force transcript persistence even if a
                // nested-session marker slips through (requires Claude
                // Code >= 2.1.172).
                .env("CLAUDE_CODE_FORCE_SESSION_PERSISTENCE", "1");
            if inject_key {
                cmd.env(key_env_for(profile).as_str(), api_key.as_str());
            }
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
    for (k, v) in proxy_env_pairs(settings) {
        cmd.env(k, v);
    }
    for arg in launch_args(settings, engine, extra_args) {
        cmd.arg(arg);
    }
    crate::logging::info(&format!(
        "launched {} '{}' in workspace {}",
        runtime.engine.binary(),
        profile.id,
        workspace.display()
    ));
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
        Engine::Codex => vec![
            (Engine::Codex.home_env(), runtime.home.to_string_lossy().into_owned()),
            // A Codex panic exits 101 with the backtrace on stderr; without
            // this the backtrace is one frame deep and hard to act on. The
            // terminal window stays open (-NoExit), so it is readable.
            ("RUST_BACKTRACE", "1".to_string()),
        ],
        Engine::Claude => {
            let mut v = vec![(
                Engine::Claude.home_env(),
                runtime.home.to_string_lossy().into_owned(),
            )];
            // Official profiles keep the CLI's built-in official endpoint:
            // no ANTHROPIC_BASE_URL override.
            if !profile.is_official() {
                v.push((
                    "ANTHROPIC_BASE_URL",
                    profile.provider.base_url.clone(),
                ));
            }
            v.extend([
                ("ANTHROPIC_MODEL", profile.model.default.clone()),
                (
                    "ANTHROPIC_SMALL_FAST_MODEL",
                    profile.model.default.clone(),
                ),
                // Forces transcript persistence even if the terminal the
                // user launches from still carries nested-session markers.
                ("CLAUDE_CODE_FORCE_SESSION_PERSISTENCE", "1".to_string()),
            ]);
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
///
/// `script_stem` names the generated script (`<stem>.ps1` / `<stem>.sh` in
/// the runtime dir). Fresh launches use `start`; session resumes use
/// `resume-<session-id>` — putting the session id in the script file name
/// keeps it visible in the launched process tree, which is what the
/// "already open" detection greps for (sessions::open_session_ids).
pub fn prepare_terminal_launch(
    store: &ProfileStore,
    profile: &Profile,
    settings: &Settings,
    workspace: &Path,
    extra_args: &[String],
    script_stem: &str,
) -> Result<TerminalLaunch> {
    let runtime = create_runtime(store, profile, workspace)?;
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
    let all_args = launch_args(settings, runtime.engine, extra_args);
    // The API key is injected into the terminal process by
    // open_in_system_terminal at launch; it must never be written to the
    // script file (spec §9).
    let script_path = write_start_script(
        &runtime.dir,
        script_stem,
        &env,
        runtime.engine,
        &all_args,
        workspace,
    )?;

    // Secret/injected env pairs: the API key first, then the global proxy
    // pairs (also injected at open time — they travel the same way so a
    // pinned terminal that does not pass env through still gets them).
    // Official profiles without a key inject NO key var at all — the
    // logged-in subscription account in the isolated home is the
    // credential, and an empty key var would shadow it (API-key mode with
    // no key).
    let mut launch_env: Vec<(String, String)> = Vec::new();
    if !(profile.is_official() && key.is_empty()) {
        launch_env.push((key_env_for(profile), key));
    }
    launch_env.extend(proxy_env_pairs(settings));
    crate::logging::info(&format!(
        "prepared terminal launch for profile '{}' (script {})",
        profile.id,
        script_path.display()
    ));
    Ok(TerminalLaunch {
        runtime_id,
        script_path,
        runtime,
        env: launch_env,
    })
}

/// Write the platform start script (`<stem>.ps1` / `<stem>.sh`) into
/// `dir`: the (non-secret) env assignments, a change into `workspace`, and
/// the CLI command line.
///
/// The binary is invoked by its FULL resolved path (quoted), not the bare
/// name: a bare `codex`/`claude` goes through the user's shell
/// aliases/functions first — e.g. a PowerShell profile wrapper that
/// already appends a bypass flag — which duplicates the argument and
/// makes the CLI abort (`… cannot be used multiple times`). A quoted full
/// path skips name resolution entirely. If the binary cannot be resolved
/// (should not happen: the GUI checks status first, the CLI pre-checks)
/// the bare name is used so the launch still fails with the usual
/// "command not found" at run time.
fn write_start_script(
    dir: &Path,
    stem: &str,
    env: &[(&'static str, String)],
    engine: Engine,
    args: &[String],
    workspace: &Path,
) -> Result<PathBuf> {
    let quoted_args: Vec<String> = if cfg!(windows) {
        args.iter().map(|a| format!("'{}'", ps_quote(a))).collect()
    } else {
        args.iter().map(|a| format!("'{}'", sh_quote(a))).collect()
    };
    let arg_suffix = if quoted_args.is_empty() {
        String::new()
    } else {
        format!(" {}", quoted_args.join(" "))
    };
    let bin = match find_engine_binary(engine) {
        Ok(p) => p.to_string_lossy().into_owned(),
        Err(_) => engine.binary().to_string(),
    };

    if cfg!(windows) {
        let mut s = String::new();
        for (k, v) in env {
            s.push_str(&format!("$env:{k} = '{}'\n", ps_quote(v)));
        }
        s.push_str(&format!(
            "Set-Location -LiteralPath '{}'\n",
            ps_quote(workspace.to_string_lossy().as_ref())
        ));
        // `&` (call operator) is required for a quoted command path.
        s.push_str(&format!("& '{}'{}\n", ps_quote(&bin), arg_suffix));
        let path = dir.join(format!("{stem}.ps1"));
        // Windows PowerShell 5.1 decodes a BOM-less .ps1 using the system
        // ANSI codepage (GBK on a Chinese Windows), which turns a non-ASCII
        // workspace path into mojibake and makes Set-Location fail. The
        // UTF-8 BOM forces UTF-8 decoding; PowerShell 7 reads it the same.
        std::fs::write(&path, format!("\u{feff}{s}"))?;
        Ok(path)
    } else {
        let mut s = String::from("#!/bin/sh\n");
        for (k, v) in env {
            s.push_str(&format!("export {k}='{}'\n", sh_quote(v)));
        }
        s.push_str(&format!(
            "cd '{}' || exit 1\n",
            sh_quote(workspace.to_string_lossy().as_ref())
        ));
        s.push_str(&format!("exec '{}'{}\n", sh_quote(&bin), arg_suffix));
        let path = dir.join(format!("{stem}.sh"));
        std::fs::write(&path, s)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
        }
        Ok(path)
    }
}

/// The CLI arguments that drive the official-account login flow. Codex has
/// a dedicated subcommand (`codex login`, which itself offers browser
/// OAuth, `--with-api-key` and `--device-auth`); Claude Code started in a
/// FRESH isolated home (no `.credentials.json` yet) opens its own login
/// screen on start, so it needs no argument at all.
pub fn login_args(engine: Engine) -> Vec<String> {
    match engine {
        Engine::Codex => vec!["login".to_string()],
        Engine::Claude => Vec::new(),
    }
}

/// Run the official-account login flow in the caller's own terminal (the
/// CLI `login` command): isolated home, NO provider vars at all (a login
/// must hit the official endpoint with a clean environment), stdio
/// inherited so the interactive browser flow stays visible. Waits for the
/// CLI and returns its exit code.
pub fn login_in_runtime(
    runtime: &Runtime,
    profile: &Profile,
    workspace: &Path,
) -> Result<i32> {
    let engine = runtime.engine;
    if matches!(engine, Engine::Claude) {
        // Best effort: pre-seed so only the login screen appears.
        let _ = crate::runtime::seed_claude_home(&runtime.home, None);
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
    cmd.env_clear();
    cmd.envs(sanitized_child_env());
    cmd.current_dir(workspace).env(engine.home_env(), &runtime.home);
    for arg in login_args(engine) {
        cmd.arg(arg);
    }
    crate::logging::info(&format!(
        "login flow for '{}' (home {})",
        profile.id,
        runtime.home.display()
    ));
    let status = cmd.status()?;
    Ok(status.code().unwrap_or(1))
}

/// Prepare a terminal launch for the official-account login flow (the GUI
/// login button): a start script carrying ONLY the isolated-home env — no
/// provider vars, no key — running `codex login` / `claude`. Returns a
/// `TerminalLaunch` with an EMPTY secret list (there is nothing secret to
/// inject for a login).
pub fn prepare_login(
    store: &ProfileStore,
    profile: &Profile,
    workspace: &Path,
) -> Result<TerminalLaunch> {
    let runtime = create_runtime(store, profile, workspace)?;
    let runtime_id = runtime
        .dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let env: Vec<(&'static str, String)> = vec![(
        runtime.engine.home_env(),
        runtime.home.to_string_lossy().into_owned(),
    )];
    let script_path = write_start_script(
        &runtime.dir,
        "login",
        &env,
        runtime.engine,
        &login_args(runtime.engine),
        workspace,
    )?;
    crate::logging::info(&format!(
        "prepared login for profile '{}' (script {})",
        profile.id,
        script_path.display()
    ));
    Ok(TerminalLaunch {
        runtime_id,
        script_path,
        runtime,
        env: Vec::new(),
    })
}

/// Open the start script in the user's terminal (see `terminal` for the
/// per-OS recipes and the auto-detect / pinned-terminal logic). The secret
/// env pairs (name, value) are injected into the terminal process (env or
/// launch command) so they never land in the start script on disk
/// (spec §9).
///
/// `terminal_override` is the user's pinned terminal path ("" = auto-detect
/// the first available terminal).
pub fn open_in_system_terminal(
    script: &Path,
    workspace: &Path,
    env: &[(String, String)],
    terminal_override: &str,
) -> Result<terminal::TerminalOutcome> {
    terminal::open_in_terminal(script, workspace, env, terminal_override)
}

/// PowerShell single-quote escaping: double the quote.
fn ps_quote(s: &str) -> String {
    s.replace('\'', "''")
}

/// Shell single-quote escaping: `'` -> `'\''`.
fn sh_quote(s: &str) -> String {
    s.replace('\'', "'\\''")
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
                context_window: None,
                models: Vec::new(),
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
        let launch =
            prepare_terminal_launch(&store, &p, &Settings::default(), &dir, &[], "start").unwrap();
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
    #[cfg(windows)]
    fn windows_start_script_is_utf8_with_bom() {
        // Windows PowerShell 5.1 reads a BOM-less .ps1 as the system ANSI
        // codepage (GBK on a Chinese Windows), mangling a non-ASCII
        // workspace path into mojibake and failing Set-Location. The script
        // must carry a UTF-8 BOM so the path round-trips correctly.
        let dir = std::env::temp_dir().join(format!(
            "as-script-bom-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let store = ProfileStore { root: dir.clone() };
        let p = profile("codex", Some("sk-bom".into()), None, None);
        let launch =
            prepare_terminal_launch(&store, &p, &Settings::default(), &dir, &[], "start").unwrap();
        let bytes = std::fs::read(&launch.script_path).unwrap();
        assert!(
            bytes.starts_with(b"\xef\xbb\xbf"),
            "start.ps1 must begin with a UTF-8 BOM (Windows PowerShell 5.1 would otherwise decode it as the ANSI codepage)"
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
        let launch =
            prepare_terminal_launch(&store, &p, &Settings::default(), &dir, &[], "start").unwrap();
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
            &Settings::default(),
            &dir,
            &[
                "--resume".to_string(),
                "aaaa1111-2222-3333-4444-555566667777".to_string(),
            ],
            "start",
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
            &Settings::default(),
            &dir,
            &[
                "resume".to_string(),
                "bbbb2222-3333-4444-5555-666677778888".to_string(),
            ],
            "start",
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
        let launch =
            prepare_terminal_launch(&store, &p, &Settings::default(), &dir, &[], "start").unwrap();
        let content = std::fs::read_to_string(&launch.script_path).unwrap();
        assert!(content.contains("CLAUDE_CODE_FORCE_SESSION_PERSISTENCE"));
        assert!(content.contains("= '1'") || content.contains("='1'") || content.contains("=1"));
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
        let launch =
            prepare_terminal_launch(&store, &p, &Settings::default(), &dir, &[], "start").unwrap();
        assert_eq!(
            launch.env,
            vec![("ANTHROPIC_API_KEY".to_string(), "sk-claude-2".to_string())]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn proxy_env_pairs_follow_the_settings() {
        assert!(proxy_env_pairs(&Settings::default()).is_empty());
        let mut s = Settings::default();
        s.proxy_host = "10.0.0.1".into();
        s.proxy_port = 8080;
        let pairs = proxy_env_pairs(&s);
        let flat = pairs
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(flat.contains("HTTP_PROXY=http://10.0.0.1:8080"));
        assert!(flat.contains("ALL_PROXY=http://10.0.0.1:8080"));
        assert!(flat.contains("all_proxy=http://10.0.0.1:8080"));
        // Loopback stays direct so a local vLLM keeps working.
        assert!(flat.contains("NO_PROXY=localhost,127.0.0.1"));
        assert!(flat.contains("no_proxy=localhost,127.0.0.1"));
    }

    #[test]
    fn settings_lands_in_terminal_launch() {
        let dir = std::env::temp_dir().join(format!(
            "as-script-settings-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let store = ProfileStore { root: dir.clone() };
        let mut s = Settings::default();
        s.dangerous_mode = true;
        s.proxy_host = "127.0.0.1".into();
        s.proxy_port = 7897;
        let p = profile("codex", Some("sk-settings-secret".into()), None, None);
        let launch =
            prepare_terminal_launch(&store, &p, &s, &dir, &[], "start").unwrap();
        let content = std::fs::read_to_string(&launch.script_path).unwrap();
        // The bypass flag is baked into the generated script...
        assert!(content.contains("--dangerously-bypass-approvals-and-sandbox"));
        // ...while the proxy pairs travel in the in-process env vec,
        // never into the script file.
        let has = |kv: &[(String, String)], k: &str| {
            kv.iter().any(|(kk, _)| kk == k)
        };
        assert!(has(&launch.env, "OPENAI_API_KEY"));
        assert!(has(&launch.env, "HTTP_PROXY"));
        assert!(has(&launch.env, "NO_PROXY"));
        assert!(!content.contains("sk-settings-secret"));
        assert!(!content.contains("HTTP_PROXY"));
        // Default settings: no flag, no proxy.
        let launch = prepare_terminal_launch(&store, &p, &Settings::default(), &dir, &[], "start2")
            .unwrap();
        let content = std::fs::read_to_string(&launch.script_path).unwrap();
        assert!(!content.contains("--dangerously"));
        assert_eq!(launch.env.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dangerous_flag_is_never_duplicated() {
        // The bypass flag can reach the command line twice: the launcher
        // appends it (dangerous mode) AND the caller's own args already
        // carry it (a user shell alias/function wrapper, or
        // `agent-switch run <id> -- <flag>`). Both CLIs abort at argument
        // parsing with "cannot be used multiple times" — so it must be
        // deduplicated.
        let dir = std::env::temp_dir().join(format!(
            "as-flag-dup-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let store = ProfileStore { root: dir.clone() };
        let mut s = Settings::default();
        s.dangerous_mode = true;
        // codex: caller already passed the flag → exactly one occurrence.
        let flag = "--dangerously-bypass-approvals-and-sandbox".to_string();
        let p = profile("codex", Some("sk-dup".into()), None, None);
        let launch =
            prepare_terminal_launch(&store, &p, &s, &dir, &[flag.clone()], "start").unwrap();
        let content = std::fs::read_to_string(&launch.script_path).unwrap();
        assert_eq!(
            content.matches(&flag).count(),
            1,
            "flag duplicated in script:\n{content}"
        );
        // No caller args → the launcher adds it, still exactly once.
        let launch =
            prepare_terminal_launch(&store, &p, &s, &dir, &[], "start2").unwrap();
        let content = std::fs::read_to_string(&launch.script_path).unwrap();
        assert_eq!(content.matches(&flag).count(), 1);
        // claude flag, same rule.
        let p = profile("claude", Some("sk-dup2".into()), None, None);
        let cflag = "--dangerously-skip-permissions".to_string();
        let launch =
            prepare_terminal_launch(&store, &p, &s, &dir, &[cflag.clone()], "start3").unwrap();
        let content = std::fs::read_to_string(&launch.script_path).unwrap();
        assert_eq!(content.matches(&cflag).count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn official_claude_launch_skips_base_url_and_key_when_keyless() {
        // Official profile WITHOUT a key: no ANTHROPIC_BASE_URL (the CLI's
        // built-in official endpoint is the point) and NO key env pair —
        // the subscription login stored in the isolated home is the
        // credential; an empty key var would shadow it (API-key mode with
        // no key).
        let dir = std::env::temp_dir().join(format!(
            "as-official-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let store = ProfileStore { root: dir.clone() };
        let mut p = profile("claude", None, None, None);
        p.provider.provider_type = "official".into();
        let launch =
            prepare_terminal_launch(&store, &p, &Settings::default(), &dir, &[], "start").unwrap();
        let content = std::fs::read_to_string(&launch.script_path).unwrap();
        assert!(content.contains("CLAUDE_CONFIG_DIR"));
        assert!(!content.contains("ANTHROPIC_BASE_URL"));
        assert!(content.contains("ANTHROPIC_MODEL"));
        assert!(launch.env.is_empty(), "no key pair for keyless official");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn official_profiles_use_the_vendor_key_env() {
        // claude official + key → ANTHROPIC_API_KEY (the vendor convention,
        // never the relay's auth-token variable — regardless of auth_mode).
        let dir = std::env::temp_dir().join(format!(
            "as-official-key-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let store = ProfileStore { root: dir.clone() };
        let mut p = profile(
            "claude",
            Some("sk-official".into()),
            None,
            Some("auth_token".into()),
        );
        p.provider.provider_type = "official".into();
        let launch =
            prepare_terminal_launch(&store, &p, &Settings::default(), &dir, &[], "start").unwrap();
        assert_eq!(
            launch.env,
            vec![("ANTHROPIC_API_KEY".to_string(), "sk-official".to_string())]
        );
        let content = std::fs::read_to_string(&launch.script_path).unwrap();
        assert!(!content.contains("ANTHROPIC_BASE_URL"));
        // codex official + key → OPENAI_API_KEY.
        let mut pc = profile("codex", Some("sk-official".into()), None, None);
        pc.provider.provider_type = "official".into();
        let launch =
            prepare_terminal_launch(&store, &pc, &Settings::default(), &dir, &[], "start2").unwrap();
        assert_eq!(
            launch.env,
            vec![("OPENAI_API_KEY".to_string(), "sk-official".to_string())]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn prepare_login_script_carries_only_the_isolated_home() {
        let dir = std::env::temp_dir().join(format!(
            "as-login-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let store = ProfileStore { root: dir.clone() };
        let mut p = profile("claude", None, None, None);
        p.provider.provider_type = "official".into();
        let launch = prepare_login(&store, &p, &dir).unwrap();
        let content = std::fs::read_to_string(&launch.script_path).unwrap();
        // Only the isolated-home env: no provider vars, no key, no flag —
        // a login must hit the official endpoint with a clean environment.
        assert!(content.contains("CLAUDE_CONFIG_DIR"));
        assert!(!content.contains("ANTHROPIC_BASE_URL"));
        assert!(!content.contains("ANTHROPIC_MODEL"));
        assert!(!content.contains("--dangerously"));
        // Nothing secret to inject for a login.
        assert!(launch.env.is_empty());
        // codex login runs the `login` subcommand.
        let pc = profile("codex", None, None, None);
        let launch = prepare_login(&store, &pc, &dir).unwrap();
        let content = std::fs::read_to_string(&launch.script_path).unwrap();
        assert!(content.contains("login"));
        assert!(!content.contains("OPENAI_API_KEY"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
