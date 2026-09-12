# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.2] - 2026-09-12

### Fixed

- Parse GitHub release metadata as JSON, so both compact and formatted
  responses work with the macOS/Linux installer.

### Documentation

- Document the Node.js prerequisite for the macOS/Linux install script.

## [0.2.1] - 2026-09-12

### Added

- macOS Apple Silicon GUI (DMG) and CLI (ZIP) packages, with SHA-256 checksums.
- A tag-triggered macOS release workflow that builds and verifies packages
  before publishing them to GitHub Releases.

### Fixed

- Detect the standard macOS Terminal.app and iTerm.app bundles when selecting
  a terminal for GUI launches.
- Restrict Unix profile/runtime directories to mode 0700 and profile files to
  mode 0600.
- Keep API keys out of the terminal command text when launching on macOS.
- Correct the size and spacing of the GUI launch dialog.
- Preserve self-hosted provider categories when editing profiles.
- Use environment-backed API keys for GUI connection tests and model-list requests.
- Recognize Apple Silicon asset names and use POSIX-compatible syntax in the
  Unix installer.
- Fix the Unix installer's syntax error when piped into macOS `/bin/sh`.
- Select the newest stable release containing a package for the current
  platform, so releases for other platforms do not break installation.

### Documentation

- Clarify CLI prerequisites, platform downloads and macOS first-launch steps.
- Explain profile key storage, configuration isolation and model-sync behavior.

## [0.1.0] - 2026-09-10

### Added

- Initial release.
- Profile-driven launcher for Codex CLI (OpenAI protocol) and Claude CLI
  (Anthropic protocol) against relays and local vLLM servers.
- Per-profile isolated persistent runtimes (`CODEX_HOME` / `CLAUDE_CONFIG_DIR`);
  global `~/.codex` and `~/.claude` are never touched.
- Session pool: list and resume conversations per profile (CLI `sessions`,
  GUI Sessions page).
- CLI: `init`, `list`, `show`, `add`, `edit`, `remove`, `test`, `models`,
  `sessions`, `run`, `doctor`, `cleanup`.
- GUI (Tauri + React): bilingual (zh/en) profile list / editor / launch /
  session pool / about pages; connection test and model-list fetch in the editor.
- API keys injected per-process only — never written to runtime configs or
  start scripts; masked everywhere they are displayed.
- Clean-environment launch: ambient nested-session and provider variables are
  stripped from spawned CLIs.
- First-launch onboarding pre-seed for Claude homes (theme, onboarding,
  workspace trust).
- Optional `model.effort` field (Claude) injected as `CLAUDE_CODE_EFFORT_LEVEL`.
- Session open-state: a session whose terminal is still running shows an "Open"
  indicator and cannot be resumed twice (atomic lock file + live process
  command-line scan, 60 s grace, dead locks auto-pruned); deleting a session
  clears its lock.
- Launch-time model auto-sync for Codex profiles: the provider's model list is
  fetched on every launch, merged additively into the profile, and written into
  a multi-entry model catalog — switch models with `/model` in the TUI.
  Failures never block launch.
- Editable per-profile model list (`model.models`, GUI "Model list" field) for
  models a relay's `/v1/models` endpoint omits; "Fetch model list" merges
  additively instead of overwriting.
- GUI: back button in the profile editor.

### Changed

- README: installation restructured into three paths — CLI-only, packaged GUI
  (GitHub Releases), build from source — each with explicit uninstall steps.
- README: intro rewritten around the core pain point (a CLI is hard-bound to
  its single global config, so the same CLI could not use different relay /
  model chains at the same time); usage section gained an "after launch"
  guide (`/model`, history, quitting).

### Fixed

- Codex TUI `/model` picker: relays that serve `codex-auto-*` model ids made
  the picker a two-tier menu ("All models" indirection) instead of one flat
  list; such ids are now excluded from the generated catalog, so every
  profile model appears in a single list.
- Codex TUI `/model` picker: selecting a model did not close the picker
  (the model was actually changed behind the still-open menu, and selecting
  again repeated the change); catalog entries now declare exactly one
  supported reasoning level (`none`), so a selection applies the model and
  dismisses the picker in one step.
- Model auto-sync no longer adds `codex-auto-*` ids to the profile's model
  list.
- Codex launches: an interactive "Set up the Codex agent sandbox" prompt
  appeared on every launch (the generated config left the Windows sandbox
  mode unresolved). The profile config now pins
  `[windows] sandbox = "unelevated"` — the restricted-token sandbox that
  needs no Administrator permissions.
- Codex launches: Codex's startup update check showed a dialog whose
  default option runs `npm install -g @openai/codex` against the user's
  GLOBAL install. Profile runtimes now set
  `check_for_update_on_startup = false`, so launches start straight into
  the TUI.
- Codex launches: the per-directory trust question re-appeared on every
  launch because the runtime `config.toml` is regenerated each start and
  that wipe dropped the `[projects]` trust table Codex writes into it.
  Existing `[projects]` entries are now carried over, and the launch
  workspace is marked trusted automatically (mirroring the existing
  Claude workspace pre-seed).
