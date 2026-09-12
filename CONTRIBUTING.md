# Contributing to Agent Switch

Thanks for your interest! This guide covers the layout, the build/test
commands, and the invariants every change must preserve.

## Prerequisites

- **Rust (stable)** — toolchain for the core, CLI and the Tauri shell.
- **Node.js 18+ / npm** — only for the GUI.
- Tauri prerequisites for your OS (see
  <https://v2.tauri.app/start/prerequisites/>) for building the GUI.

## Build & test

```bash
# Core + CLI
cargo test                 # unit tests (sandboxed via AGENT_SWITCH_HOME)
cargo build --release

# Unix installer (offline fixtures; includes stdin execution on macOS /bin/sh)
python3 -m unittest discover -s tests -p 'test_install.py'

# GUI (from desktop/)
cd desktop
npm install
npx tsc --noEmit           # type-check the frontend
npm run tauri build        # packaged installer (src-tauri/target/release/bundle/)
```

The whole stack is expected to stay green: `cargo test`, `cargo build --release`
(root workspace), `cargo check` (desktop/src-tauri workspace) and
`npx tsc --noEmit` (desktop).

## Project layout

```
crates/agent-switch-core/   all logic: profiles, runtimes, launcher, sessions
crates/agent-switch-cli/    thin clap binary over the core API
desktop/                    Tauri 2 + React/TypeScript GUI
  src/                      frontend (i18n.tsx is the zh/en string table)
  src-tauri/src/commands.rs backend commands (invoke bridge)
examples/                   ready-to-copy profile TOMLs
```

Rules of thumb:

- **All behavior lives in `agent-switch-core`.** The CLI and the GUI are both
  thin layers over it; never put launch/profile logic in a binary or a command.
- **Add tests in the core crate** next to the code you change. The suite must
  pass with no network access and must never touch the real config root
  (tests set `AGENT_SWITCH_HOME` to a temp dir).

## Security invariants (do not break)

These are the project's core promises; CI-style review should reject any
change that weakens them:

1. The API key is **never written** into generated Codex `config.toml` files or
   start scripts. A literal `provider.api_key` is intentionally stored in the
   profile TOML; use `provider.api_key_env` to keep the secret outside the
   config directory. At launch it is injected as a per-process environment
   variable of the spawned CLI.
2. Agent Switch must not modify the user's global `~/.codex` / `~/.claude`
   configuration or the parent process's environment. Profile homes isolate
   configuration and history; they are not filesystem or network sandboxes.
3. Keys are masked in summaries and cards; the profile editor has an explicit
   reveal control for the person editing that profile.
4. No database, no proxy server, no global "active provider" state — profiles
   are plain TOML files, the runtime homes are plain directories.

## Checklist: adding a profile field

A new TOML field (see `model.effort` for a recent example) touches:

1. `crates/agent-switch-core/src/profile.rs` — struct + `from_toml`/`to_toml`
   + a round-trip test.
2. `crates/agent-switch-core/src/validation.rs` — constraints (if any).
3. `crates/agent-switch-core/src/launcher.rs` — where it affects launch.
4. `desktop/src-tauri/src/commands.rs` — `*In`/`*Out` mapping.
5. `desktop/src/api.ts` — TS type; `desktop/src/Editor.tsx` — form state,
   load, save, UI; `desktop/src/i18n.tsx` — zh/en strings (both required,
   type-checked against the English key set).

## Style

- Rust: `cargo fmt` defaults, comments explain *why* (the codebase is
  comment-dense on purpose — keep new code equally legible).
- TypeScript/React: follow the existing file layout; no i18n framework, plain
  string tables.
- Commits: one logical change each, imperative subject
  (`add model.effort field`, not `added ...`).
