# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
