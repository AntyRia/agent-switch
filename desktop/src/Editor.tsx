// Editor page: create (id == null) or edit an existing profile.
// Engine-aware: the CLI dropdown (codex/claude) switches the Base URL /
// model placeholders and reveals the key-mode dropdown for claude.
// Legacy provider types (openai-compatible/openai) are mapped to the new
// relay/vllm categories on load and normalized on save.

import { useEffect, useState } from "react";
import {
  api,
  errorMessage,
  type ModelListResult,
  type Profile,
  type TestResult,
} from "./api";
import { useI18n } from "./i18n";

interface EditorProps {
  /** null = creating a new provider; otherwise the existing profile id.
   *  (The id is an internal, auto-generated UUID — never shown or edited.) */
  id: string | null;
  onBack: () => void;
  /** Called after a successful save; App navigates back + shows toast. */
  onSaved: () => void;
}

interface FormState {
  id: string;
  name: string;
  description: string;
  cli: "codex" | "claude";
  provider_type: string;
  base_url: string;
  api_key: string;
  model: string;
  // Claude only: reasoning effort, injected as CLAUDE_CODE_EFFORT_LEVEL.
  // Empty = the CLI's default.
  effort: string;
  // Codex only: context window (tokens) for the generated model catalog.
  // Empty = Codex's default window; auto-filled from the server's
  // max_model_len when fetching the model list.
  context_window: string;
  // Codex only: every known model id, comma/space separated (server list
  // merged with manual entries). Written into the model catalog, which is
  // what the TUI's /model switcher offers.
  model_list: string;
  // Claude only: how the key is sent ("auth_token" = Bearer, the relay
  // convention and default; "api_key" = x-api-key).
  auth_mode: "auth_token" | "api_key";
  // Hidden field: preserved from the loaded profile when editing, defaults
  // to the profile id when creating (same behavior as the CLI `add` command).
  provider_name: string;
  // Hidden field (spec §9): env var name for the API key. Preserved from the
  // loaded profile when editing; null when creating. Not editable in the GUI.
  api_key_env: string | null;
}

type TestState =
  | { status: "idle" }
  | { status: "busy" }
  | { status: "ok"; message: string; model_count: number | null }
  | { status: "fail"; message: string };

const EMPTY_FORM: FormState = {
  id: "",
  name: "",
  description: "",
  cli: "codex",
  provider_type: "relay",
  base_url: "https://",
  api_key: "",
  model: "",
  effort: "",
  context_window: "",
  model_list: "",
  auth_mode: "auth_token",
  provider_name: "",
  api_key_env: null,
};

/** Sentinel option value: the user opted out of the fetched model list
 *  and wants a free-text model id instead. */
const MODEL_CUSTOM = "__custom__";

/** Split the model-list field into clean, de-duplicated ids
 *  (comma/semicolon/whitespace separated, both scripts' separators). */
function parseModelList(s: string): string[] {
  const seen = new Set<string>();
  const out: string[] = [];
  for (const part of s.split(/[\s,;，；]+/)) {
    const m = part.trim();
    if (m !== "" && !seen.has(m)) {
      seen.add(m);
      out.push(m);
    }
  }
  return out;
}

/** Map a legacy provider type to the current categories. "official"
 *  passes through; a local base URL (localhost / 127.0.0.1) is assumed to
 *  be a self-hosted (vllm) deployment. */
function mapProviderType(type: string, baseUrl: string): string {
  if (type === "openai-compatible") return "vllm";
  if (type === "relay" || type === "vllm" || type === "official") return type;
  const local = /^https?:\/\/(localhost|127\.0\.0\.1)/.test(baseUrl);
  return local ? "vllm" : "relay";
}

export default function Editor({ id, onBack, onSaved }: EditorProps) {
  const { t } = useI18n();
  const editing = id !== null;
  const [form, setForm] = useState<FormState>(EMPTY_FORM);
  const [loading, setLoading] = useState(editing);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [showKey, setShowKey] = useState(false);
  const [test, setTest] = useState<TestState>({ status: "idle" });
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  // Fetched model list (null = not fetched for the current base_url/cli),
  // plus its status message. When non-empty the model field renders as a
  // <select> listing every fetched model (a datalist would filter by the
  // typed value and hide the other entries).
  const [models, setModels] = useState<string[] | null>(null);
  const [modelsBusy, setModelsBusy] = useState(false);
  // True = free-text model input instead of the fetched-list select.
  const [modelCustom, setModelCustom] = useState(false);
  const [modelsMsg, setModelsMsg] = useState<{
    kind: "ok" | "fail";
    text: string;
  } | null>(null);
  // Result line for the official-account login action.
  const [loginMsg, setLoginMsg] = useState<{
    kind: "ok" | "fail";
    text: string;
  } | null>(null);

  // Load the existing profile when editing.
  useEffect(() => {
    if (!id) return;
    let cancelled = false;
    api
      .getProfile(id)
      .then((p) => {
        if (cancelled) return;
        setForm({
          id: p.id,
          name: p.name,
          description: p.description,
          cli: p.cli === "claude" ? "claude" : "codex",
          provider_type: mapProviderType(p.provider.type, p.provider.base_url),
          base_url: p.provider.base_url,
          api_key: p.provider.api_key,
          model: p.model.default,
          effort: p.model.effort ?? "",
          context_window:
            p.model.context_window != null ? String(p.model.context_window) : "",
          model_list: (p.model.models ?? []).join(", "),
          auth_mode:
            p.provider.auth_mode === "api_key" ? "api_key" : "auth_token",
          provider_name: p.codex.provider_name,
          api_key_env: p.provider.api_key_env,
        });
        setLoading(false);
      })
      .catch((e) => {
        if (cancelled) return;
        setLoadError(errorMessage(e));
        setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [id]);

  // A fetched list is only valid for the base URL + engine it came from.
  useEffect(() => {
    setModels(null);
    setModelsMsg(null);
    setModelCustom(false);
  }, [form.base_url, form.cli]);

  function set<K extends keyof FormState>(key: K, value: FormState[K]) {
    setForm((f) => ({ ...f, [key]: value }));
  }

  // Fetch the provider's model list (mimics ccswitch's "获取模型列表"):
  // populate the datalist and auto-fill the first model when the field is
  // still empty.
  async function runFetchModels() {
    setModelsBusy(true);
    setModelsMsg(null);
    try {
      const r: ModelListResult = await api.fetchModels({
        base_url: effectiveBaseUrl,
        api_key: form.api_key,
        api_key_env: form.api_key_env,
        engine: form.cli,
        auth_mode: form.cli === "claude" ? form.auth_mode : null,
        provider_type: form.provider_type,
      });
      if (r.ok && r.models.length > 0) {
        setModels(r.models);
        // STRICT sync (mirrors core's merge_model_list): the stored list is
        // replaced by the upstream list, so a model the provider no longer
        // serves is never offered. The current default model is always kept
        // (relays regularly omit it from /models) and `codex-auto-*` ids are
        // excluded (OpenAI built-ins, meaningless for third-party profiles).
        const current = parseModelList(form.model_list);
        const defaultModel = form.model.trim();
        const merged: string[] = [];
        if (defaultModel !== "" && !r.models.includes(defaultModel)) {
          merged.push(defaultModel);
        }
        for (const m of r.models) {
          if (m.startsWith("codex-auto-") || merged.includes(m)) continue;
          merged.push(m);
        }
        const added = merged.filter((m) => !current.includes(m)).length;
        const removed = current.filter(
          (m) => m !== defaultModel && !merged.includes(m),
        ).length;
        set("model_list", merged.join(", "));
        setModelsMsg({
          kind: "ok",
          text:
            added > 0 || removed > 0
              ? t("modelsSynced", { a: added, b: removed })
              : t("modelsFound", { n: r.models.length }),
        });
        // vLLM servers advertise max_model_len: auto-fill the context
        // window while the field is still empty (codex engine only).
        if (
          form.cli === "codex" &&
          r.max_model_len != null &&
          !form.context_window.trim()
        ) {
          set("context_window", String(r.max_model_len));
        }
        if (!form.model.trim()) {
          set("model", r.models[0]);
          setModelCustom(false);
        } else if (!r.models.includes(form.model.trim())) {
          // Keep the already-typed model: switch to the free-text input so
          // it is not silently replaced by the list.
          setModelCustom(true);
        }
      } else if (r.ok) {
        setModelsMsg({ kind: "fail", text: t("modelsNotFound") });
      } else {
        setModelsMsg({ kind: "fail", text: r.message });
      }
    } catch (e) {
      setModelsMsg({ kind: "fail", text: errorMessage(e) });
    } finally {
      setModelsBusy(false);
    }
  }

  async function runTest() {
    setTest({ status: "busy" });
    try {
      const r: TestResult = await api.testConnection({
        base_url: effectiveBaseUrl,
        api_key: form.api_key,
        api_key_env: form.api_key_env,
        model: form.model.trim(),
        engine: form.cli,
        auth_mode: form.cli === "claude" ? form.auth_mode : null,
        provider_type: form.provider_type,
      });
      setTest(
        r.ok
          ? {
              status: "ok",
              message: r.message,
              model_count: r.model_count,
            }
          : { status: "fail", message: r.message },
      );
    } catch (e) {
      setTest({ status: "fail", message: errorMessage(e) });
    }
  }

  // Official-account login: opens a terminal in the profile's isolated home
  // (codex login / the claude login screen). The credential stays in that
  // home; the profile then launches without any key.
  async function runLogin() {
    setLoginMsg(null);
    try {
      const r = await api.loginProfile(form.id);
      setLoginMsg({
        kind: "ok",
        text: r.warning ? `⚠ ${r.warning}` : t("loginStarted"),
      });
    } catch (e) {
      setLoginMsg({ kind: "fail", text: t("loginFail", { err: errorMessage(e) }) });
    }
  }

  async function runSave() {
    setSaveError(null);
    setSaving(true);
    // The id is internal: the loaded id when editing, a fresh UUID when
    // creating (crypto.randomUUID — unique, never surfaced in the UI).
    const trimmedId = editing ? form.id.trim() : crypto.randomUUID();
    const cw = form.context_window.trim();
    // The context window is optional, but when present it must be a
    // positive integer (it is written verbatim into the catalog JSON).
    if (form.cli === "codex" && cw !== "" && !/^\d+$/.test(cw)) {
      setSaveError(t("contextWindowInvalid"));
      setSaving(false);
      return;
    }
    const profile: Profile = {
      id: trimmedId,
      name: form.name.trim(),
      description: form.description.trim(),
      cli: form.cli,
      provider: {
        type: form.provider_type.trim(),
        // Official: the vendor endpoint is hardcoded (the field is not
        // shown); everything else saves the typed value.
        base_url: isOfficial ? effectiveBaseUrl : form.base_url.trim(),
        api_key: form.api_key,
        api_key_env: form.api_key_env,
        // Key mode is only meaningful for the claude engine; the official
        // claude always uses the vendor's x-api-key header (enforced by the
        // backend), so nothing is stored for it.
        auth_mode:
          form.cli === "claude" && form.provider_type !== "official"
            ? form.auth_mode
            : null,
      },
      model: {
        default: form.model.trim(),
        effort:
          form.cli === "claude" && form.effort.trim() !== ""
            ? form.effort.trim()
            : null,
        context_window:
          form.cli === "codex" && cw !== "" && /^\d+$/.test(cw)
            ? Number(cw)
            : null,
        // The catalog must always carry the default model, so add it to
        // the parsed list (order kept; duplicates dropped).
        models: (() => {
          const ids = parseModelList(form.model_list);
          const def = form.model.trim();
          return def !== "" && !ids.includes(def) ? [def, ...ids] : ids;
        })(),
      },
      codex: {
        // Backend validation requires a non-empty provider name; fall back
        // to the id (matches the CLI) when nothing was stored.
        provider_name: form.provider_name.trim() || trimmedId,
      },
    };
    try {
      // Creating with an id that already exists must fail, not overwrite.
      await api.saveProfile(profile, !editing);
      onSaved();
    } catch (e) {
      setSaveError(errorMessage(e));
      setSaving(false);
    }
  }

  const isClaude = form.cli === "claude";
  const isOfficial = form.provider_type === "official";
  // Official endpoints are fixed (never editable in the form).
  const OFFICIAL_URLS = {
    codex: "https://api.openai.com/v1",
    claude: "https://api.anthropic.com",
  } as const;
  const effectiveBaseUrl = isOfficial
    ? OFFICIAL_URLS[form.cli]
    : form.base_url.trim();
  // Models the select offers once a fetch happened: the merged list
  // (fetched + saved), i.e. exactly what the model catalog will carry.
  const listOptions =
    models !== null && models.length > 0
      ? parseModelList(form.model_list)
      : [];

  if (loading) {
    return <p className="hint">{t("editorLoad")}</p>;
  }
  if (loadError) {
    return (
      <div className="panel">
        <p className="error-text">{t("editorLoadError", { err: loadError })}</p>
        <div className="button-row">
          <button type="button" className="btn secondary" onClick={onBack}>
            {t("back")}
          </button>
        </div>
      </div>
    );
  }

  return (
    <div className="panel editor">
      <div className="editor-head">
        <button type="button" className="btn secondary small" onClick={onBack}>
          ← {t("back")}
        </button>
        <h2>{editing ? t("editTitle", { name: form.name }) : t("newTitle")}</h2>
      </div>

      <div className="field-grid">
        <label className="field">
          <span>{t("fName")}</span>
          <input
            type="text"
            value={form.name}
            placeholder={t("fNamePh")}
            onChange={(e) => set("name", e.target.value)}
          />
        </label>

        <label className="field">
          <span>{t("fDesc")}</span>
          <input
            type="text"
            value={form.description}
            placeholder={t("fDescPh")}
            onChange={(e) => set("description", e.target.value)}
          />
        </label>

        <label className="field">
          <span>{t("fCli")}</span>
          <select
            value={form.cli}
            onChange={(e) =>
              set("cli", e.target.value === "claude" ? "claude" : "codex")
            }
          >
            <option value="codex">Codex (OpenAI)</option>
            <option value="claude">Claude (Anthropic)</option>
          </select>
        </label>

        <label className="field">
          <span>{t("fType")}</span>
          <select
            value={
              form.provider_type === "vllm" || form.provider_type === "official"
                ? form.provider_type
                : "relay"
            }
            onChange={(e) => {
              const v = e.target.value;
              setForm((f) => ({ ...f, provider_type: v }));
            }}
          >
            <option value="relay">{t("typeRelayOpt")}</option>
            <option value="vllm">{t("typeOpenaiCompOpt")}</option>
            <option value="official">{t("typeOfficialOpt")}</option>
          </select>
          <span className="hint">
            {form.provider_type === "official"
              ? t("typeOfficialHint")
              : t("typeOpenaiCompHint")}
          </span>
        </label>

        {/* Key mode is a relay convention; the official claude always uses
            the vendor's x-api-key header, so the dropdown is hidden there. */}
        {isClaude && !isOfficial && (
          <label className="field">
            <span>{t("fAuthMode")}</span>
            <select
              value={form.auth_mode}
              onChange={(e) =>
                set(
                  "auth_mode",
                  e.target.value === "api_key" ? "api_key" : "auth_token",
                )
              }
            >
              <option value="auth_token">{t("authBearer")}</option>
              <option value="api_key">{t("authApiKey")}</option>
            </select>
          </label>
        )}

        {/* Official: the vendor endpoint is fixed — the field is hidden and
            the hardcoded URL is written on save (mirrors cc-switch). */}
        {!isOfficial && (
          <label className="field span-2">
            <span>{t("fBaseUrl")}</span>
            <input
              type="text"
              value={form.base_url}
              placeholder={
                isClaude ? t("basePhClaude") : t("basePhCodex")
              }
              onChange={(e) => set("base_url", e.target.value)}
            />
          </label>
        )}

        <label className="field span-2">
          <span>{t("fKey")}</span>
          <div className="key-row">
            <input
              type={showKey ? "text" : "password"}
              value={form.api_key}
              placeholder={isOfficial ? t("keyPhOfficial") : t("keyPh")}
              onChange={(e) => set("api_key", e.target.value)}
            />
            <button
              type="button"
              className="btn secondary small"
              onClick={() => setShowKey((s) => !s)}
            >
              {showKey ? t("hideKey") : t("showKey")}
            </button>
          </div>
        </label>

        <div className="field span-2">
          <div className="field-head">
            <span>{t("fModel")}</span>
            <button
              type="button"
              className="btn link"
              onClick={runFetchModels}
              disabled={modelsBusy || !effectiveBaseUrl}
            >
              {modelsBusy ? t("fetchingModels") : t("fetchModelsBtn")}
            </button>
          </div>
          {listOptions.length > 0 && !modelCustom ? (
            <select
              value={form.model || listOptions[0]}
              onChange={(e) => {
                if (e.target.value === MODEL_CUSTOM) {
                  setModelCustom(true);
                  return;
                }
                set("model", e.target.value);
              }}
            >
              {form.model !== "" && !listOptions.includes(form.model) && (
                <option value={form.model}>{form.model}</option>
              )}
              {listOptions.map((m) => (
                <option key={m} value={m}>
                  {m}
                </option>
              ))}
              <option value={MODEL_CUSTOM}>{t("modelCustomOpt")}</option>
            </select>
          ) : (
            <>
              <input
                type="text"
                value={form.model}
                placeholder={isClaude ? t("modelPhClaude") : t("modelPhCodex")}
                onChange={(e) => set("model", e.target.value)}
              />
              {models !== null && models.length > 0 && modelCustom && (
                <button
                  type="button"
                  className="btn link"
                  onClick={() => setModelCustom(false)}
                >
                  {t("modelFromList")}
                </button>
              )}
            </>
          )}
          <span className="hint">
            {isClaude ? t("modelHintClaude") : t("modelHintCodex")}
          </span>
          {modelsMsg && (
            <span className={`test-result ${modelsMsg.kind}`}>
              {modelsMsg.text}
            </span>
          )}
        </div>

        {!isClaude && (
          <label className="field span-2">
            <span>{t("fModelList")}</span>
            <input
              type="text"
              value={form.model_list}
              placeholder={t("modelListPh")}
              onChange={(e) => set("model_list", e.target.value)}
            />
            <span className="hint">{t("modelListHint")}</span>
          </label>
        )}

        {!isClaude && (
          <label className="field span-2">
            <span>{t("fContextWindow")}</span>
            <input
              type="text"
              inputMode="numeric"
              value={form.context_window}
              placeholder={t("contextWindowPh")}
              onChange={(e) => set("context_window", e.target.value)}
            />
            <span className="hint">{t("contextWindowHint")}</span>
          </label>
        )}

        {isClaude && (
          <label className="field span-2">
            <span>{t("fEffort")}</span>
            <input
              type="text"
              value={form.effort}
              placeholder={t("effortPh")}
              onChange={(e) => set("effort", e.target.value)}
            />
            <span className="hint">{t("effortHint")}</span>
          </label>
        )}
      </div>

      <div className="test-row">
        <button
          type="button"
          className="btn secondary"
          onClick={runTest}
          disabled={test.status === "busy"}
        >
          {test.status === "busy" ? t("testing") : t("testBtn")}
        </button>

        {isOfficial && editing && (
          <button
            type="button"
            className="btn secondary"
            onClick={runLogin}
            title={t("loginHint")}
          >
            {t("loginBtn")}
          </button>
        )}

        {loginMsg && (
          <span className={`test-result ${loginMsg.kind}`}>{loginMsg.text}</span>
        )}

        {test.status === "ok" && (
          <span className="test-result ok">
            {t("testOk")}
            {test.model_count !== null &&
              ` — ${t("modelsCount", { n: test.model_count })}`}
            {test.message ? ` — ${test.message}` : ""}
          </span>
        )}
        {test.status === "fail" && (
          <span className="test-result fail">
            {t("testFail", { msg: test.message })}
          </span>
        )}
      </div>

      {saveError && <p className="error-text">{t("saveError", { err: saveError })}</p>}

      <div className="button-row">
        <button type="button" className="btn secondary" onClick={onBack}>
          {t("cancel")}
        </button>
        <button
          type="button"
          className="btn primary"
          onClick={runSave}
          disabled={
            saving ||
            !form.name.trim() ||
            (editing && form.id.trim() === "")
          }
        >
          {saving ? t("saving") : t("save")}
        </button>
      </div>
    </div>
  );
}
