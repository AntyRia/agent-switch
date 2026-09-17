//! agent-switch CLI — profile-driven Codex / Claude multi-provider launcher.
//!
//! All profile/runtime/launch logic lives in `agent-switch-core`; this
//! binary is only the command-line layer on top of that API.

use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::time::SystemTime;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};

use agent_switch_core::{
    claude, codex, engine::Engine, health, launcher, logging, models,
    profile::{Profile, ProviderConfig, ModelConfig, CodexConfig},
    profile_store::ProfileStore,
    runtime, sessions, settings::Settings, update, validation,
};

#[derive(Parser)]
#[command(
    name = "agent-switch",
    version,
    about = "Profile-driven Codex / Claude multi-provider launcher"
)]
struct Cli {
    /// The command to run; with none, a quick-start guide is printed
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Create the config root (profiles/ + runtime/); idempotent
    Init,
    /// List all provider profiles
    List,
    /// Show one provider profile (API key is masked)
    Show {
        /// Profile id
        profile: String,
    },
    /// Add a new provider profile interactively (the id is generated
    /// automatically as a UUID)
    Add,
    /// Edit a profile with $EDITOR (fallback: notepad / vi). The id field
    /// cannot be changed.
    Edit {
        /// Profile id
        profile: String,
    },
    /// Remove a profile (asks for confirmation unless --yes)
    Remove {
        /// Profile id
        profile: String,
        /// Skip the confirmation prompt (for scripts)
        #[arg(short, long)]
        yes: bool,
    },
    /// Test the provider endpoint of a profile
    Test {
        /// Profile id
        profile: String,
    },
    /// Fetch the provider's model list for a profile
    Models {
        /// Profile id
        profile: String,
    },
    /// List resumable sessions from all profile runtimes
    Sessions,
    /// Run Codex or Claude with a profile in an isolated runtime
    Run {
        /// Profile id
        profile: String,
        /// Workspace directory (default: current directory)
        workspace: Option<PathBuf>,
        /// Extra arguments passed to the CLI, after `--`
        #[arg(last = true, allow_hyphen_values = true)]
        extra_args: Vec<String>,
    },
    /// Log in to the official account for an "official" profile: runs
    /// `codex login` / `claude` in the profile's isolated home so the
    /// subscription credentials stay per-profile (no API key needed after)
    Login {
        /// Profile id
        profile: String,
        /// Workspace directory (default: current directory)
        workspace: Option<PathBuf>,
    },
    /// Check that the Codex/Claude CLIs and the config directories are in place
    Doctor,
    /// Delete old runtime directories (keeps the newest 20)
    Cleanup,
    /// Show the last lines of the log file (troubleshooting)
    Logs {
        /// Number of lines (default 50)
        #[arg(default_value_t = 50)]
        lines: usize,
    },
    /// Show or edit the global launch settings (dangerous mode, proxy,
    /// terminal)
    Settings {
        /// Open settings.toml in $EDITOR instead of printing the values
        #[arg(short, long)]
        edit: bool,
    },
    /// Check the official releases and self-update the CLI binary in
    /// place (`--check` only reports)
    Update {
        /// Only check and report; do not download or replace anything
        #[arg(short, long)]
        check: bool,
    },
}

fn main() {
    // A Windows self-update leaves "<binary>.old" that the next launch
    // can finally delete.
    if let Ok(exe) = std::env::current_exe() {
        update::remove_stale_old(&exe);
    }
    let cli = Cli::parse();
    let store = ProfileStore::new();
    // File logging for troubleshooting (silent on failure; the log file is
    // shared with the GUI's log viewer).
    logging::init(&store.root);
    // New-version notice on every launch (never blocks the command).
    if !matches!(cli.command, Some(Command::Update { .. })) {
        maybe_print_update_notice();
    }
    if let Err(err) = dispatch(cli, &store) {
        logging::error(&format!("cli error: {err:#}"));
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}

fn dispatch(cli: Cli, store: &ProfileStore) -> Result<()> {
    // No subcommand: a short, friendly quick-start instead of an error.
    let Some(command) = cli.command else {
        print_guide();
        return Ok(());
    };
    match command {
        Command::Init => cmd_init(store),
        Command::List => cmd_list(store),
        Command::Show { profile } => cmd_show(store, &profile),
        Command::Add => cmd_add(store),
        Command::Edit { profile } => cmd_edit(store, &profile),
        Command::Remove {
            profile,
            yes,
        } => cmd_remove(store, &profile, yes),
        Command::Test { profile } => cmd_test(store, &profile),
        Command::Models { profile } => cmd_models(store, &profile),
        Command::Sessions => cmd_sessions(store),
        Command::Run {
            profile,
            workspace,
            extra_args,
        } => cmd_run(store, &profile, workspace, &extra_args),
        Command::Login {
            profile,
            workspace,
        } => cmd_login(store, &profile, workspace),
        Command::Doctor => cmd_doctor(store),
        Command::Cleanup => cmd_cleanup(store),
        Command::Logs { lines } => cmd_logs(lines),
        Command::Settings { edit } => cmd_settings(edit),
        Command::Update { check } => cmd_update(check),
    }
}

/// Quick-start guide printed when the binary runs without a subcommand.
fn print_guide() {
    println!("agent-switch — profile-driven Codex / Claude multi-provider launcher");
    println!();
    println!("Quick start:");
    println!("  1. agent-switch doctor      check the local Codex / Claude CLIs");
    println!("  2. agent-switch add         create a provider profile (interactive)");
    println!("  3. agent-switch run <id>    launch that profile in an isolated runtime");
    println!();
    println!("Profiles:   list   show <id>   edit <id>   remove <id>   add");
    println!("Provider:   test <id>   models <id>");
    println!("Sessions:   sessions");
    println!("Launch:     run <profile> [-- <workspace>] [-- <extra CLI args>]");
    println!("            login <profile>        official profiles only (subscription)");
    println!("System:     init   doctor   cleanup   logs [n]   settings [--edit]   update");
    println!();
    println!("Run 'agent-switch <command> --help' for details on one command.");
}

/// Shared "profile not found" message with a pointer to `list`.
fn get_profile(store: &ProfileStore, id: &str) -> Result<Profile> {
    match store.get(id) {
        Ok(p) => Ok(p),
        Err(agent_switch_core::error::Error::ProfileNotFound(_)) => {
            bail!("profile not found: {id} (run 'agent-switch list' to see available profiles)")
        }
        Err(e) => Err(e.into()),
    }
}

/// Friendly missing-CLI notice (spec wording + the npm install command).
fn print_cli_missing(engine: Engine) {
    match engine {
        Engine::Codex => {
            println!("Codex CLI not found.");
            println!("Please install Codex CLI first:");
            println!("  npm install -g @openai/codex");
        }
        Engine::Claude => {
            println!("Claude CLI not found.");
            println!("Please install Claude CLI first:");
            println!("  npm install -g @anthropic-ai/claude-code");
        }
    }
}

/// `init` — create the directory layout (idempotent), print created paths.
fn cmd_init(store: &ProfileStore) -> Result<()> {
    store.init()?;
    println!("Initialized agent-switch at {}", store.root.display());
    println!("  profiles: {}", store.profiles_dir().display());
    println!("  runtime:  {}", store.runtime_dir().display());
    Ok(())
}

/// `list` — ID / NAME / CLI / MODEL table with a dash underline.
fn cmd_list(store: &ProfileStore) -> Result<()> {
    let profiles = store.list()?;
    if profiles.is_empty() {
        println!("no profiles yet — run: agent-switch add");
        return Ok(());
    }
    let mut id_w = 2usize;
    let mut name_w = 4usize;
    let mut cli_w = 3usize;
    for p in &profiles {
        id_w = id_w.max(p.id.chars().count());
        name_w = name_w.max(p.name.chars().count());
        cli_w = cli_w.max(p.engine().value().len());
    }
    let header = format!(
        "{:<w1$}  {:<w2$}  {:<w3$}  {}",
        "ID", "NAME", "CLI", "MODEL",
        w1 = id_w,
        w2 = name_w,
        w3 = cli_w
    );
    println!("{header}");
    println!("{}", "-".repeat(header.chars().count()));
    for p in &profiles {
        println!(
            "{:<w1$}  {:<w2$}  {:<w3$}  {}",
            p.id, p.name, p.engine().value(), p.model.default,
            w1 = id_w,
            w2 = name_w,
            w3 = cli_w
        );
    }
    Ok(())
}

/// `show` — full profile; the API key is masked, never printed in full.
fn cmd_show(store: &ProfileStore, id: &str) -> Result<()> {
    let p = get_profile(store, id)?;
    println!("id: {}", p.id);
    println!("name: {}", p.name);
    println!("description: {}", p.description);
    println!("cli: {}", p.engine().value());
    println!();
    println!("provider.type: {}", p.provider.provider_type);
    println!("provider.base_url: {}", p.provider.base_url);
    if let Some(env) = p
        .provider
        .api_key_env
        .as_deref()
        .filter(|e| !e.is_empty())
    {
        println!("provider.api_key_env: {env}");
    } else if let Some(key) = p.provider.api_key.as_deref() {
        println!("provider.api_key: {}", mask_key(key));
    } else {
        println!("provider.api_key: (not set)");
    }
    if let Some(mode) = p
        .provider
        .auth_mode
        .as_deref()
        .filter(|m| !m.is_empty())
    {
        println!("provider.auth_mode: {mode}");
    }
    println!();
    println!("model.default: {}", p.model.default);
    if let Some(effort) = p
        .model
        .effort
        .as_deref()
        .filter(|e| !e.trim().is_empty())
    {
        println!("model.effort: {effort}");
    }
    println!();
    println!("codex.provider_name: {}", p.codex.provider_name);
    Ok(())
}

/// Mask an API key: first 4 + "…" + last 4 when longer than 8 chars, else "****".
fn mask_key(key: &str) -> String {
    if key.chars().count() > 8 {
        let chars: Vec<char> = key.chars().collect();
        let n = chars.len();
        let first: String = chars[..4].iter().collect();
        let last: String = chars[n - 4..].iter().collect();
        format!("{first}…{last}")
    } else {
        "****".to_string()
    }
}

/// `add` — read the fields from stdin in a fixed order (pipe-friendly).
/// Empty answers take the bracketed defaults; description is not prompted
/// and stays empty. The id is generated automatically (UUID v4) and shown
/// after creation — it is not asked for, and cannot be changed later.
fn cmd_add(store: &ProfileStore) -> Result<()> {
    println!("Creating a new profile — press Enter to accept the [default].");
    let stdin = io::stdin();
    let mut lines = stdin.lock().lines();
    let name = ask_line(&mut lines, "Profile name:")?;
    let cli_raw = ask_line(
        &mut lines,
        "CLI [codex / claude] (default codex; codex = OpenAI protocol, claude = Anthropic protocol):",
    )?;
    let cli = match cli_raw.as_str() {
        "" => "codex".to_string(),
        c => c.trim().to_ascii_lowercase(),
    };
    println!(
        "  provider types: relay = third-party relay API · vllm = self-hosted \
         (vLLM / SGLang / llama.cpp …) · official = vendor API / subscription login"
    );
    let type_raw = ask_line(
        &mut lines,
        "Provider type [relay / vllm / official] (default relay):",
    )?;
    let provider_type = match type_raw.as_str() {
        "" => "relay".to_string(),
        "relay" => "relay".to_string(),
        t if t.trim().to_ascii_lowercase().starts_with("openai")
            || t.trim().to_ascii_lowercase() == "vllm" =>
        {
            "openai-compatible".to_string()
        }
        t if t.trim().to_ascii_lowercase() == "official" => "official".to_string(),
        t => t.trim().to_ascii_lowercase(),
    };
    let base_url = if provider_type == "official" {
        // Official: the vendor endpoint is built in; an empty answer keeps
        // the vendor default (recorded for display, never sent as override).
        let raw = ask_line(&mut lines, "Base URL (official: leave empty for the vendor default):")?;
        if raw.is_empty() {
            match cli.as_str() {
                "claude" => "https://api.anthropic.com".to_string(),
                _ => "https://api.openai.com/v1".to_string(),
            }
        } else {
            raw
        }
    } else {
        ask_line(&mut lines, "Base URL (e.g. https://api.example.com/v1):")?
    };
    let api_key = ask_line(
        &mut lines,
        if provider_type == "official" {
            "API Key (official: optional — a subscription login via `agent-switch login` also works; leave empty to use login):"
        } else {
            "API Key (leave empty only for local servers with no auth):"
        },
    )?;
    let model = ask_line(&mut lines, "Default model (e.g. gpt-5 / claude-sonnet-4-5):")?;
    // Key transport is only meaningful for the claude engine, and fixed to
    // the vendor's x-api-key convention for official providers.
    let auth_mode = if cli == "claude" && provider_type != "official" {
        let raw = ask_line(&mut lines, "Key mode [auth_token/api_key] (default auth_token):")?;
        match raw.as_str() {
            "" => None,
            m => Some(m.trim().to_ascii_lowercase()),
        }
    } else {
        None
    };

    // Auto-generated UUID id (unique; hidden in the GUI, shown here).
    let id = uuid::Uuid::new_v4().to_string();
    // The codex provider name is a stable slug derived from the display
    // name (falls back to the id when the name has no usable characters).
    let provider_name: String = name
        .to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    let provider_name = if provider_name.is_empty() {
        id.clone()
    } else {
        provider_name
    };

    let profile = Profile {
        id: id.clone(),
        name,
        description: String::new(),
        provider: ProviderConfig {
            provider_type,
            base_url,
            api_key: if api_key.is_empty() {
                None
            } else {
                Some(api_key)
            },
            api_key_env: None,
            auth_mode,
        },
        model: ModelConfig {
            default: model,
            effort: None,
            context_window: None,
            models: Vec::new(),
        },
        codex: CodexConfig {
            provider_name,
        },
        cli,
    };

    let errors = validation::validate_profile(&profile);
    if !errors.is_empty() {
        for e in &errors {
            eprintln!("error: {e}");
        }
        std::process::exit(1);
    }
    store.init().ok();
    let path = store.save(&profile, None)?;
    logging::info(&format!("created profile {id}"));
    println!("Created {}", path.display());
    println!("ID: {id}");
    println!();
    println!("Next steps:");
    println!("  agent-switch test {id}    check the endpoint");
    println!("  agent-switch run {id}     launch the CLI with this profile");
    Ok(())
}

/// Print a prompt, then read and trim one line of stdin.
fn ask_line<'a>(lines: &mut io::Lines<io::StdinLock<'a>>, prompt: &str) -> Result<String> {
    println!("{prompt}");
    io::stdout().flush().context("failed to write to stdout")?;
    let line = lines
        .next()
        .transpose()
        .context("failed to read from stdin")?
        .unwrap_or_default();
    Ok(line.trim().to_string())
}

/// The user's editor: $EDITOR when set, else notepad (Windows) / vi.
fn current_editor() -> String {
    std::env::var("EDITOR")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| {
            if cfg!(windows) {
                "notepad".to_string()
            } else {
                "vi".to_string()
            }
        })
}

/// `edit` — open the profile TOML in the user's editor, wait, re-validate.
fn cmd_edit(store: &ProfileStore, id: &str) -> Result<()> {
    // Fail early with a clear error if the profile does not exist.
    get_profile(store, id)?;
    let path = store.path_for(id);
    let editor = current_editor();
    let status = match std::process::Command::new(&editor).arg(&path).status() {
        Ok(s) => s,
        Err(e) => bail!("failed to run editor {editor}: {e}"),
    };
    if !status.success() {
        bail!("editor exited with status {status:?}; profile left unchanged");
    }
    // Re-read and validate whatever the editor saved.
    let p = match store.get(id) {
        Ok(p) => p,
        Err(e) => bail!("failed to re-read {} after editing: {e}", path.display()),
    };
    // Ids are auto-generated (UUID) and immutable: they are the file name
    // and the key for runtimes/sessions, so renaming is refused.
    if p.id != id {
        bail!(
            "the id field cannot be changed (it is auto-generated); \
             revert it to \"{id}\" and save again"
        );
    }
    let errors = validation::validate_profile(&p);
    if !errors.is_empty() {
        for e in &errors {
            eprintln!("error: {e}");
        }
        std::process::exit(1);
    }
    println!("saved");
    Ok(())
}

/// `remove` — confirm with y/yes (case-insensitive), otherwise "aborted".
/// `--yes` skips the prompt for scripts.
fn cmd_remove(store: &ProfileStore, id: &str, yes: bool) -> Result<()> {
    get_profile(store, id)?;
    if !yes {
        print!("Delete profile \"{id}\"? [y/N] ");
        io::stdout().flush().context("failed to write to stdout")?;
        let line = io::stdin()
            .lock()
            .lines()
            .next()
            .transpose()
            .context("failed to read from stdin")?
            .unwrap_or_default()
            .trim()
            .to_lowercase();
        if line != "y" && line != "yes" {
            println!("aborted");
            return Ok(());
        }
    }
    store.delete(id)?;
    println!("Deleted profile \"{id}\"");
    Ok(())
}

/// `test` — health-check the profile's endpoint with the resolved API key.
/// The engine picks the wire protocol (codex → GET /models, claude → POST
/// /v1/messages with a 1-token request).
fn cmd_test(store: &ProfileStore, id: &str) -> Result<()> {
    let p = get_profile(store, id)?;
    let key = launcher::resolve_api_key(&p)?;
    let result = health::test_engine(
        p.engine(),
        &p.provider.base_url,
        &key,
        &p.model.default,
        p.provider.auth_mode.as_deref(),
        p.is_official(),
    )?;
    println!("Profile: {}", p.id);
    if result.ok {
        println!("Connection: ✓ Success");
    } else {
        println!("Connection: ✗ Failed: {}", result.message);
    }
    println!("Endpoint: {}", p.provider.base_url);
    println!("Model: {}", p.model.default);
    if !result.ok {
        std::process::exit(1);
    }
    Ok(())
}

/// `models` — fetch the provider's model list with the resolved API key.
fn cmd_models(store: &ProfileStore, id: &str) -> Result<()> {
    let p = get_profile(store, id)?;
    let key = launcher::resolve_api_key(&p)?;
    // Listing is a key-only feature: a keyless official profile relies on a
    // subscription login, and that login state has no list endpoint.
    if p.is_official() && key.trim().is_empty() {
        bail!("no key set — fetching the model list requires a key (subscription login has no list endpoint)");
    }
    // Official claude always speaks the vendor's x-api-key convention.
    let auth_mode = if p.is_official() { Some("api_key") } else { p.provider.auth_mode.as_deref() };
    let list = models::fetch_models(p.engine(), &p.provider.base_url, &key, auth_mode)?;
    if !list.ok {
        bail!("failed to fetch models: {}", list.message);
    }
    if list.models.is_empty() {
        println!(
            "no models reported by {} ({})",
            p.provider.base_url, list.message
        );
        return Ok(());
    }
    for m in &list.models {
        println!("{m}");
    }
    Ok(())
}

/// `sessions` — the conversation pool across all per-profile runtimes.
fn cmd_sessions(store: &ProfileStore) -> Result<()> {
    let pool = sessions::list_sessions(store)?;
    if pool.is_empty() {
        println!("no sessions yet — launch a profile first");
        return Ok(());
    }
    let mut id_w = 7usize;
    for s in &pool {
        id_w = id_w.max(s.profile_id.chars().count());
    }
    let header = format!(
        "{:<36}  {:<6}  {:<w$}  {:<11}  {}",
        "SESSION", "CLI", "PROFILE", "LAST ACTIVE", "PREVIEW",
        w = id_w
    );
    println!("{header}");
    println!("{}", "-".repeat(header.chars().count()));
    for s in &pool {
        println!(
            "{:<36}  {:<6}  {:<w$}  {:<11}  {}",
            s.session_id,
            s.engine.value(),
            s.profile_id,
            relative_age(s.modified),
            truncate_preview(&s.preview),
            w = id_w
        );
    }
    println!(
        "\nresume: agent-switch run <PROFILE> -- --resume <SESSION>  (claude profiles)"
    );
    println!(
        "        agent-switch run <PROFILE> -- resume <SESSION>    (codex profiles)"
    );
    Ok(())
}

/// "just now" / "N m ago" / "N h ago" / "N d ago".
fn relative_age(modified: SystemTime) -> String {
    let secs = SystemTime::now()
        .duration_since(modified)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if secs < 60 {
        "just now".to_string()
    } else if secs < 3600 {
        format!("{} m ago", secs / 60)
    } else if secs < 86400 {
        format!("{} h ago", secs / 3600)
    } else {
        format!("{} d ago", secs / 86400)
    }
}

/// Truncate a preview (char-safe) with an ellipsis.
fn truncate_preview(preview: &str) -> String {
    const CAP: usize = 60;
    let chars: Vec<char> = preview.chars().collect();
    if chars.len() <= CAP {
        preview.to_string()
    } else {
        chars[..CAP].iter().collect::<String>() + "…"
    }
}

/// `run` — the core command: isolated runtime + codex with per-process env.
/// The session id when `extra_args` resumes a conversation
/// (codex `resume <id>` / claude `--resume <id>`); None otherwise.
fn resume_id_from_args(extra_args: &[String]) -> Option<String> {
    extra_args
        .iter()
        .position(|a| a == "resume" || a == "--resume")
        .and_then(|i| extra_args.get(i + 1))
        .cloned()
}

fn cmd_run(
    store: &ProfileStore,
    id: &str,
    workspace: Option<PathBuf>,
    extra_args: &[String],
) -> Result<()> {
    // 1. Find and validate the profile.
    let profile = get_profile(store, id)?;
    let errors = validation::validate_profile(&profile);
    if !errors.is_empty() {
        for e in &errors {
            eprintln!("error: {e}");
        }
        std::process::exit(1);
    }

    // CLI availability for the profile's engine (exit 1, engine-specific
    // wording + the npm install command).
    let engine = profile.engine();
    if launcher::find_engine_binary(engine).is_err() {
        print_cli_missing(engine);
        std::process::exit(1);
    }

    let workspace = match workspace {
        Some(ws) => ws,
        None => std::env::current_dir()?,
    };
    if !workspace.is_dir() {
        bail!("workspace does not exist: {}", workspace.display());
    }

    // 2a. Before a NEW conversation (not a resume) on the codex engine,
    // refresh the profile's model list from the provider — strict sync
    // (stale models removed, never blocks the launch). The refreshed list
    // feeds the generated model catalog, so the TUI's /model switcher
    // offers everything the server still serves.
    let resume_id = resume_id_from_args(extra_args);
    let profile = if resume_id.is_none() && engine == Engine::Codex {
        match models::sync_profile_models(store, &profile) {
            Ok((p, note)) => {
                if let Some(n) = note {
                    println!("note: {n}");
                }
                p
            }
            Err(e) => {
                println!("note: model sync skipped: {e}");
                profile
            }
        }
    } else {
        profile
    };
    if let Some(sid) = &resume_id {
        // Remember the session as open while this process tree runs it;
        // the GUI pool reads the same lock.
        if let Err(e) = sessions::mark_session_open(store, sid) {
            println!("note: {e}");
        }
    }

    // 2b. Create the isolated runtime, resolve the key.
    let settings = Settings::load();
    let runtime = runtime::create_runtime(store, &profile, &workspace)?;
    let key = launcher::resolve_api_key(&profile)?;

    // Banner.
    println!("Profile:   {}", profile.name);
    println!("CLI:       {}", engine.value());
    println!("Base URL:  {}", profile.provider.base_url);
    println!("Model:     {}", profile.model.default);
    println!("Workspace: {}", workspace.display());
    println!("Runtime:   {}", runtime.dir.display());
    if settings.dangerous_flag(engine).is_some() {
        println!("Bypass:    enabled (dangerous mode — no approval prompts)");
    }
    if let Some(proxy) = settings.proxy_url() {
        println!("Proxy:     {proxy}");
    }

    // 3. Spawn the CLI (stdio inherited), wait, exit with its exit code.
    let code = launcher::launch_in_runtime(
        &runtime,
        &profile,
        &settings,
        key,
        &workspace,
        extra_args,
    )?;

    // 4. Opportunistic cleanup, quietly.
    let _ = runtime::cleanup(store);
    std::process::exit(code);
}

/// `login` — official-account login in the profile's isolated home
/// (`codex login` / the claude login screen). No provider vars and no key:
/// the subscription credential lands in the isolated home only.
fn cmd_login(store: &ProfileStore, id: &str, workspace: Option<PathBuf>) -> Result<()> {
    let profile = get_profile(store, id)?;
    let errors = validation::validate_profile(&profile);
    if !errors.is_empty() {
        for e in &errors {
            eprintln!("error: {e}");
        }
        std::process::exit(1);
    }
    if !profile.is_official() {
        bail!(
            "'{}' is not an 'official' profile — login is only for vendor accounts \
             (relay / self-hosted profiles use their API key instead; switch the \
             provider type to official first if you meant to log in to the vendor)",
            profile.id
        );
    }

    let engine = profile.engine();
    if launcher::find_engine_binary(engine).is_err() {
        print_cli_missing(engine);
        std::process::exit(1);
    }

    let workspace = match workspace {
        Some(ws) => ws,
        None => std::env::current_dir()?,
    };
    if !workspace.is_dir() {
        bail!("workspace does not exist: {}", workspace.display());
    }

    let runtime = runtime::create_runtime(store, &profile, &workspace)?;
    println!("Profile:   {}", profile.name);
    println!("CLI:       {}", engine.value());
    println!("Workspace: {}", workspace.display());
    println!("Runtime:   {}", runtime.dir.display());
    println!(
        "{} — finish the browser flow in the terminal that opens below.",
        match engine {
            Engine::Codex => "Opening Codex login",
            Engine::Claude => "Opening Claude (the login screen appears)",
        }
    );

    let code = launcher::login_in_runtime(&runtime, &profile, &workspace)?;
    if code == 0 {
        println!("Login complete — this profile now uses the official account (no key needed).");
    }
    let _ = runtime::cleanup(store);
    std::process::exit(code);
}

/// `doctor` — check codex + config directories.
fn cmd_doctor(store: &ProfileStore) -> Result<()> {
    let mut ok_all = true;
    if codex::find_codex().is_ok() {
        println!("Codex CLI: ✓ Found");
        if let Some(v) = codex::codex_version() {
            println!("Codex Version: {v}");
        }
    } else {
        // Exact spec wording for the not-found case (todo §4.1), plus the
        // npm install command.
        println!("Codex CLI not found.");
        println!();
        println!("Please install Codex CLI first:");
        println!("  npm install -g @openai/codex");
        ok_all = false;
    }
    // Claude is optional: a user with only codex profiles needs no claude,
    // so a missing binary is a warning, not a failure.
    if claude::find_claude().is_ok() {
        println!();
        println!("Claude CLI: ✓ Found");
        if let Some(v) = claude::claude_version() {
            println!("Claude Version: {v}");
        }
    } else {
        println!();
        println!("Claude CLI: ✗ not found (optional; required for claude profiles)");
        println!("  npm install -g @anthropic-ai/claude-code");
    }
    println!();
    let profiles_dir = store.profiles_dir();
    let runtime_dir = store.runtime_dir();
    if profiles_dir.is_dir() {
        println!("Profile Directory: ✓ {}", profiles_dir.display());
    } else {
        println!("Profile Directory: ✗ missing (hint: run agent-switch init)");
        ok_all = false;
    }
    if runtime_dir.is_dir() {
        println!("Runtime Directory: ✓ {}", runtime_dir.display());
    } else {
        println!("Runtime Directory: ✗ missing (hint: run agent-switch init)");
        ok_all = false;
    }
    if ok_all {
        Ok(())
    } else {
        bail!("some checks failed");
    }
}

/// `cleanup` — delete old runtime dirs via core, report how many.
fn cmd_cleanup(store: &ProfileStore) -> Result<()> {
    let deleted = runtime::cleanup(store);
    println!(
        "Deleted {} runtime director{}",
        deleted.len(),
        if deleted.len() == 1 { "y" } else { "ies" }
    );
    Ok(())
}

/// `logs` — tail the log file (same file the GUI's log viewer shows).
fn cmd_logs(lines: usize) -> Result<()> {
    let Some(path) = logging::path() else {
        println!("no log file available (the log directory could not be created)");
        return Ok(());
    };
    let recent = logging::recent_lines(lines);
    if recent.is_empty() {
        println!("log is empty: {}", path.display());
        return Ok(());
    }
    for line in recent {
        println!("{line}");
    }
    Ok(())
}

/// `settings` — print the global launch settings, or open settings.toml
/// in the editor with `--edit`.
fn cmd_settings(edit: bool) -> Result<()> {
    let path = Settings::path();
    if edit {
        let editor = current_editor();
        // Make sure the file exists so the editor does not start blank.
        let s = Settings::load();
        s.save().ok();
        let status = std::process::Command::new(&editor)
            .arg(&path)
            .status()
            .with_context(|| format!("failed to run editor {editor}"))?;
        if !status.success() {
            bail!("editor exited with status {status:?}; settings left unchanged");
        }
        println!("saved");
        return Ok(());
    }
    let s = Settings::load();
    println!("settings file: {}", path.display());
    println!(
        "dangerous_mode: {}",
        if s.dangerous_mode { "true" } else { "false" }
    );
    match s.proxy_url() {
        Some(url) => println!("proxy: {url}"),
        None => println!("proxy: (disabled — direct connection)"),
    }
    match s.terminal.trim() {
        "" => println!("terminal: (auto-detect)"),
        t => println!("terminal: {t}"),
    }
    Ok(())
}

/// New-version notice on every launch (the user opted in: no forced
/// upgrade, but the notice must appear each time a newer version exists).
/// Never blocks the command: the 6 h check cache makes repeated launches
/// cost one tiny file read, and the network is capped at 2.5 s. Set
/// AGENT_SWITCH_NO_UPDATE_CHECK=1 to disable it entirely.
fn maybe_print_update_notice() {
    if std::env::var_os("AGENT_SWITCH_NO_UPDATE_CHECK").is_some() {
        return;
    }
    match update::check(false, std::time::Duration::from_millis(2500)) {
        update::CheckOutcome::Known { release, .. }
            if update::is_newer(&release.tag_name) =>
        {
            eprintln!(
                "\nnote: agent-switch {} is available (you have {}) — run `agent-switch update`",
                release.version(),
                update::current_version(),
            );
        }
        _ => {}
    }
}

/// The manual install command for this platform (shown when the release
/// ships no package matching this machine).
fn manual_install_command() -> &'static str {
    if cfg!(windows) {
        "irm https://raw.githubusercontent.com/AntyRia/agent-switch/main/scripts/install.ps1 | iex"
    } else {
        "curl -fsSL https://raw.githubusercontent.com/AntyRia/agent-switch/main/scripts/install.sh | sh"
    }
}

/// `update` — check the official releases and self-update the CLI binary
/// in place. `--check` only reports.
fn cmd_update(check_only: bool) -> Result<()> {
    println!("Current version: {}", update::current_version());
    match update::check(true, std::time::Duration::from_secs(10)) {
        update::CheckOutcome::Unavailable => {
            bail!("could not reach the release API — check the network and retry");
        }
        update::CheckOutcome::Known { release, cached } => {
            println!(
                "Latest release:  {}{}",
                release.tag_name,
                if cached { " (cached)" } else { "" }
            );
            if !update::is_newer(&release.tag_name) {
                println!("Already up to date.");
                return Ok(());
            }
            if check_only {
                println!("Update available.");
                return Ok(());
            }
            let asset = update::asset_for(&release, update::PackageKind::Cli).ok_or_else(|| {
                anyhow::anyhow!(
                    "no CLI package for this platform in {} — install manually:\n  {}",
                    release.tag_name,
                    manual_install_command()
                )
            })?;
            let dir = std::env::temp_dir().join(format!(
                "agent-switch-update-{}",
                uuid::Uuid::new_v4()
            ));
            std::fs::create_dir_all(&dir)?;
            let zip_path = dir.join(&asset.name);
            let new_bin =
                dir.join(if cfg!(windows) { "agent-switch.exe" } else { "agent-switch" });

            eprintln!("Downloading {} ...", asset.name);
            update::download(
                &asset.browser_download_url,
                &zip_path,
                |done, total| {
                    let pct = done
                        .saturating_mul(100)
                        .checked_div(total.max(1))
                        .unwrap_or(0)
                        .min(100);
                    if total > 0 {
                        eprint!("\r  {done:>12} / {total} bytes ({pct}%)");
                    } else {
                        eprint!("\r  {done:>12} bytes");
                    }
                },
            )?;
            eprintln!();

            match update::verify_against_sums(&release, &asset.name, &zip_path)? {
                Some(digest) => eprintln!("SHA-256 verified: {digest}"),
                None => eprintln!("note: no checksum published for this asset — verification skipped"),
            }

            update::extract_cli_binary(&zip_path, &new_bin)
                .context("extracting the zip failed")?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&new_bin, std::fs::Permissions::from_mode(0o755))?;
            }

            let current =
                std::env::current_exe().context("cannot determine the current binary path")?;
            update::replace_binary(&current, &new_bin)
                .context("replacing the binary failed")?;
            let _ = std::fs::remove_dir_all(&dir);

            println!("Updated {} → {}", current.display(), release.version());
            #[cfg(windows)]
            println!("(the previous binary is cleaned up on the next start)");
            Ok(())
        }
    }
}
