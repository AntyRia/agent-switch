// Agent Switch — root component.
// State-based view switching (list / editor / launch), no router library.
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
import Launch from "./Launch";
import SessionsPage from "./Sessions";
import { I18nProvider, useI18n, type TKey } from "./i18n";

type View =
  | { name: "list" }
  | { name: "editor"; id: string | null } // null = new profile
  | { name: "launch"; preselect: string | null }
  | { name: "sessions" }
  | { name: "about" };

export default function App() {
  return (
    <I18nProvider>
      <AppInner />
    </I18nProvider>
  );
}

/** Display label for a provider type (legacy values pass through raw). */
function typeLabel(
  p: ProfileView,
  t: (key: TKey, vars?: Record<string, string | number>) => string,
): string {
  const v = p.provider_type;
  if (v === "relay") return t("typeRelay");
  if (v === "vllm") return t("typeVllm");
  return v;
}

function AppInner() {
  const { t, lang, toggle } = useI18n();
  const [view, setView] = useState<View>({ name: "list" });
  const [profiles, setProfiles] = useState<ProfileView[]>([]);
  const [status, setStatus] = useState<Status | null>(null);
  const [listError, setListError] = useState<string | null>(null);
  const [toast, setToast] = useState<string | null>(null);
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
    if (!window.confirm(t("confirmDelete", { id: p.id }))) {
      return;
    }
    try {
      await api.deleteProfile(p.id);
      showToast(t("toastDeleted", { id: p.id }));
      void refresh();
    } catch (e) {
      showToast(t("toastDeleteFailed", { err: errorMessage(e) }));
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
              view.name === "sessions" || view.name === "about" ? "" : "active"
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
            onLaunch={(id) => setView({ name: "launch", preselect: id })}
            onDelete={handleDelete}
          />
        )}

        {view.name === "editor" && (
          <Editor
            id={view.id}
            existingIds={profiles.map((p) => p.id)}
            onBack={goBack}
            onSaved={() => {
              showToast(t("toastSaved"));
              goBack();
              void refresh();
            }}
          />
        )}

        {view.name === "launch" && (
          <Launch
            profiles={profiles}
            preselect={view.preselect}
            onBack={goBack}
          />
        )}

        {view.name === "sessions" && <SessionsPage />}

        {view.name === "about" && (
          <AboutPage status={status} onRecheck={() => void refresh()} />
        )}
      </main>

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
  onLaunch: (id: string) => void;
  onDelete: (p: ProfileView) => void;
}

function ListView({
  profiles,
  error,
  onAdd,
  onEdit,
  onLaunch,
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
              <span className="badge mono">{p.id}</span>
            </div>
            {p.description && <p className="card-desc">{p.description}</p>}
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
                onClick={() => onLaunch(p.id)}
              >
                {t("launchBtn")}
              </button>
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
