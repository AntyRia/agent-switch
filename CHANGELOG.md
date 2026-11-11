# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.4.15] - 2026-11-11

### Fixed

- **Codex TUI could not pick a model reasoning effort**: the generated
  model catalog only exposed the profile's default effort level, so the
  Codex TUI's model picker had no strength options to show. Catalog
  entries now carry the full reasoning ladder (none → low → medium →
  high → xhigh → max → ultra), and the effort the user picks is carried
  over when the home's `config.toml` is rewritten on the next launch, so
  the choice persists.
- **Codex strength picker still stale after a catalog fix**: the
  per-home Codex app-server daemon caches `config.toml` and
  `model-catalog.json` in memory for its whole lifetime and serves that
  cached state to every TUI that connects, so a TUI restart alone kept
  showing the old (single-strength) catalog. Launch now stops a daemon
  whose state is older than the rewritten files (Windows verifies the
  process image before terminating, Unix falls back to a plain signal),
  and the next TUI start auto-launches a fresh daemon that reads the
  new catalog.
- **Pool migration left the home split and history looking wrong**: the
  automatic 0.4.9→0.4.10 home move only renamed an entry to the *same*
  name, and the as-is→compact move ran only at that profile's next
  launch. A home a running session kept locked therefore sat in
  `runtime/` with its full-id name forever, and a session listed from a
  leftover home resumed in the profile's (empty) current home, so the
  conversation appeared to vanish. Homes are now migrated
  profile-aware in one pass — a legacy/as-is entry lands directly in the
  profile's compact home, merging with what is already there (transcripts
  are never clobbered), and the as-is→compact move no longer waits for a
  launch. Every session also records the pool dir that actually holds
  its transcript, and resume/attach now launch the CLI inside THAT dir,
  so history from a leftover home is found and carried on.

## [0.4.14] - 2026-11-11

### Added

- **Session pool migration** — the Settings page now shows where the
  session pool lives (default location, entry/session counts, leftover
  0.4.9 `runtime/` entries, open sessions) and lets you change the pool
  path. Saving a new path stops every session launched by agent-switch
  (the CLIs, the terminals hosting them and the daemons holding the
  homes), then moves every profile home to the new folder — homes are
  moved or merged, never copied-over, and Codex daemon junctions are
  repaired in every moved home. The old `runtime/` leftovers and a
  previous custom pool path are folded in too, so the pool is whole
  afterwards.
- **Settings redesign** — every user-modifiable item (dangerous mode,
  proxy host/port, terminal, pool path) has its own 保存修改 button and a
  status chip that states at a glance whether it is on/off, enabled and
  where, or missing. Text fields have a clearly distinct non-edit state
  (flat, dashed, muted — click to edit) and an edit state (accent border
  and glow, Enter saves, Esc cancels) instead of always-editable inputs.

### Fixed

- **Sessions from the 0.4.9 `runtime/` directory were missing from the
  pool**: the pool only enumerated the active `rt/` root, while a home a
  still-running session held could not be renamed out of `runtime/` (see
  0.4.12) and simply stayed there. Every pool read — the session list,
  usage/export, delete/clear, and the session-ownership scan — now
  enumerates all pool roots (active + legacy `runtime/`) and de-duplicates,
  so sessions in a leftover root are visible, resumable and cleanable
  again. The session-ownership scan covers the leftover roots too, so an
  open session in a legacy home is still reported as open (and not
  double-resumed).

## [0.4.13] - 2026-11-11

### Fixed

- **Codex profiles failed to launch with "daemon executable not found at
  …\packages\app-server-daemon\current\bin\codex.exe"** after a home
  migration: Codex (≥ v0.166) points its app-server daemon's `current`
  directory junction at the installed release with an ABSOLUTE path, so
  every home move (0.4.9 `runtime/` → `rt/`, full-id → compact name)
  left the junction dangling and the daemon could not start. The app now
  re-creates that junction (as an absolute junction, the same layout
  Codex and `mklink /J` write) pointing at the release the home actually
  holds — the version recorded in `auto-update-version` when present,
  else the newest `releases/` entry — after every home migration and on
  every Codex launch. The repair is best effort: nothing blocks launch.

## [0.4.12] - 2026-11-11

### Fixed

- **Launch aborted with "IO error: 拒绝访问。 (os error 5)"** when a
  still-running session held a 0.4.9 `runtime/<id>` directory: the
  0.4.10→0.4.11 migration moved every legacy entry into `rt/` on each
  launch, and Windows refuses to rename a directory whose files are still
  open (a terminal left open in it, a background session, a file being
  written) — so every subsequent launch re-ran the migration and failed
  the same way. The per-entry moves are now best effort: an in-use entry
  is skipped with a warning and migrates automatically on a later start
  once the handle is gone; the current launch proceeds. No session data
  is ever deleted — entries are moved, and the legacy `runtime/` root
  stays until every entry has moved.

## [0.4.11] - 2026-11-11

### Fixed

- **Model list fetch and connection test ignored the Settings proxy** (e.g.
  `unreachable … (os error 10060)` for a relay behind Cloudflare while the
  launched CLI reached the very same relay fine through the proxy): the app's
  own HTTP requests — the model-list fetch, the connection test and the
  release check/download — now resolve a proxy the same way the launched CLIs
  get one: the Settings proxy first (the value injected into every launched
  CLI), then `HTTPS_PROXY`/`ALL_PROXY`/`HTTP_PROXY`, then the Windows
  "System Proxy" (Clash & co). Loopback hosts (`localhost`, `127.x`, `::1`)
  and `NO_PROXY` entries always stay direct, so self-hosted vLLM/SGLang
  profiles are never tunneled through the proxy.
- Probes now retry once (after a short pause) on a transport-level failure
  and use a 20 s timeout (was 5 s for the model list, 10 s for the
  connection test), so a flaky-but-reachable relay is no longer reported as
  unreachable.
- Probe error messages now say which route was attempted ("via proxy
  http://…" vs "direct connection, no proxy") instead of the bare error.

### Changed

- The Settings proxy hint now notes that the connection test, the model-list
  fetch and the update download route through the proxy too.

## [0.4.10] - 2026-11-10

### Fixed

- **Codex profiles fail to launch on Windows** ("Error: app server did not
  become ready … path must be shorter than SUN_LEN"): Codex v0.166.0's new
  background app-server daemon binds an AF_UNIX socket inside `CODEX_HOME`,
  and the previous home layout (`runtime/<36-char-profile-uuid>/.codex/...`)
  pushed that socket path to 133+bytes — over Windows' 108-byte `SUN_LEN`
  limit for every username, so the daemon never started and the TUI exited
  with the error.

### Changed

- Per-profile homes moved to a shorter layout so the socket path fits
  (100+bytes; fits usernames up to 7 chars on Windows, unaffected on
  macOS): the root segment `runtime/` is now `rt/`, and per-profile
  directories use a compact name — the profile id verbatim when it is
  already short, otherwise an 8-hex-char token derived from the id (auto
  UUIDs map to their first 8 hex digits).
- Existing installs migrate automatically on next start: `runtime/` →
  `rt/` and full-id dirs → their compact names. Session history stays
  resumable; nothing is deleted.

## [0.4.9] - 2026-10-25

### Added

- The server card now shows average **generation throughput** and **prefill
  throughput** in tokens/s, plus average time-to-first-token and inter-token
  latency. The values come from vLLM's prompt/generation token counters and
  TTFT/ITL histograms.
- Throughput is computed over the interval between the last two `/metrics`
  samples (about 12 s), so the first sample shows "采样中". TTFT/ITL are
  averages since the server started.

## [0.4.8] - 2026-10-22

### Changed

- Simplified the summary area to two wide cards: **total prompt tokens** and
  **total output tokens**. The recent-hit-rate and session-count cards were
  removed; cache hit rate remains in the session table, server card and
  detail drawer.

## [0.4.7] - 2026-10-22

### Fixed

- The scope selector only lists profiles that actually have usage, and it
  automatically falls back to "全部配置" if the selected profile disappears.
- Added a visible "全部配置" reset next to the selector and inside the empty
  state, so a down/misconfigured local profile can always be left.
- The server overview only lists local profiles with usage; clicking a dead
  profile no longer traps the page on an empty table.

## [0.4.6] - 2026-10-22

### Changed

- The primary cache stat is now the **last-30-turn hit rate** (97% on warm
  sessions), with cumulative cache tokens and the cumulative rate as a
  secondary line. This removes the 2.9% headline that was dominated by the
  thousands of pre-flag turns.
- Added a scope note to the server card: server counters run since vLLM
  started and include every session/tool on that profile, while the client
  rate is the selected scope's last 30 turns.

## [0.4.5] - 2026-10-22

### Changed

- The session cache badge and the drawer ring now use the **last 30 turns**
  (weighted) instead of a single last turn. A single turn was noisy when a
  new request carried a large uncached prefix — e.g. 49% on the last turn
  versus 97% over the recent window and on the server.
- The top "cache read" card shows both the recent (30-turn) and cumulative
  rates; the drawer keeps the cumulative line separately. Historical
  pre-flag turns no longer dominate the headline number.
- One page scope: the header "配置范围" selector now drives the stat cards,
  trend, server card and session table together. The independent table
  profile filter and the server-card dropdown were removed.
- With "全部配置", the server card lists every local vLLM profile with its
  own hit rate; click a profile to scope the whole page. Selecting a
  relay/official profile shows "no /metrics" and scopes the client-side data.

## [0.4.4] - 2026-10-22

### Fixed

- Refresh `Cargo.lock` for the 0.4.x version bump so `cargo test --locked`
  and the GitHub release workflow no longer fail before building. The 0.4.3
  tag was withdrawn before any assets were published.

## [0.4.3] - 2026-10-22

### Fixed

- Unified the cache metrics across the page: the top cards and the table's
  token column are cumulative, while the table badge and the drawer ring are
  the last turn; the drawer now also shows the cumulative rate explicitly.
- Distinguish "the gateway did not report cache fields" from a real 0%: a new
  `cache_reported` flag is derived from the transcript, and the table shows
  `未上报` (not `0.0%`) when neither client nor server data exists.
- The server-cache dropdown now lists every profile with a base URL,
  including relays and official endpoints; non-local profiles show a note
  instead of silently missing from the list.

### Notes

- Local vLLM needs `--enable-prompt-tokens-details` for per-request cache
  fields; without it the table uses the profile-wide `/metrics` value (SV).

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
