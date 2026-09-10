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
  /** null = creating a new profile; otherwise the existing profile id. */
  id: string | null;
  /** Ids of all existing profiles (create flow only): the auto-generated
   *  id is suffixed until it does not collide. */
  existingIds: string[];
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
  auth_mode: "auth_token",
  provider_name: "",
  api_key_env: null,
};

/** Derive a profile id from the display name: lowercased, non
 *  [a-z0-9] runs collapsed to "-", capped at 32 chars, "" → "provider".
 *  Collides with `existing` get a -2/-3/… suffix. */
function generateId(name: string, existing: string[]): string {
  const base =
    name
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-+|-+$/g, "")
      .slice(0, 32) || "provider";
  let id = base;
  let n = 2;
  while (existing.includes(id)) {
    id = `${base}-${n}`;
    n++;
  }
  return id;
}

/** Map a legacy provider type to the relay/vllm categories. A local
 *  base URL (localhost / 127.0.0.1) is assumed to be a vLLM deployment. */
function mapProviderType(type: string, baseUrl: string): string {
  if (type === "relay" || type === "vllm") return type;
  const local = /^https?:\/\/(localhost|127\.0\.0\.1)/.test(baseUrl);
  return local ? "vllm" : "relay";
}

export default function Editor({
  id,
  existingIds,
  onBack,
  onSaved,
}: EditorProps) {
  const { t } = useI18n();
  const editing = id !== null;
  const [form, setForm] = useState<FormState>(EMPTY_FORM);
  // Once the user types in the ID field (create flow) the auto-generated
  // id stops tracking the name.
  const [idTouched, setIdTouched] = useState(false);
  const [loading, setLoading] = useState(editing);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [showKey, setShowKey] = useState(false);
  const [test, setTest] = useState<TestState>({ status: "idle" });
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  // Fetched model list for the model field's datalist (null = not fetched
  // for the current base_url/cli), plus its status message.
  const [models, setModels] = useState<string[] | null>(null);
  const [modelsBusy, setModelsBusy] = useState(false);
  const [modelsMsg, setModelsMsg] = useState<{
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
  }, [form.base_url, form.cli]);

  // Create flow: keep the id in sync with the name (regenerate on every
  // name change / new-profile-id appearance) until the user edits it.
  useEffect(() => {
    if (!editing && !idTouched) {
      setForm((f) => ({ ...f, id: generateId(f.name, existingIds) }));
    }
  }, [form.name, existingIds, editing, idTouched]);

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
        base_url: form.base_url.trim(),
        api_key: form.api_key,
        engine: form.cli,
        auth_mode: form.cli === "claude" ? form.auth_mode : null,
      });
      if (r.ok && r.models.length > 0) {
        setModels(r.models);
        setModelsMsg({
          kind: "ok",
          text: t("modelsFound", { n: r.models.length }),
        });
        if (!form.model.trim()) set("model", r.models[0]);
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
        base_url: form.base_url.trim(),
        api_key: form.api_key,
        model: form.model.trim(),
        engine: form.cli,
        auth_mode: form.cli === "claude" ? form.auth_mode : null,
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

  async function runSave() {
    setSaveError(null);
    setSaving(true);
    const trimmedId = form.id.trim();
    const profile: Profile = {
      id: trimmedId,
      name: form.name.trim(),
      description: form.description.trim(),
      cli: form.cli,
      provider: {
        type: form.provider_type.trim(),
        base_url: form.base_url.trim(),
        api_key: form.api_key,
        api_key_env: form.api_key_env,
        // Key mode is only meaningful for the claude engine.
        auth_mode: form.cli === "claude" ? form.auth_mode : null,
      },
      model: {
        default: form.model.trim(),
        effort:
          form.cli === "claude" && form.effort.trim() !== ""
            ? form.effort.trim()
            : null,
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
        <h2>{editing ? t("editTitle", { id: id! }) : t("newTitle")}</h2>
      </div>

      <div className="field-grid">
        <label className="field">
          <span>
            {t("fId")}{" "}
            {editing && <em className="muted">{t("fIdReadOnly")}</em>}
          </span>
          <input
            type="text"
            value={form.id}
            readOnly={editing}
            placeholder={t("fIdPh")}
            onChange={(e) => {
              set("id", e.target.value);
              setIdTouched(true);
            }}
          />
          {!editing && <span className="hint">{t("idAutoHint")}</span>}
        </label>

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
            value={form.provider_type === "vllm" ? "vllm" : "relay"}
            onChange={(e) =>
              set("provider_type", e.target.value === "vllm" ? "vllm" : "relay")
            }
          >
            <option value="relay">{t("typeRelayOpt")}</option>
            <option value="vllm">{t("typeVllmOpt")}</option>
          </select>
        </label>

        {isClaude && (
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

        <label className="field span-2">
          <span>{t("fKey")}</span>
          <div className="key-row">
            <input
              type={showKey ? "text" : "password"}
              value={form.api_key}
              placeholder={t("keyPh")}
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
              disabled={modelsBusy || !form.base_url.trim()}
            >
              {modelsBusy ? t("fetchingModels") : t("fetchModelsBtn")}
            </button>
          </div>
          <input
            type="text"
            list="model-options"
            value={form.model}
            placeholder={isClaude ? t("modelPhClaude") : t("modelPhCodex")}
            onChange={(e) => set("model", e.target.value)}
          />
          {models !== null && (
            <datalist id="model-options">
              {models.map((m) => (
                <option key={m} value={m} />
              ))}
            </datalist>
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
          disabled={saving || !form.id.trim() || !form.name.trim()}
        >
          {saving ? t("saving") : t("save")}
        </button>
      </div>
    </div>
  );
}
