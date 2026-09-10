// Thin typed wrapper around the Tauri backend commands.
// Invoke names and JS-side argument keys must match the Rust parameter
// names in src-tauri/src/commands.rs exactly (snake_case, no conversion).

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
  /** "relay" | "vllm" (legacy "openai-compatible"/"openai" may appear). */
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
  testConnection: (t: {
    base_url: string;
    api_key: string;
    model: string;
    engine: string;
    auth_mode: string | null;
  }): Promise<TestResult> => invoke("test_connection", { t }),
  // JS arg key "m" matches the Rust parameter name. The engine picks the
  // endpoint (codex → GET /models, claude → GET /v1/models).
  fetchModels: (m: {
    base_url: string;
    api_key: string;
    engine: string;
    auth_mode: string | null;
  }): Promise<ModelListResult> => invoke("fetch_models", { m }),
  launchProfile: (id: string, workspace: string | null): Promise<LaunchResult> =>
    invoke("launch_profile", { id, workspace }),
  listSessions: (): Promise<Session[]> => invoke("list_sessions"),
  resumeSession: (session_id: string): Promise<LaunchResult> =>
    invoke("resume_session", { session_id }),
  getStatus: (): Promise<Status> => invoke("get_status"),
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
