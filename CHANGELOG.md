# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.4.2] - 2026-10-22

### Added

- The session table now falls back to the profile-wide server cache hit rate
  (badge `SV`) when a request has no client cache fields, instead of showing
  a misleading `0.0%`. The value comes from vLLM `/metrics` and is labelled
  as profile-wide.
- The banner explains the vLLM `--enable-prompt-tokens-details` flag needed
  for per-request `cache_read_input_tokens`; with it, new turns show their
  own exact cache hit rate and do not use the SV fallback.

### Notes

- Requests made before the flag was enabled keep the SV aggregate; historical
  per-request cache counts cannot be reconstructed.

## [0.4.1] - 2026-10-22

### Fixed

- Windows auto-update failed with `Windows 找不到 '\\' 文件`: the detached
  `cmd /C "... start "" "..." "` line was re-quoted by cmd, so the installer
  path collapsed to a bare backslash. The updater now writes a tiny `.cmd`
  next to the installer and launches it through `%~dp0`, which avoids the
  nested quoting entirely. The downloaded installer is now actually started,
  so the old process is replaced instead of leaving the user on the previous
  version with the update dialog still appearing.
- If an old Agent Switch window is still open after an update, fully quit it
  before launching the newly installed build; the 0.4.1 check treats the
  matching release as up to date.

## [0.4.0] - 2026-10-22

### Added

- **CSV export** on the Usage page: one row per assistant request (Claude)
  or `token_count` event (Codex) across every session, with timestamp,
  session/profile/model, request index, input / cache-read / cache-write /
  output tokens, total prompt tokens and per-request cache hit rate.
- Native save dialog (`@tauri-apps/plugin-dialog`) and a UTF-8 BOM so Excel
  opens Chinese titles correctly. The export always covers the full history,
  independent of the page filters and of the 200-turn UI cap.
- New Tauri command `export_usage_csv` and core `usage::export_csv`.

### Notes

- The CSV is generated from the same de-duplicated transcript parsing as the
  dashboard; cache columns stay 0 when the gateway does not report them.

## [0.3.0] - 2026-10-22

### Added

- **Usage Insights (用量洞察)** — a new top-level view next to Sessions.
  Per-session token accounting parsed from the isolated Claude/Codex
  transcripts, de-duplicated by message id so streaming chunks no longer
  inflate input/output counts.
- Prompt / cache-read / cache-write / output totals, a 14-day aggregate
  chart, per-turn SVG bars in the session drawer, filters by profile, CLI
  and time range, plus search.
- **Server cache card** — reads vLLM `/metrics` (`prefix_cache_hits_total`,
  `prefix_cache_queries_total`, KV usage, running requests) for the selected
  local profile and refreshes every 12 s. This is the authoritative cache
  hit rate when the Anthropic-compatible gateway does not return
  `cache_read_input_tokens` (the CLI-side number stays at 0%).
- Explain banner when the gateway omits cache fields, connecting the client
  0% to the server-side number.

### Changed

- New Tauri commands `list_session_usage` and `get_server_cache`. Transcript
  scans are cached by (path, mtime, size), so multi-megabyte JSONL files are
  not re-read on every poll.
- Version bumped to 0.3.0 across desktop, core and CLI.

### Notes

- Client cache numbers are only meaningful for gateways that return
  Anthropic cache usage; vLLM's Anthropic frontend currently does not.

## [0.2.6] - 2026-10-15

### Fixed

- Resume live Claude background sessions with `claude attach` in the same
  isolated profile home instead of starting a competing `--resume` writer.
  The GUI offers "Attach background session" while a background owner is live.
- Detect session ownership from actual agent arguments and Claude's session
  registry, including process creation identity. Terminal hosts, stopped tabs,
  fork source transcripts and unrelated prompts no longer keep sessions open.
  Incomplete scans remain unverified instead of allowing duplicate writers.
- Validate CLI startup with a successful, time-bounded version probe. Broken npm
  shims and native-binary placeholders left by a full disk are reported before
  opening a terminal, and no longer count as healthy installed CLIs.
- Roll back failed launches and release session locks when their CLI exits,
  even if the terminal shell remains open. Ownership tokens protect newer locks;
  CLI resumes now enforce the same exclusive lock as the GUI.
- Preserve launch warnings in the session UI, protect active transcripts from
  deletion, and skip PowerShell profiles for predictable terminal startup.
- Route release downloads through the configured/system proxy and provide
  clearer network failure messages.

## [0.2.5] - 2026-09-17

### Fixed

- Windows: the per-session start script is now written with a UTF-8 BOM, so
  workspace paths containing non-ASCII characters (e.g. Chinese) no longer
  come out garbled and break `Set-Location` when launching from the GUI.

### Performance

- Status checks are faster: the Codex/Claude version probes run in parallel
  and the cache lock is released while they run.
- Launches no longer wait on the network: the Codex model-list sync now runs
  in the background.
- The Sessions page refresh is faster: transcript heads are streamed and stop
  reading as soon as the working directory and first message are known, and
  pools with many sessions parse in parallel.

## [0.2.4] - 2026-09-13

### Added

- Built-in update system, on every end:
  - GUI: checks the official GitHub releases on launch (cached,
    non-blocking) and offers a confirmation dialog when a new version
    is available; confirming shows a download progress bar, verifies
    the package's SHA-256 against the release's `SHA256SUMS.txt`,
    replaces the old version and relaunches automatically (macOS
    swaps the .app bundle, Windows runs the new installer after the
    app exits). The About page adds a manual "check for updates"
    button.
  - CLI: a one-line notice on every launch when a newer version exists
    (network wait capped at 2.5s, disable with
    `AGENT_SWITCH_NO_UPDATE_CHECK=1`), plus `agent-switch update`
    which downloads, checksum-verifies and replaces the binary in
    place (`--check` only reports the latest version).

## [0.2.3] - 2026-09-13

### Fixed

- Apps launched from the Dock, Finder or Start menu no longer report the
  Codex/Claude CLIs as not installed: when the shell-less process PATH
  misses a CLI, the app now probes well-known install locations (Homebrew,
  npm/bun/volta globals, nvm/fnm/asdf/mise node trees, scoop, nvm-windows)
  and, on macOS/Linux, the user's login-shell PATH.
- The version probe adds the CLI's own directory to the child PATH so the
  `#!/usr/bin/env node` shebang resolves even in a minimal GUI environment.
- Ship the installer fixes committed after the v0.2.2 tag (release-metadata
  JSON parsing, Windows asset enumeration) — the published v0.2.2
  installers predate them.

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
