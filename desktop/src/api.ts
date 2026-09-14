// Thin typed wrapper around the Tauri backend commands.
// Invoke names are the snake_case Rust fn names; JS-side ARGUMENT keys are
// camelCase of the Rust parameter names (Tauri 2 default), e.g. Rust
// `session_id: String` is passed as `{ sessionId: ... }`.

import { invoke } from "@tauri-apps/api/core";

/** "codex" (OpenAI protocol) or "claude" (Anthropic protocol). */
export type Cli = "codex" | "claude";

/** View shape returned by list_profiles(): no secrets, key presence flag. */
export interface ProfileView {
  id: string;
  name: string;
  description: string;
  model: string;
  base_url: string;
  /** "relay" | "openai-compatible" | "official" (the backend normalizes
   *  legacy "vllm"/"openai" on read, so old files surface as the new
   *  value). */
  provider_type: string;
  cli: Cli;
  has_api_key: boolean;
}

/** Full profile shape used by get_profile()/save_profile().
 *  api_key is the real string, or "" when absent. */
export interface Profile {
  id: string;
  name: string;
  description: string;
  cli: Cli;
  provider: {
    type: string;
    base_url: string;
    api_key: string;
    /** Env var name for the API key, or null when absent. Preserved on save. */
    api_key_env: string | null;
    /** Claude only: "auth_token" (Bearer) | "api_key" (x-api-key); null = default. */
    auth_mode: string | null;
  };
  model: {
    default: string;
    /** Claude only: reasoning effort (CLAUDE_CODE_EFFORT_LEVEL); null = CLI default. */
    effort: string | null;
    /** Codex only: context window in tokens; null = Codex default (272000). */
    context_window: number | null;
    /** Every known model id for the provider. Strictly synced from the
     *  server on every new codex launch (models the upstream no longer
     *  serves are removed, new ones added, the default model always kept)
     *  and written into the model catalog — this is what /model offers in
     *  the TUI. */
    models: string[];
  };
  codex: {
    provider_name: string;
  };
}

export interface TestResult {
  ok: boolean;
  message: string;
  model_count: number | null;
}

export interface ModelListResult {
  ok: boolean;
  message: string;
  /** Model ids in server order (empty on failure or when none reported). */
  models: string[];
  /** Context window in tokens when the server advertises one
   *  (vLLM's max_model_len); null otherwise. */
  max_model_len: number | null;
}

export interface LaunchResult {
  runtime_id: string;
  script_path: string;
  warning: string | null;
}

/** One resumable conversation from the per-profile session pool. */
export interface Session {
  session_id: string;
  engine: Cli;
  profile_id: string;
  /** Profile display name; falls back to profile_id when the profile
   *  was deleted (the row stays, just unresumable). */
  profile_name: string;
  /** Provider category chip ("relay"/"vllm"/…); "" when the profile is gone. */
  provider_type: string;
  cwd: string | null;
  /** First user message (flattened, capped). */
  preview: string;
  /** Last activity, unix seconds. */
  modified: number;
  /** True while the owning profile still exists (resume possible). */
  resumable: boolean;
  /** Pinned by the user (floats to the top of the pool). */
  pinned: boolean;
  /** Custom display title; null when the preview is the row's label. */
  title: string | null;
  /** True while a launched terminal still runs this session — it can only
   *  be resumed again after that terminal window is closed. */
  open: boolean;
}

export interface Status {
  codex_found: boolean;
  codex_version: string | null;
  claude_found: boolean;
  claude_version: string | null;
  config_dir: string;
  profiles_count: number;
  version: string;
}

/** Global launch settings (settings.toml, shared with the CLI). */
export interface SettingsData {
  /** Dangerous mode: launch CLIs with their bypass flags. */
  dangerous_mode: boolean;
  /** Proxy host ("" = disabled / direct connection). */
  proxy_host: string;
  /** Proxy port (used only when proxy_host is non-empty). */
  proxy_port: number;
  /** Pinned terminal path ("" = auto-detect). */
  terminal: string;
  /** Where the settings file lives (shown in the UI footer). */
  path: string;
}

/** What the UI sends back on save (path is backend-managed). */
export interface SettingsDraft {
  dangerous_mode: boolean;
  proxy_host: string;
  proxy_port: number;
  terminal: string;
}

/** One terminal the launcher knows how to open (auto-detect order). */
export interface TerminalInfo {
  label: string;
  bin: string;
  path: string;
}

/** Latest official release info (built-in update system). */
export interface UpdateInfo {
  current: string;
  /** null when the release API was unreachable and no cache exists. */
  latest: string | null;
  has_update: boolean;
  release_url: string | null;
  /** The package this machine would download; null = no build for this
   *  platform in the release. */
  asset_name: string | null;
}

/** Payload of the "update-progress" event (bytes). */
export interface UpdateProgress {
  done: number;
  total: number;
}

/** The tail of the log file for the log viewer. */
export interface LogsResult {
  /** null until the log file has been opened at least once. */
  path: string | null;
  lines: string[];
}

export const api = {
  listProfiles: (): Promise<ProfileView[]> => invoke("list_profiles"),
  getProfile: (id: string): Promise<Profile> => invoke("get_profile", { id }),
  // JS arg keys "p" / "create" match the Rust parameter names.
  // create=true (New profile flow) makes an existing id an error.
  saveProfile: (p: Profile, create = false): Promise<{ id: string }> =>
    invoke("save_profile", { p, create }),
  deleteProfile: (id: string): Promise<{ id: string }> =>
    invoke("delete_profile", { id }),
  // JS arg key "t" matches the Rust parameter name. The engine picks the
  // wire protocol (codex → GET /models, claude → POST /v1/messages).
  // provider_type "official" switches to vendor conventions (key optional,
  // x-api-key header for claude).
  testConnection: (t: {
    base_url: string;
    api_key: string;
    api_key_env?: string | null;
    model: string;
    engine: string;
    auth_mode: string | null;
    provider_type: string;
  }): Promise<TestResult> => invoke("test_connection", { t }),
  // JS arg key "m" matches the Rust parameter name. The engine picks the
  // endpoint (codex → GET /models, claude → GET /v1/models).
  fetchModels: (m: {
    base_url: string;
    api_key: string;
    api_key_env?: string | null;
    engine: string;
    auth_mode: string | null;
    provider_type: string;
  }): Promise<ModelListResult> => invoke("fetch_models", { m }),
  launchProfile: (id: string, workspace: string | null): Promise<LaunchResult> =>
    invoke("launch_profile", { id, workspace }),
  // Official-account login: opens a terminal in the profile's isolated
  // home running `codex login` / `claude` (login screen).
  loginProfile: (id: string): Promise<LaunchResult> =>
    invoke("login_profile", { id }),
  listSessions: (): Promise<Session[]> => invoke("list_sessions"),
  resumeSession: (session_id: string): Promise<LaunchResult> =>
    invoke("resume_session", { sessionId: session_id }),
  setSessionPinned: (session_id: string, pinned: boolean): Promise<void> =>
    invoke("set_session_pinned", { sessionId: session_id, pinned }),
  setSessionTitle: (session_id: string, title: string): Promise<void> =>
    invoke("set_session_title", { sessionId: session_id, title }),
  deleteSession: (session_id: string): Promise<void> =>
    invoke("delete_session", { sessionId: session_id }),
  clearUnpinnedSessions: (): Promise<number> =>
    invoke("clear_unpinned_sessions"),
  // force=true bypasses the backend's 30s status cache (About re-check).
  getStatus: (force?: boolean): Promise<Status> => invoke("get_status", { force }),
  getSettings: (): Promise<SettingsData> => invoke("get_settings"),
  saveSettings: (s: SettingsDraft): Promise<void> => invoke("save_settings", { s }),
  getLogs: (lines?: number): Promise<LogsResult> => invoke("get_logs", { lines }),
  detectTerminals: (): Promise<TerminalInfo[]> => invoke("detect_terminals"),
  // Built-in updates: force=true bypasses the backend's 6 h check cache.
  checkForUpdate: (force?: boolean): Promise<UpdateInfo> =>
    invoke("check_for_update", { force }),
  // Downloads + verifies + prepares the install; the app must then call
  // exitApp() (the detached platform installer swaps the running app and
  // relaunches it). "update-progress" events carry the download progress.
  startUpdate: (): Promise<string> => invoke("start_update"),
  exitApp: (): Promise<void> => invoke("exit_app"),
};

/** Backend errors reject with a plain string; normalize any other shape. */
export function errorMessage(e: unknown): string {
  if (typeof e === "string") return e;
  if (e instanceof Error) return e.message;
  try {
    return JSON.stringify(e);
  } catch {
    return String(e);
  }
}
