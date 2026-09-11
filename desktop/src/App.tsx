// Agent Switch — root component.
// State-based view switching (list / editor / sessions / about), no router
// library. Launching a profile opens a lightweight modal (Launch.tsx), not
// a separate view.
// Watches "profiles://changed" so CLI-side TOML edits sync into the UI.
// Bilingual: I18nProvider + a one-click toggle in the top-right corner.

import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  api,
  errorMessage,
  type ProfileView,
  type Status,
} from "./api";
import AboutPage from "./About";
import Editor from "./Editor";
import LaunchModal from "./Launch";
import SessionsPage from "./Sessions";
import SettingsPage from "./Settings";
import { I18nProvider, useI18n, type TKey } from "./i18n";

type View =
  | { name: "list" }
  | { name: "editor"; id: string | null } // null = new profile
  | { name: "sessions" }
  | { name: "settings" }
  | { name: "about" };

export default function App() {
  return (
    <I18nProvider>
      <AppInner />
    </I18nProvider>
  );
}

/** Display label for a provider type. The backend normalizes legacy
 *  "vllm"/"openai" to "openai-compatible" on read; the extra arms cover
 *  data served before a restart. Unknown values pass through raw. */
function typeLabel(
  p: ProfileView,
  t: (key: TKey, vars?: Record<string, string | number>) => string,
): string {
  const v = p.provider_type;
  if (v === "relay") return t("typeRelay");
  if (v === "official") return t("typeOfficial");
  if (v === "vllm" || v === "openai" || v === "openai-compatible") {
    return t("typeOpenaiComp");
  }
  return v;
}

function AppInner() {
  const { t, lang, toggle } = useI18n();
  const [view, setView] = useState<View>({ name: "list" });
  const [profiles, setProfiles] = useState<ProfileView[]>([]);
  const [status, setStatus] = useState<Status | null>(null);
  const [listError, setListError] = useState<string | null>(null);
  const [toast, setToast] = useState<string | null>(null);
  const [launchingProfile, setLaunchingProfile] = useState<ProfileView | null>(null);
  const toastTimer = useRef<number | null>(null);

  const refresh = useCallback(async () => {
    try {
      setProfiles(await api.listProfiles());
      setListError(null);
    } catch (e) {
      setListError(errorMessage(e));
    }
    try {
      setStatus(await api.getStatus());
    } catch {
      // Keep the previous status if the call fails; the footer tolerates null.
    }
  }, []);

  // Force a fresh CLI check (bypasses the backend's 30s status cache) —
  // used after the user installs a missing CLI.
  const recheckStatus = useCallback(async () => {
    try {
      setStatus(await api.getStatus(true));
    } catch {
      // Keep the previous status on failure.
    }
  }, []);

  // Initial load + file-watcher sync: every "profiles://changed" event
  // (debounced ~400 ms backend-side) re-fetches list + status.
  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    listen("profiles://changed", () => {
      void refresh();
    })
      .then((u) => {
        if (cancelled) {
          u();
          return;
        }
        unlisten = u;
      })
      .catch(() => {
        // Event listener unavailable (e.g. running outside Tauri); the
        // manual refresh on actions still keeps the UI consistent.
      });
    void refresh();
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [refresh]);

  useEffect(() => {
    return () => {
      if (toastTimer.current !== null) window.clearTimeout(toastTimer.current);
    };
  }, []);

  function showToast(message: string) {
    if (toastTimer.current !== null) window.clearTimeout(toastTimer.current);
    setToast(message);
    toastTimer.current = window.setTimeout(() => setToast(null), 2500);
  }

  async function handleDelete(p: ProfileView) {
    if (!window.confirm(t("confirmDelete", { name: p.name }))) {
      return;
    }
    try {
      await api.deleteProfile(p.id);
      showToast(t("toastDeleted", { name: p.name }));
      void refresh();
    } catch (e) {
      showToast(t("toastDeleteFailed", { err: errorMessage(e) }));
    }
  }

  async function handleLogin(p: ProfileView) {
    try {
      const r = await api.loginProfile(p.id);
      showToast(r.warning ? `⚠ ${r.warning}` : t("loginStarted"));
    } catch (e) {
      showToast(t("loginFail", { err: errorMessage(e) }));
    }
  }

  const goBack = () => setView({ name: "list" });

  return (
    <div className="app">
      <header className="topbar">
        <button
          type="button"
          className="brand"
          onClick={goBack}
          title={t("backToList")}
        >
          Agent Switch
        </button>
        <span className="topbar-sub">{t("topbarSub")}</span>
        <nav className="topbar-nav">
          <button
            type="button"
            className={
              view.name === "sessions" ||
              view.name === "settings" ||
              view.name === "about"
                ? ""
                : "active"
            }
            onClick={() => setView({ name: "list" })}
          >
            {t("navProfiles")}
          </button>
          <button
            type="button"
            className={view.name === "sessions" ? "active" : ""}
            onClick={() => setView({ name: "sessions" })}
          >
            {t("navSessions")}
          </button>
          <button
            type="button"
            className={view.name === "settings" ? "active" : ""}
            onClick={() => setView({ name: "settings" })}
          >
            {t("navSettings")}
          </button>
          <button
            type="button"
            className={view.name === "about" ? "active" : ""}
            onClick={() => setView({ name: "about" })}
          >
            {t("navAbout")}
          </button>
        </nav>
        <div className="topbar-right">
          <button
            type="button"
            className="btn secondary small lang-toggle"
            onClick={toggle}
            title={t("langToggleTitle")}
          >
            {lang === "zh" ? "EN" : "中文"}
          </button>
        </div>
      </header>

      <main className="content">
        {view.name === "list" && (
          <ListView
            profiles={profiles}
            error={listError}
            onAdd={() => setView({ name: "editor", id: null })}
            onEdit={(id) => setView({ name: "editor", id })}
            onLaunch={(p) => setLaunchingProfile(p)}
            onLogin={handleLogin}
            onDelete={handleDelete}
          />
        )}

        {view.name === "editor" && (
          <Editor
            id={view.id}
            onBack={goBack}
            onSaved={() => {
              showToast(t("toastSaved"));
              goBack();
              void refresh();
            }}
          />
        )}

        {view.name === "sessions" && <SessionsPage status={status} />}

        {view.name === "settings" && <SettingsPage />}

        {view.name === "about" && (
          <AboutPage status={status} onRecheck={() => void recheckStatus()} />
        )}
      </main>

      {launchingProfile && (
        <LaunchModal
          profile={launchingProfile}
          cliFound={
            launchingProfile.cli === "claude"
              ? status?.claude_found ?? false
              : status?.codex_found ?? false
          }
          onRecheck={() => void recheckStatus()}
          onClose={() => setLaunchingProfile(null)}
          onLaunched={(r) => {
            const started =
              launchingProfile.cli === "claude"
                ? t("launchStartedClaude")
                : t("launchStartedCodex");
            setLaunchingProfile(null);
            showToast(r.warning ? `⚠ ${r.warning}` : started);
          }}
        />
      )}

      {toast && <div className="toast">{toast}</div>}

      <footer className="statusbar">
        {status === null ? (
          <span className="muted">{t("statusLoading")}</span>
        ) : (
          <>
            <span className={status.codex_found ? "ok-text" : "fail-text"}>
              Codex {status.codex_found ? "✓" : "✗"}
              {status.codex_version ? ` ${status.codex_version}` : ""}
            </span>
            <span className={status.claude_found ? "ok-text" : "fail-text"}>
              Claude {status.claude_found ? "✓" : "✗"}
              {status.claude_version ? ` ${status.claude_version}` : ""}
            </span>
            <span className="muted statusbar-dir">{status.config_dir}</span>
            <span>{t("profilesCount", { n: status.profiles_count })}</span>
          </>
        )}
      </footer>
    </div>
  );
}

interface ListViewProps {
  profiles: ProfileView[];
  error: string | null;
  onAdd: () => void;
  onEdit: (id: string) => void;
  onLaunch: (p: ProfileView) => void;
  onLogin: (p: ProfileView) => void;
  onDelete: (p: ProfileView) => void;
}

function ListView({
  profiles,
  error,
  onAdd,
  onEdit,
  onLaunch,
  onLogin,
  onDelete,
}: ListViewProps) {
  const { t } = useI18n();
  return (
    <div className="list-page">
      <div className="list-head">
        <h2>{t("profilesTitle")}</h2>
        <button type="button" className="btn primary" onClick={onAdd}>
          {t("addBtn")}
        </button>
      </div>

      {error && <p className="error-text">{t("listError", { err: error })}</p>}

      {profiles.length === 0 && !error && (
        <div className="empty-state">
          <p>{t("emptyState")}</p>
          <p className="hint">{t("emptyHint")}</p>
        </div>
      )}

      <div className="card-grid">
        {profiles.map((p) => (
          <div key={p.id} className="card">
            <div className="card-head">
              <h3>{p.name}</h3>
              <span className={`cli-chip ${p.cli === "claude" ? "claude" : "codex"}`}>
                {p.cli === "claude" ? "Claude" : "Codex"}
              </span>
            </div>
            <dl className="card-meta">
              <dt>{t("cardModel")}</dt>
              <dd className="mono">{p.model}</dd>
              <dt>{t("cardBaseUrl")}</dt>
              <dd className="mono break-all">{p.base_url}</dd>
              <dt>{t("cardProvider")}</dt>
              <dd>{typeLabel(p, t)}</dd>
              <dt>{t("cardApiKey")}</dt>
              <dd className={p.has_api_key ? "ok-text" : "fail-text"}>
                {p.has_api_key ? t("keyConfigured") : t("keyMissing")}
              </dd>
            </dl>
            <div className="card-actions">
              <button
                type="button"
                className="btn primary small"
                onClick={() => onLaunch(p)}
              >
                {t("launchBtn")}
              </button>
              {p.provider_type === "official" && (
                <button
                  type="button"
                  className="btn secondary small"
                  onClick={() => onLogin(p)}
                  title={t("loginHint")}
                >
                  {t("loginBtn")}
                </button>
              )}
              <button
                type="button"
                className="btn secondary small"
                onClick={() => onEdit(p.id)}
              >
                {t("editBtn")}
              </button>
              <button
                type="button"
                className="btn danger small"
                onClick={() => onDelete(p)}
              >
                {t("deleteBtn")}
              </button>
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}
