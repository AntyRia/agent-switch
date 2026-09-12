# Agent Switch

A profile-driven, multi-provider launcher for **Codex CLI** (OpenAI protocol)
and **Claude CLI** (Anthropic protocol). Each provider is one small TOML
*profile*; every launch uses a persistent configuration and session directory
for that profile, without changing your global CLI configuration.

<p align="center">
  <b>English</b> | <a href="README.zh.md">简体中文</a>
</p>

- [Overview](#overview)
- [Features](#features)
- [Requirements](#requirements)
- [Quick start](#quick-start)
- [Installation](#installation)
- [Usage](#usage)
- [GUI tour](#gui-tour)
- [Profile format](#profile-format)
- [How isolation works](#how-isolation-works)
- [Troubleshooting](#troubleshooting)
- [Development](#development)
- [Contributing](#contributing)
- [Donations](#donations)
- [Acknowledgments](#acknowledgments)
- [License](#license)

## Overview

### The problem

Codex CLI and Claude CLI use a default configuration directory
(`~/.codex`, `~/.claude`). Managing several providers through that default
configuration can be cumbersome:

- **Switching providers requires managing settings.** Endpoint, credentials
  and model must stay consistent without disturbing existing login state.
- **Parallel providers need separate configuration.** Running a relay in one
  terminal and a local server in another requires managing environment
  variables or separate CLI homes for each launch.

### The idea

Agent Switch moves the "provider" out of the global config and into a small
per-provider file:

```text
Profile (profiles/<id>.toml)
        |
        v
Isolated persistent home (runtime/<profile-id>/.codex or .claude)
        |
        v
Independent CODEX_HOME / CLAUDE_CONFIG_DIR
        |
        v
Launch Codex CLI (OpenAI) or Claude CLI (Anthropic),
bound to the profile's provider / model / key
```

Each profile owns a **persistent** isolated home, so:

- **One provider = one profile file.** Adding a provider never touches anything
  global.
- **Parallel providers.** The same CLI runs against any number of relays,
  local servers or official endpoints at the same time, without interference.
- **Zero-cost switching.** Switching providers means launching a different
  profile; inside a session, `/model` switches models within the profile.
- **Key-safe launch.** The API key is injected as a per-process environment
  variable and is never copied into generated runtime config files or start
  scripts. A literal `provider.api_key` is stored in the profile TOML; use
  `provider.api_key_env` when the secret must remain outside the config
  directory.

## Features

- **One profile per provider** — a single TOML file defines the endpoint, API
  key and default model (relay, self-hosted vLLM, or the official vendor
  endpoint).
- **Both protocols, both CLIs** — `codex` (OpenAI protocol, GPT-family models)
  and `claude` (Anthropic protocol, Claude-family models); the profile decides
  which CLI is launched.
- **Separate configuration and history** — each profile owns a persistent
  runtime (`CODEX_HOME` / `CLAUDE_CONFIG_DIR`). Agent Switch does not edit
  your global CLI configuration; it does not sandbox the CLI's file or network access.
- **Official vendor support** — point a profile at OpenAI / Anthropic
  directly: use a Platform API key, or run a subscription login
  (`agent-switch login`) once and let the isolated home keep the account.
- **Session pool** — conversation history is stored per profile and resumable
  from the CLI or the GUI; a session whose terminal is still open is flagged
  **Open** and cannot be resumed twice. Pinned sessions are protected, and the
  GUI can clear all non-pinned sessions in one click.
- **Model auto-sync** — a successful model-list fetch replaces the provider's
  catalog (stale entries are removed and the configured default is kept). If
  the fetch fails, the existing catalog is retained and the launch reports the
  sync failure; an old model may still be unavailable upstream.
- **Key-safe launch** — the API key is injected as a per-process environment
  variable and is never copied into generated runtime config files or start
  scripts. If `provider.api_key` is used, the profile TOML necessarily stores
  that value; use `provider.api_key_env` when the key must remain outside the
  Agent Switch config directory. Summaries mask keys; the editor has an explicit reveal control.
- **CLI + GUI** — a single-file command-line tool and a bilingual
  (Chinese / English) Tauri desktop app that share the same profile files.

## Requirements

Agent Switch drives the official CLI tools; it does **not** bundle them.
Install whichever matches the profiles you want to use (once, globally):

| Profile CLI | Install command |
| --- | --- |
| Codex CLI (OpenAI protocol) | `npm install -g @openai/codex` |
| Claude CLI (Anthropic protocol) | `npm install -g @anthropic-ai/claude-code` |

If a required CLI is missing, both the command line and the GUI tell you
exactly which one to install — and the GUI disables launching until you do.
`agent-switch doctor` checks everything at any time.

## Quick start

A three-step path from install to first conversation.

**Step 1 — Install** (choose the form that fits you):

- Command line only → [install the CLI](#1-cli-only-no-gui)
- Desktop app → [install the GUI](#2-gui-desktop-app)

**Step 2 — Check the environment**:

In the GUI, open **About** and check the CLI environment. For the command line:

```bash
agent-switch doctor      # Codex / Claude CLI + config directories
```

**Step 3 — Create a profile and launch**:

In the GUI, add a profile, fill in its provider settings, save it, then click
**Launch** and select a workspace. For the command line:

```bash
agent-switch add         # interactive: CLI / provider / endpoint / key / model
agent-switch test <id>   # check the endpoint (optional)
agent-switch run <id>    # launch — you land in the CLI's TUI
agent-switch sessions    # list / resume past conversations
```

> Running `agent-switch` with no arguments prints this quick-start guide
> plus a command reference. `agent-switch <command> --help` covers one
> command in detail.

## Installation

Both forms share one config root (Windows: `%APPDATA%\agent-switch`;
macOS / Linux: `~/.agent-switch`) — to remove Agent Switch profiles and history, delete the program
and that folder. The underlying CLIs and OS application caches are separate.

### 1. CLI only (no GUI)

#### Option A — One-line install (recommended)

The macOS/Linux installer requires [Node.js](https://nodejs.org) to parse
GitHub release metadata. You can also download the CLI ZIP manually.

Downloads the newest stable release with a package for your platform from
[GitHub Releases](https://github.com/AntyRia/agent-switch/releases) and installs
it. Windows updates both the user PATH and the current PowerShell session.
On macOS/Linux it installs to `~/.local/bin`. If needed, it adds a PATH entry
to existing `.bashrc`, `.zshrc` and `.profile` files and prints the export
command for the current terminal; a piped script cannot change its parent shell.

**Windows** (PowerShell):

```powershell
irm https://raw.githubusercontent.com/AntyRia/agent-switch/main/scripts/install.ps1 | iex
```

**macOS / Linux**:

```bash
curl -fsSL https://raw.githubusercontent.com/AntyRia/agent-switch/main/scripts/install.sh | sh
```

> Releases can contain different platforms. The installer selects the newest
> stable release with a matching package; drafts and prereleases are skipped.
> If no release includes your platform, build from source.

Release asset naming: `agent-switch-<version>-<target>.zip` (standard Rust
target names). Check the release's asset list before downloading.

| Platform | CLI package |
| --- | --- |
| Windows x64 | `agent-switch-0.2.2-x86_64-pc-windows-msvc.zip` ([v0.2.2](https://github.com/AntyRia/agent-switch/releases/tag/v0.2.2)) |
| macOS (Apple Silicon) | `agent-switch-0.2.2-aarch64-apple-darwin.zip` |
| macOS (Intel) / Linux | Build from source |

Prefer to do it by hand? Download the matching zip, unzip it, and put
`agent-switch(.exe)` anywhere on your `PATH`.

**Uninstall**:

| How you installed | How to remove |
| --- | --- |
| One-line (Windows) | Delete `%LOCALAPPDATA%\agent-switch` and remove its `bin` from your user PATH |
| One-line (macOS / Linux) | Delete `~/.local/bin/agent-switch` |
| Manual / cargo | Delete the executable, or `cargo uninstall agent-switch` |

To also remove profiles and session history, delete the config root.

#### Option B — Build from source

Prerequisites: [Rust stable](https://rustup.rs), plus the underlying CLI(s)
(see [Requirements](#requirements)).

```bash
git clone https://github.com/AntyRia/agent-switch.git && cd agent-switch
cargo build --release                    # → target/release/agent-switch(.exe)
cargo install --path crates/agent-switch-cli   # optional: onto your PATH
```

### 2. GUI desktop app

The GUI includes profile editing, launch, session
management, settings, logs, connection tests and model lists. The `agent-switch`
CLI binary is not required. You still need the underlying Codex / Claude
CLI(s) from npm (see [Requirements](#requirements)); the GUI only reports them
as available after a real executable check.

#### Option A — Download the installer (recommended)

Download the installer for your platform from
[GitHub Releases](https://github.com/AntyRia/agent-switch/releases) and run
it.

| Platform | GUI package |
| --- | --- |
| Windows x64 | Use the [v0.2.2 Windows installers](https://github.com/AntyRia/agent-switch/releases/tag/v0.2.2) |
| macOS (Apple Silicon) | `Agent.Switch_0.2.2_aarch64.dmg` (ad-hoc signed, not notarized) |
| macOS (Intel) | Not built by this release |
| Linux | Not built by this release; build from source |

**macOS installation** — open the DMG and drag **Agent Switch** to
**Applications**. The package is for Apple Silicon and does not include Codex
or Claude; install the required CLI first. Terminal and iTerm are detected
automatically.

The app is ad-hoc signed and has not been notarized by Apple. If macOS blocks
it, verify the download's checksum against `SHA256SUMS.txt` from the same
release, attempt to open the app, then use **System Settings → Privacy &
Security → Open Anyway** if you trust the download.

**Uninstall** — the normal way for your OS (Add or Remove Programs on Windows,
drag out of Applications on macOS, `apt` / `rpm` on Linux). Profiles remain in
the config root; delete that folder to remove Agent Switch profiles and session history.

#### Option B — Build from source

Prerequisites: [Rust stable](https://rustup.rs), **Node.js 18+** (Tauri
frontend), the underlying CLI(s), macOS Command Line Tools (`xcode-select --install`),
and — on Linux — the
[Tauri system dependencies](https://tauri.app/start/prerequisites/).

```bash
git clone https://github.com/AntyRia/agent-switch.git && cd agent-switch
cd desktop
npm install
npm run tauri build            # installers → src-tauri/target/release/bundle/
```

## Usage

### Commands

| Command | Description |
| --- | --- |
| `agent-switch` | No arguments: prints a quick-start guide and command reference. |
| `agent-switch init` | Create the config root (`profiles/` + `runtime/`). Idempotent. |
| `agent-switch list` | List all profiles (ID / NAME / CLI / MODEL). |
| `agent-switch show <PROFILE>` | Show one profile; the API key is always masked. |
| `agent-switch add` | Interactively create a new profile (empty answer = the bracketed default; pipe-friendly). |
| `agent-switch edit <PROFILE>` | Open the profile TOML in `$EDITOR` (fallback: `notepad` on Windows, `vi` elsewhere). |
| `agent-switch remove <PROFILE> [--yes]` | Delete a profile (asks for confirmation; `--yes` for scripts). |
| `agent-switch test <PROFILE>` | Check the provider endpoint (codex: `GET <base_url>/models`; claude: `POST <base_url>/v1/messages` with a 1-token request). |
| `agent-switch models <PROFILE>` | Fetch the provider's model list, one id per line. |
| `agent-switch sessions` | List resumable sessions from all profile runtimes (session pool). |
| `agent-switch run <PROFILE> [WORKSPACE] [-- cli args]` | Launch the profile's CLI (Codex or Claude) in an isolated runtime (default workspace: current directory). |
| `agent-switch login <PROFILE> [WORKSPACE]` | Log in to the official account of an `official` profile: runs `codex login` / the Claude login screen in the profile's isolated home. The subscription credential stays in that home — no API key needed afterwards. |
| `agent-switch doctor` | Check Codex / Claude CLI availability and the config directories. |
| `agent-switch cleanup` | Delete old runtime directories (keeps the newest 20, drops anything older than 7 days). |
| `agent-switch logs [N]` | Show the last N lines of the app log (default 50). |
| `agent-switch settings [--edit]` | Show or edit the global launch settings (dangerous mode, proxy, terminal). |

### Examples

```bash
# Profile management
agent-switch show my-relay-gpt
agent-switch edit my-relay-gpt
agent-switch remove my-relay-gpt

# Extra arguments to the CLI go after --
agent-switch run my-relay-gpt -- --model gpt-5.6-sol

# Resume a session (codex: resume, claude: --resume)
agent-switch run my-relay-gpt -- resume <SESSION_ID>
agent-switch run my-relay-claude -- --resume <SESSION_ID>

# Environment health check / prune old runtimes / view the log
agent-switch doctor
agent-switch cleanup
agent-switch logs 80
```

If a session's terminal is still open, resuming reports `session already
open` — close that terminal first, then resume.

### After launch (inside the TUI)

`agent-switch run` drops you straight into the CLI's TUI (Codex or Claude),
with the workspace set to the directory you ran it from:

- **`/model`** — switch models within the profile: it lists every model of
  the profile (default model + the profile's *Model list* + models synced
  from the server); selecting one applies it and closes the picker
  immediately.
- **Conversation history** — stored in the profile's own isolated runtime and
  survives exiting the TUI. Find it later with `agent-switch sessions` and
  continue with `agent-switch run <PROFILE> -- resume <SESSION_ID>`.
- **Quitting** — type `/exit` (or press `Ctrl+C`) to return to your own
  terminal. The profile and all sessions are kept as-is, ready to `run` or
  resume again.

If the profile's CLI is not installed, `agent-switch run` prints:

```text
Claude CLI not found.
Please install Claude CLI first:
  npm install -g @anthropic-ai/claude-code
```

(with the analogous `Codex CLI not found.` /
`npm install -g @openai/codex` message for codex profiles.)

### Running multiple providers at the same time

```bash
# Terminal A
agent-switch run relay-gpt

# Terminal B
agent-switch run relay-claude

# Terminal C
agent-switch run local-vllm
```

All processes run concurrently — each has its own `runtime/<profile-id>/`
home and its own key environment variable, so they cannot interfere with each
other.

## GUI tour

The desktop app (Tauri + TypeScript) shares the exact same profile files as
the CLI and is **bilingual (Chinese / English)** — switch with the button in
the top-right corner; your choice is remembered.

- **Profile list** — one card per profile: CLI chip (Codex / Claude),
  provider category, default model, key state, and Launch / Login
  (official) / Edit / Delete actions. Launch opens a small dialog to pick the
  workspace folder, then opens the CLI in your system terminal.
- **Profile editor** — pick the CLI / protocol (Codex / OpenAI or Claude /
  Anthropic), the provider category (relay / self-hosted OpenAI-compatible /
  official) and, for Claude profiles, the key mode (Bearer / x-api-key). For
  official profiles the vendor endpoint is fixed and the API key is optional.
  *Fetch model list* next to the model field refreshes the *Model list* field
  after a successful server response (stale entries are removed and the default
  model is kept); a failed fetch leaves the previous list in place. The list is written
  into the model catalog on launch and switchable with `/model` in the TUI.
  A new profile's ID is an auto-generated UUID, kept internal — never shown
  or edited.
- **Sessions pool** — every resumable conversation across all profiles,
  newest first, with chips labeling the CLI (codex / claude) and the channel
  (profile name + provider category), the first user message, the original
  working directory and a relative last-active time. **A session whose
  terminal is still open shows a green "Open" indicator and its Resume button
  is disabled** — close the terminal, then resume. Search, pin, rename,
  delete, one-click Resume, and **Clear unpinned** (removes every session
  that is not pinned or open) are all available.
- **Settings** — dangerous mode (bypass all approvals and the sandbox on
  every launch), an HTTP proxy that all launched CLIs route through
  (localhost / 127.0.0.1 always bypassed), the terminal to use
  (auto-detect or a custom path), and a read-only tail of the app log for
  troubleshooting.
- **About** — the CLI health check (install instructions when one is
  missing, with a re-check button that unlocks launching after an install)
  and the config directory location.
- The status bar shows whether both CLIs are installed.
- When a profile's CLI is missing, launching (and resuming its sessions) is
  disabled with the exact `npm install` command shown — no cryptic errors.

## Profile format

Profiles live in `profiles/<id>.toml` inside the config root
(`~/.agent-switch` on Unix, `%APPDATA%\agent-switch` on Windows).

GPT relay — Codex CLI, OpenAI protocol:

```toml
id = "relay-gpt"
name = "GPT Relay A"
description = "Primary cloud relay"
cli = "codex"

[provider]
type = "relay"
base_url = "https://example.com/v1"
api_key = "sk-xxxxxxxx"

[model]
default = "gpt-5.6"

[codex]
provider_name = "relay-gpt"
```

Claude relay — Claude CLI, Anthropic protocol:

```toml
id = "relay-claude"
name = "Claude Relay A"
description = "Primary Claude relay"
cli = "claude"

[provider]
type = "relay"
# Anthropic convention: the server root, without a trailing /v1
base_url = "https://relay.example.com"
api_key = "sk-xxxxxxxx"
# How the key is sent: "auth_token" (Authorization: Bearer, default)
# or "api_key" (x-api-key header, official API convention)
auth_mode = "auth_token"

[model]
default = "claude-sonnet-4-5"

[codex]
provider_name = "relay-claude"
```

Local vLLM server — Codex CLI (vLLM speaks the OpenAI protocol):

```toml
id = "local-vllm"
name = "Local Qwen vLLM"
description = "Local vLLM server"
cli = "codex"

[provider]
type = "vllm"
base_url = "http://127.0.0.1:8000/v1"
# no api_key: local servers usually need none

[model]
default = "Qwen/Qwen3.8-27B"

[codex]
provider_name = "local-vllm"
```

Official vendor — no relay at all (subscription login or a vendor API key):

```toml
id = "official-gpt"
name = "OpenAI Official"
cli = "codex"

[provider]
type = "official"
# Displayed only; launches use the built-in vendor endpoint and never send
# a base URL override. Leave at the vendor default.
base_url = "https://api.openai.com/v1"
# api_key is OPTIONAL: either a Platform API key (injected as
# OPENAI_API_KEY / ANTHROPIC_API_KEY), or no key at all — then log in once
# with `agent-switch login official-gpt` and the subscription account in
# this profile's isolated home is used.

[model]
default = "gpt-5.6"

[codex]
provider_name = "official-gpt"
```

For claude: `type = "official"`, `base_url = "https://api.anthropic.com"`,
and the key (when set) is always sent with the vendor's `x-api-key` header.

Field notes:

- `id` — `a-z`, `0-9`, `-`, `_`, max 64 chars; must match the file name.
- `cli` — `"codex"` (OpenAI protocol, Codex CLI) or `"claude"` (Anthropic
  protocol, Claude CLI). Absent = `codex` (legacy profiles keep working).
- `provider.type` — `"relay"` (third-party relay), `"vllm"` (local
  deployment) or `"official"` (vendor direct). `relay` / `vllm` are
  informational only; `official` changes launch behavior: no base URL
  override, key optional (a keyless launch uses the subscription login stored
  in the profile's isolated home, and codex pins
  `cli_auth_credentials_store = "file"` so the login never leaks into the OS
  keyring). Legacy values `openai-compatible` / `openai` are still accepted.
- `provider.base_url` — for codex: the OpenAI-compatible root (usually ends
  in `/v1`); for claude: the Anthropic server root (usually **without**
  `/v1`).
- `provider.api_key` — the key, injected per-process at launch. It is never
  copied into generated runtime config or start scripts; the literal profile
  field remains on disk. Use `provider.api_key_env` for external secret
  storage.
- `provider.api_key_env` — optional: name of an environment variable holding
  the key. When that variable is set and non-empty it takes precedence over
  `api_key`.
- `provider.auth_mode` — claude only: `"auth_token"` (Bearer, default) or
  `"api_key"` (x-api-key). Ignored by codex profiles.
- `model.default` — model id (Codex config `model` / Claude
  `ANTHROPIC_MODEL`).
- `model.effort` — claude only, optional: reasoning effort injected as
  `CLAUDE_CODE_EFFORT_LEVEL`. Set it when the server rejects the CLI's
  default (some vLLM builds accept only `xhigh` / `medium` / `low`).
- `model.models` — model list refreshed after a successful provider fetch. Stale
  entries are removed on success; when the provider cannot be reached, the last
  catalog remains so an offline launch can still be attempted.
- `codex.provider_name` — the name of the `[model_providers.<name>]` section
  in the generated Codex config (only used by the codex engine).

Ready-to-copy templates for every combination (relay / vLLM / official ×
codex / claude) live in [`examples/`](examples/).

## How isolation works

- Every profile owns a **persistent** isolated home `runtime/<profile-id>/`:
  - **codex profiles**: `runtime/<profile-id>/.codex/` with a `config.toml`
    that is regenerated from the profile on every launch (model, provider,
    base URL — but **no API key**); the profile stays the single source of
    routing truth.
  - **claude profiles**: `runtime/<profile-id>/.claude/` (Claude gets
    everything it needs from environment variables; the directory collects
    session transcripts).
- Because the home persists, **conversation history is per-profile and
  resumable**: Claude transcripts and Codex rollouts live inside it, and the
  session pool (`agent-switch sessions` / the GUI's Sessions page) lists them
  and resumes them in the same isolated home.
- The CLI is started with `CODEX_HOME` (codex) or `CLAUDE_CONFIG_DIR`
  (claude) pointing at that profile's directory. Agent Switch does not edit
  the global CLI configuration. Workspace files, plugins and the underlying
  CLI's own file and network access remain subject to that CLI's settings.
- For claude profiles the launch also sets `ANTHROPIC_BASE_URL`,
  `ANTHROPIC_MODEL` and `ANTHROPIC_SMALL_FAST_MODEL` (pinned to the same
  model, so relays without a haiku do not break background requests).
- The API key is passed only as a per-process environment variable of the
  spawned CLI — `OPENAI_API_KEY` for codex, `ANTHROPIC_AUTH_TOKEN` or
  `ANTHROPIC_API_KEY` for claude (per `auth_mode`). It is not exported into
  your shell or written to generated runtime config/start scripts. A literal
  `provider.api_key` remains in the profile TOML by design; use
  `provider.api_key_env` to keep the secret outside the config directory.
- **The launched CLI gets a clean environment.** Ambient `CLAUDE_CODE_*` /
  `CLAUDECODE` nested-session markers and stray `ANTHROPIC_*` / `OPENAI_*`
  provider vars from the shell (or the app the GUI was started from) are
  stripped; only the profile's own vars are applied. This keeps transcript
  saving on even when the GUI itself runs inside a Claude Code session, and
  keeps routing fully independent of the parent shell.
- **First-launch onboarding is pre-seeded for claude homes.** A fresh
  `runtime/<profile-id>/.claude/` is seeded with `settings.json` (`theme`,
  only when absent) and `.claude.json` (`hasCompletedOnboarding`, plus a
  per-workspace `hasTrustDialogAccepted` entry, merged without clobbering
  Claude's own state) so the first launch skips the theme picker and the
  "trust this folder" dialog.
- Legacy ephemeral `runtime/<uuid>` directories left over from older versions
  are pruned automatically: anything older than 7 days is deleted and only
  the newest 20 are kept (`agent-switch cleanup` does the same on demand).
  Per-profile homes are never pruned.

## Troubleshooting

- **`Codex CLI not found.` / `Claude CLI not found.`** — the underlying CLI
  for that profile is not installed. Install it with the printed `npm
  install -g …` command, then retry (the GUI: press *Re-check* on the About
  page or in the launch dialog).
- **`session already open`** — the session's terminal is still running.
  Close that terminal window, then resume.
- **Connection test fails** — check `provider.base_url` (codex usually ends
  in `/v1`, claude usually does not), the API key, and whether the model id
  exists on that server (`agent-switch models <id>`).
- **Models are listed, but conversations fail** — a successful `/models`
  request does not prove that inference works. Codex profiles require the
  Responses API; Claude profiles require Anthropic Messages. A server that
  only implements Chat Completions is insufficient. Check the selected
  model's availability and the provider's error response.
- **Something else** — `agent-switch logs` shows the last lines of the app
  log (the GUI's Settings page has the same tail), and
  `agent-switch doctor` re-checks the environment.

## Development

- **Separate development data** — set `AGENT_SWITCH_HOME` to point the CLI and GUI at a
  different config root. This isolates Agent Switch data, not filesystem or network
  access of the underlying CLIs:

  ```bash
  export AGENT_SWITCH_HOME=/tmp/agent-switch-test   # Unix
  # set AGENT_SWITCH_HOME=C:\temp\agent-switch-test # Windows
  ```

- **Run the GUI with hot reload**:

  ```bash
  cd desktop
  npm install
  npm run tauri dev
  ```

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for the project layout, build / test
commands, and the security invariants every change must preserve.

## Donations

If Agent Switch saves you time, you are welcome to buy the maintainer a
coffee. Donations go toward ongoing maintenance and the model API costs of
development and testing.

| Amount | Link |
| --- | --- |
| HKD 20 | [Donate via Stripe](https://buy.stripe.com/4gM7sLepN8bmdCq1eQ08g01) |
| HKD 50 | [Donate via Stripe](https://buy.stripe.com/bJe5kD81pajucym9Lm08g02) |
| HKD 100 | [Donate via Stripe](https://buy.stripe.com/9B67sL6XlfDObui9Lm08g03) |

Thank you!

## Acknowledgments

- **HyperRoute · 超路由** — [hyperroute.cc](https://hyperroute.cc) — model
  relay access (GPT / Claude) that works out of the box; thank you for the
  support.
- **linux.do** — [linux.do](https://linux.do/) — the open technical community
  where much of this project's discussion and feedback happens; thank you.

## License

[MIT](LICENSE) — see the [LICENSE](LICENSE) file for the full text.

The software is provided "as is", without warranty of any kind. You are
responsible for the providers, keys and content you configure with it.
