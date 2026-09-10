# Agent Switch

Agent Switch is a lightweight local tool that lets you run **Codex CLI** (OpenAI protocol,
GPT-family models) and **Claude CLI** (Anthropic protocol, Claude-family models) against
**any relay (中转站) or local vLLM deployment** — without ever touching your global Codex or
Claude configuration.

Each provider is described by one small TOML *profile* file. Every time you launch, Agent
Switch generates a fresh, isolated runtime — its own `CODEX_HOME` with a generated
`config.toml` for Codex, or its own `CLAUDE_CONFIG_DIR` for Claude — and starts the CLI
bound to that profile's provider, model and API key. You can run as many instances in
parallel as you want, each pointed at a different provider, while your real `~/.codex`
and `~/.claude` stay completely untouched.

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
Launch Codex CLI (OpenAI) or Claude CLI (Anthropic)
        |
        v
Bound to the profile's provider
```

For example, three profiles can run three processes side by side:

```text
relay-gpt.toml        relay-claude.toml        local-vllm.toml
      |                       |                        |
      v                       v                        v
Codex (GPT relay)     Claude (Claude relay)    Codex (local vLLM)
```

## Requirements

- **Rust (stable)** — to build the CLI and core.
- **Node.js 18+ / npm** — only needed for the GUI (Tauri desktop app); the CLI does not need it.
- **Codex CLI** — for profiles with `cli = "codex"` (e.g. `npm install -g @openai/codex`).
- **Claude CLI** — for profiles with `cli = "claude"` (e.g. `npm install -g @anthropic-ai/claude-code`).
  You only need the CLIs you actually use; `agent-switch doctor` reports both.
  Agent Switch never installs or modifies either CLI.

## Build & Install

```bash
# Build everything (CLI + core)
cargo build --release

# Install the agent-switch binary to your cargo bin
cargo install --path crates/agent-switch-cli
```

Verify the install:

```bash
agent-switch --version
agent-switch doctor
```

## Commands

| Command | Description |
| --- | --- |
| `agent-switch init` | Create the config root (`profiles/` + `runtime/`). Idempotent. |
| `agent-switch list` | List all profiles (ID / NAME / CLI / MODEL). |
| `agent-switch show <PROFILE>` | Show one profile; the API key is always masked. |
| `agent-switch add` | Interactively create a new profile. |
| `agent-switch edit <PROFILE>` | Open the profile TOML in `$EDITOR` (fallback: `notepad` on Windows, `vi` elsewhere). |
| `agent-switch remove <PROFILE> [--yes]` | Delete a profile (asks for confirmation; `--yes` for scripts). |
| `agent-switch test <PROFILE>` | Check the provider endpoint (codex: `GET <base_url>/models`; claude: `POST <base_url>/v1/messages` with a 1-token request — an empty model is auto-detected from the model list first). |
| `agent-switch models <PROFILE>` | Fetch the provider's model list (codex: `GET <base_url>/models`; claude: `GET <base_url>/v1/models`), one id per line. |
| `agent-switch sessions` | List resumable sessions from all profile runtimes (session pool). |
| `agent-switch run <PROFILE> [WORKSPACE] [-- cli args]` | Launch the profile's CLI (Codex or Claude) in an isolated runtime (default workspace: current directory). |
| `agent-switch doctor` | Check Codex/Claude CLI availability and the config directories. |
| `agent-switch cleanup` | Delete old runtime directories (keeps the newest 20, drops anything older than 7 days). |

### Examples

```bash
# First-time setup
agent-switch init

# Create a profile (prompts: Profile ID, Name, CLI [codex/claude],
# Provider type [relay/vllm], Base URL, API Key, Model — and Key mode
# [auth_token/api_key] when CLI is claude; empty answer = default)
agent-switch add

# See what you have
agent-switch list
agent-switch show relay-gpt

# Edit a profile in your editor ($EDITOR, fallback notepad/vi)
agent-switch edit relay-gpt

# Test the provider endpoint before running
agent-switch test relay-claude

# List the models the provider offers
agent-switch models relay-claude

# Browse the session pool and resume a conversation
agent-switch sessions
agent-switch run relay-claude -- --resume <SESSION>
agent-switch run relay-gpt -- resume <SESSION>

# Run in an isolated runtime (the CLI follows the profile's `cli` field)
agent-switch run relay-gpt
agent-switch run relay-claude ~/Projects/demo
agent-switch run relay-gpt -- --model gpt-5.6   # extra args go after --

# Delete a profile (asks: Delete profile "relay-gpt"? [y/N])
agent-switch remove relay-gpt

# Environment health check
agent-switch doctor

# Prune old runtimes
agent-switch cleanup
```

If the profile's CLI is not installed, `agent-switch run` prints:

```text
Claude CLI not found.
Please install Claude CLI first.
```

(and the analogous `Codex CLI not found.` / `Please install Codex CLI first.` for codex profiles.)

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

Field notes:

- `id` — `a-z`, `0-9`, `-`, `_`, max 64 chars; must match the file name.
- `cli` — `"codex"` (OpenAI protocol, Codex CLI) or `"claude"` (Anthropic protocol,
  Claude CLI). Absent = `codex` (legacy profiles keep working).
- `provider.type` — `"relay"` (中转站) or `"vllm"` (local deployment). Legacy values
  `openai-compatible` / `openai` are still accepted; the GUI normalizes them to
  relay/vllm on save. The type is informational — it does not change launch behavior.
- `provider.base_url` — for codex: the OpenAI-compatible root (usually ends in `/v1`);
  for claude: the Anthropic server root (usually **without** `/v1`).
- `provider.api_key` — the key, injected per-process at launch. Never written into any
  runtime config or start script.
- `provider.api_key_env` — optional: name of an environment variable holding the key.
  When that variable is set and non-empty it takes precedence over `api_key`.
- `provider.auth_mode` — claude only: `"auth_token"` (Bearer, default) or `"api_key"`
  (x-api-key). Ignored by codex profiles.
- `model.default` — model id (Codex config `model` / Claude `ANTHROPIC_MODEL`).
- `model.effort` — claude only, optional: reasoning effort injected as
  `CLAUDE_CODE_EFFORT_LEVEL`. Set it when the server rejects the CLI's default
  (some vLLM builds accept only `xhigh` / `medium` / `low`).
- `codex.provider_name` — the name of the `[model_providers.<name>]` section in the
  generated Codex config (only used by the codex engine).

Ready-to-copy templates for all four combinations (relay/vLLM × codex/claude)
live in [`examples/`](examples/).

## How isolation works

- Every profile owns a **persistent** isolated home `runtime/<profile-id>/`:
  - **codex profiles**: `runtime/<profile-id>/.codex/` with a `config.toml` that is
    regenerated from the profile on every launch (model, provider, base URL — but
    **no API key**); the profile stays the single source of routing truth.
  - **claude profiles**: `runtime/<profile-id>/.claude/` (Claude gets everything it
    needs from environment variables; the directory collects session transcripts).
- Because the home persists, **conversation history is per-profile and resumable**:
  Claude transcripts and Codex rollouts live inside it, and the session pool
  (`agent-switch sessions` / the GUI's Sessions page) lists them and resumes them in
  the same isolated home. Your global `~/.codex` / `~/.claude` are never touched.
- The CLI is started with `CODEX_HOME` (codex) or `CLAUDE_CONFIG_DIR` (claude) pointing
  at that private directory, so it never reads or writes your global `~/.codex` / `~/.claude`.
- For claude profiles the launch also sets `ANTHROPIC_BASE_URL`, `ANTHROPIC_MODEL` and
  `ANTHROPIC_SMALL_FAST_MODEL` (pinned to the same model, so relays without a haiku do
  not break background requests).
- The API key is passed only as a per-process environment variable of the spawned CLI —
  `OPENAI_API_KEY` for codex, `ANTHROPIC_AUTH_TOKEN` or `ANTHROPIC_API_KEY` for claude
  (per `auth_mode`). It is not exported into your shell and not stored in any config file.
- **The launched CLI gets a clean environment.** Ambient `CLAUDE_CODE_*` / `CLAUDECODE`
  nested-session markers and stray `ANTHROPIC_*` / `OPENAI_*` provider vars from the
  shell (or the app the GUI was started from) are stripped; only the profile's own vars
  are applied. This keeps transcript saving on even when the GUI itself runs inside a
  Claude Code session, and keeps routing fully independent of the parent shell.
- **First-launch onboarding is pre-seeded for claude homes.** A fresh
  `runtime/<profile-id>/.claude/` is seeded with `settings.json` (`theme`, only when
  absent) and `.claude.json` (`hasCompletedOnboarding`, plus a per-workspace
  `hasTrustDialogAccepted` entry, merged without clobbering Claude's own state) so the
  first launch skips the theme picker and the "trust this folder" dialog.
- Legacy ephemeral `runtime/<uuid>` directories left over from older versions are pruned
  automatically: anything older than 7 days is deleted and only the newest 20 are kept
  (`agent-switch cleanup` does the same on demand). Per-profile homes are never pruned.

### Running multiple providers at the same time

```bash
# Terminal A
agent-switch run relay-gpt

# Terminal B
agent-switch run relay-claude

# Terminal C
agent-switch run local-vllm
```

All processes run concurrently — each has its own `runtime/<profile-id>/` home and its own
key environment variable, so they cannot interfere with each other.

## GUI

The desktop GUI (Tauri + TypeScript) lives in `desktop/` and shares the exact same profile
files as the CLI:

```bash
cd desktop
npm install            # if npm is slow, retry: npm install --registry=https://registry.npmmirror.com
npm run tauri dev      # development (hot reload)
npm run tauri build    # packaged installer → src-tauri/target/release/bundle/
```

The GUI is **bilingual (Chinese / English)** — switch with the button in the top-right
corner; your choice is remembered. The editor lets you pick the CLI/protocol
(Codex/OpenAI or Claude/Anthropic), the provider category (relay / vLLM local) and, for
Claude profiles, the key mode (Bearer / x-api-key). Next to the model field there is a
*Fetch model list* (获取模型列表) button that pulls the available model ids from the
base URL and offers them as suggestions — it auto-fills the first model when the field
is empty. The status bar shows whether both CLIs are installed.

The top bar also switches to the **Sessions pool** page (会话池): every resumable
conversation across all profiles, newest first, with chips labeling the CLI (codex/claude)
and the channel (profile name + relay/vLLM), the first user message, the original working
directory and a relative last-active time. One click on *Resume* reopens the session in a
new terminal, inside the same isolated home and the original workspace. In the editor, a
new profile's ID is auto-generated from its display name (editable — touch the field to
take over).

Requires Node 18+ and the Rust stable toolchain (Tauri builds the shell with cargo).

## Testing / sandboxing

Set `AGENT_SWITCH_HOME` to point the CLI and GUI at a different config root:

```bash
export AGENT_SWITCH_HOME=/tmp/agent-switch-test   # Unix
# set AGENT_SWITCH_HOME=C:\temp\agent-switch-test # Windows
agent-switch init
agent-switch doctor
```

This is how the test suite sandboxes itself; nothing outside the chosen root is ever touched.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for the project layout, build/test
commands, and the security invariants every change must preserve.

## License

[MIT](LICENSE) — see [LICENSE](LICENSE).
