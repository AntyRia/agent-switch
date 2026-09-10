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
    claude, codex, engine::Engine, health, launcher, models,
    profile::{Profile, ProviderConfig, ModelConfig, CodexConfig},
    profile_store::ProfileStore,
    runtime, sessions, validation,
};

#[derive(Parser)]
#[command(
    name = "agent-switch",
    version,
    about = "Profile-driven Codex / Claude multi-provider launcher"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create the config root (profiles/ + runtime/); idempotent
    Init,
    /// List all profiles
    List,
    /// Show one profile (API key is masked)
    Show {
        /// Profile id
        profile: String,
    },
    /// Add a new profile interactively
    Add,
    /// Edit a profile with $EDITOR (fallback: notepad / vi)
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
    /// Check that the Codex/Claude CLIs and the config directories are in place
    Doctor,
    /// Delete old runtime directories (keeps the newest 20)
    Cleanup,
}

fn main() {
    let cli = Cli::parse();
    let store = ProfileStore::new();
    if let Err(err) = dispatch(cli, &store) {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}

fn dispatch(cli: Cli, store: &ProfileStore) -> Result<()> {
    match cli.command {
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
        Command::Doctor => cmd_doctor(store),
        Command::Cleanup => cmd_cleanup(store),
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
    let p = store.get(id)?;
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
/// and stays empty.
fn cmd_add(store: &ProfileStore) -> Result<()> {
    let stdin = io::stdin();
    let mut lines = stdin.lock().lines();
    let id = ask_line(&mut lines, "Profile ID:")?;
    let name = ask_line(&mut lines, "Name:")?;
    let cli_raw = ask_line(&mut lines, "CLI [codex/claude] (default codex):")?;
    let cli = match cli_raw.as_str() {
        "" => "codex".to_string(),
        c => c.trim().to_ascii_lowercase(),
    };
    let type_raw = ask_line(&mut lines, "Provider type [relay/vllm] (default relay):")?;
    let provider_type = match type_raw.as_str() {
        "" => "relay".to_string(),
        t => t.trim().to_ascii_lowercase(),
    };
    let base_url = ask_line(&mut lines, "Base URL:")?;
    let api_key = ask_line(&mut lines, "API Key:")?;
    let model = ask_line(&mut lines, "Model:")?;
    // Key transport is only meaningful for the claude engine.
    let auth_mode = if cli == "claude" {
        let raw = ask_line(&mut lines, "Key mode [auth_token/api_key] (default auth_token):")?;
        match raw.as_str() {
            "" => None,
            m => Some(m.trim().to_ascii_lowercase()),
        }
    } else {
        None
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
        },
        codex: CodexConfig {
            provider_name: id.clone(),
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
    if store.path_for(&id).exists() {
        bail!("profile already exists: {id}");
    }
    store.init().ok();
    let path = store.save(&profile, None)?;
    println!("Created {}", path.display());
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

/// `edit` — open the profile TOML in the user's editor, wait, re-validate.
fn cmd_edit(store: &ProfileStore, id: &str) -> Result<()> {
    // Fail early with a clear error if the profile does not exist.
    store.get(id)?;
    let path = store.path_for(id);
    let editor = std::env::var("EDITOR")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| {
            if cfg!(windows) {
                "notepad".to_string()
            } else {
                "vi".to_string()
            }
        });
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
    let errors = validation::validate_profile(&p);
    if !errors.is_empty() {
        for e in &errors {
            eprintln!("error: {e}");
        }
        std::process::exit(1);
    }
    if p.id != id {
        // The editor changed the id inside the file: rename the file to
        // follow it, unless the new id is already taken by another profile.
        if store.path_for(&p.id).exists() {
            bail!(
                "cannot rename to id \"{}\": a profile with that id already exists",
                p.id
            );
        }
        store.save(&p, Some(id))?;
    }
    println!("saved");
    Ok(())
}

/// `remove` — confirm with y/yes (case-insensitive), otherwise "aborted".
/// `--yes` skips the prompt for scripts.
fn cmd_remove(store: &ProfileStore, id: &str, yes: bool) -> Result<()> {
    store.get(id)?;
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
    let p = store.get(id)?;
    let key = launcher::resolve_api_key(&p)?;
    let result = health::test_engine(
        p.engine(),
        &p.provider.base_url,
        &key,
        &p.model.default,
        p.provider.auth_mode.as_deref(),
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
    let p = store.get(id)?;
    let key = launcher::resolve_api_key(&p)?;
    let list = models::fetch_models(
        p.engine(),
        &p.provider.base_url,
        &key,
        p.provider.auth_mode.as_deref(),
    )?;
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
fn cmd_run(
    store: &ProfileStore,
    id: &str,
    workspace: Option<PathBuf>,
    extra_args: &[String],
) -> Result<()> {
    // 1. Find and validate the profile.
    let profile = match store.get(id) {
        Ok(p) => p,
        Err(agent_switch_core::error::Error::ProfileNotFound(_)) => {
            bail!("profile not found: {id} (run 'agent-switch list' to see available profiles)");
        }
        Err(e) => return Err(e.into()),
    };
    let errors = validation::validate_profile(&profile);
    if !errors.is_empty() {
        for e in &errors {
            eprintln!("error: {e}");
        }
        std::process::exit(1);
    }

    // CLI availability for the profile's engine (exit 1, engine-specific
    // wording).
    let engine = profile.engine();
    if launcher::find_engine_binary(engine).is_err() {
        match engine {
            Engine::Codex => {
                println!("Codex CLI not found.");
                println!("Please install Codex CLI first.");
            }
            Engine::Claude => {
                println!("Claude CLI not found.");
                println!("Please install Claude CLI first.");
            }
        }
        std::process::exit(1);
    }

    let workspace = match workspace {
        Some(ws) => ws,
        None => std::env::current_dir()?,
    };
    if !workspace.is_dir() {
        bail!("workspace does not exist: {}", workspace.display());
    }

    // 2. Create the isolated runtime, resolve the key.
    let runtime = runtime::create_runtime(store, &profile)?;
    let key = launcher::resolve_api_key(&profile)?;

    // Banner.
    println!("Profile:   {}", profile.name);
    println!("CLI:       {}", engine.value());
    println!("Base URL:  {}", profile.provider.base_url);
    println!("Model:     {}", profile.model.default);
    println!("Workspace: {}", workspace.display());
    println!("Runtime:   {}", runtime.dir.display());

    // 3. Spawn the CLI (stdio inherited), wait, exit with its exit code.
    let code = launcher::launch_in_runtime(&runtime, &profile, key, &workspace, extra_args)?;

    // 4. Opportunistic cleanup, quietly.
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
        // Exact spec wording for the not-found case (todo §4.1).
        println!("Codex CLI not found.");
        println!();
        println!("Please install Codex CLI first.");
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
