// Sessions pool — resumable conversations found in the per-profile
// isolated runtimes. Each row carries chips (which CLI, which channel),
// a title (custom when set, otherwise the first user message), the
// original workspace, a relative last-active time, and actions:
// pin / set title / delete / one-click Resume.
// The pool is search-by-title, pinned-first, and paginated client-side.

import { useCallback, useEffect, useState } from "react";
import { api, errorMessage, type Session, type Status } from "./api";
import { useI18n, type TKey } from "./i18n";

/** Rows per page of the pool. */
const PAGE_SIZE = 10;

/** "relay" → localized chip label; "vllm" (and the legacy
 *  "openai"/"openai-compatible" values served before a restart) map to the
 *  OpenAI-compatible label. Unknown values pass through raw. */
function providerLabel(
  v: string,
  t: (key: TKey, vars?: Record<string, string | number>) => string,
): string {
  if (v === "relay") return t("typeRelay");
  if (v === "official") return t("typeOfficial");
  if (v === "vllm" || v === "openai" || v === "openai-compatible") {
    return t("typeOpenaiComp");
  }
  return v || "—";
}

/** unix seconds → "just now" / "N min ago" / …, dates for anything older. */
function relativeTime(
  modified: number,
  t: (key: TKey, vars?: Record<string, string | number>) => string,
): string {
  const secs = Math.max(0, Math.floor(Date.now() / 1000) - modified);
  if (secs < 60) return t("relJustNow");
  const mins = Math.floor(secs / 60);
  if (mins < 60) return t("relMinAgo", { n: mins });
  const hours = Math.floor(mins / 60);
  if (hours < 24) return t("relHourAgo", { n: hours });
  const days = Math.floor(hours / 24);
  if (days < 30) return t("relDayAgo", { n: days });
  return new Date(modified * 1000).toISOString().slice(0, 10);
}

interface ActionResult {
  sessionId: string;
  ok: boolean;
  text: string;
}

interface SessionsPageProps {
  /** CLI availability from get_status; a session whose engine's CLI is
   *  missing cannot be resumed. */
  status: Status | null;
}

export default function SessionsPage({ status }: SessionsPageProps) {
  const { t } = useI18n();
  const [sessions, setSessions] = useState<Session[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [result, setResult] = useState<ActionResult | null>(null);
  // Search box: matches the row's title (custom title, or the preview
  // when no custom title is set) and the session id.
  const [search, setSearch] = useState("");
  const [page, setPage] = useState(1);
  // Inline title editing for one row at a time.
  const [editingId, setEditingId] = useState<string | null>(null);
  const [titleDraft, setTitleDraft] = useState("");
  // Bulk "clear unpinned" action: busy flag + its result banner.
  const [clearing, setClearing] = useState(false);
  const [clearMsg, setClearMsg] = useState<{ ok: boolean; text: string } | null>(
    null,
  );

  const load = useCallback(async () => {
    try {
      setSessions(await api.listSessions());
      setError(null);
    } catch (e) {
      setError(errorMessage(e));
    }
  }, []);

  // Live refresh: poll while the page is open so conversations started in
  // a launched terminal show up without a manual refresh.
  useEffect(() => {
    void load();
    const timer = window.setInterval(() => void load(), 4000);
    return () => window.clearInterval(timer);
  }, [load]);

  /** True when the session's engine CLI is not installed. */
  function cliMissing(engine: string): boolean {
    if (!status) return false; // status still loading: let the backend decide
    return engine === "claude" ? !status.claude_found : !status.codex_found;
  }

  async function resume(s: Session) {
    // A session that is still running in a terminal must be closed first —
    // the backend enforces this too (belt and suspenders).
    if (s.open) {
      setResult({
        sessionId: s.session_id,
        ok: false,
        text: t("sessionAlreadyOpen"),
      });
      return;
    }
    if (cliMissing(s.engine)) {
      setResult({
        sessionId: s.session_id,
        ok: false,
        text: t("launchCliMissing", {
          cli: s.engine === "claude" ? "Claude" : "Codex",
        }),
      });
      return;
    }
    setBusy(s.session_id);
    setResult(null);
    try {
      await api.resumeSession(s.session_id);
      setResult({ sessionId: s.session_id, ok: true, text: t("sessionResumed") });
    } catch (e) {
      setResult({
        sessionId: s.session_id,
        ok: false,
        text: t("sessionResumeFail", { err: errorMessage(e) }),
      });
    } finally {
      setBusy(null);
    }
  }

  async function togglePin(s: Session) {
    setBusy(s.session_id);
    try {
      await api.setSessionPinned(s.session_id, !s.pinned);
      await load();
    } catch (e) {
      setResult({
        sessionId: s.session_id,
        ok: false,
        text: t("pinFail", { err: errorMessage(e) }),
      });
    } finally {
      setBusy(null);
    }
  }

  function startRename(s: Session) {
    setEditingId(s.session_id);
    setTitleDraft(s.title ?? "");
  }

  async function saveTitle(s: Session) {
    setBusy(s.session_id);
    try {
      await api.setSessionTitle(s.session_id, titleDraft);
      setEditingId(null);
      await load();
    } catch (e) {
      setResult({
        sessionId: s.session_id,
        ok: false,
        text: t("titleFail", { err: errorMessage(e) }),
      });
    } finally {
      setBusy(null);
    }
  }

  async function remove(s: Session) {
    if (!window.confirm(t("confirmDeleteSession", { id: s.session_id }))) return;
    setBusy(s.session_id);
    try {
      await api.deleteSession(s.session_id);
      // The row disappears on reload — that is the success feedback.
      setEditingId(null);
      await load();
    } catch (e) {
      setResult({
        sessionId: s.session_id,
        ok: false,
        text: t("sessionDeleteFail", { err: errorMessage(e) }),
      });
    } finally {
      setBusy(null);
    }
  }

  /** One-click cleanup: delete every session that is neither pinned nor
   *  open. The count shown in the confirmation is the pool-wide number of
   *  deletable rows (not the filtered view). */
  async function clearUnpinned() {
    const deletable = (sessions ?? []).filter((s) => !s.pinned && !s.open).length;
    if (deletable === 0) {
      setClearMsg({ ok: false, text: t("clearUnpinnedNone") });
      return;
    }
    if (!window.confirm(t("confirmClearUnpinned", { n: deletable }))) return;
    setClearing(true);
    setClearMsg(null);
    try {
      const n = await api.clearUnpinnedSessions();
      setClearMsg({
        ok: true,
        text: n > 0 ? t("clearUnpinnedDone", { n }) : t("clearUnpinnedNone"),
      });
      await load();
    } catch (e) {
      setClearMsg({
        ok: false,
        text: t("clearUnpinnedFail", { err: errorMessage(e) }),
      });
    } finally {
      setClearing(false);
    }
  }

  const q = search.trim().toLowerCase();
  const all = sessions ?? [];
  const filtered = all.filter((s) => {
    if (!q) return true;
    const label = (s.title ?? s.preview).toLowerCase();
    return label.includes(q) || s.session_id.toLowerCase().includes(q);
  });
  // Pinned rows float to the top; within each group, newest first.
  const sorted = [...filtered].sort(
    (a, b) => Number(b.pinned) - Number(a.pinned) || b.modified - a.modified,
  );
  const pages = Math.max(1, Math.ceil(sorted.length / PAGE_SIZE));
  const clampedPage = Math.min(page, pages);
  const visible = sorted.slice(
    (clampedPage - 1) * PAGE_SIZE,
    clampedPage * PAGE_SIZE,
  );

  return (
    <div className="list-page">
      <div className="list-head">
        <h2>{t("sessionsTitle")}</h2>
        <div className="list-tools">
          <input
            type="search"
            className="search-input"
            placeholder={t("sessionSearchPh")}
            value={search}
            onChange={(e) => {
              setSearch(e.target.value);
              setPage(1);
            }}
          />
          <button type="button" className="btn secondary" onClick={() => void load()}>
            {t("sessionRefresh")}
          </button>
          <button
            type="button"
            className="btn danger"
            disabled={clearing || sessions === null || sessions.length === 0}
            onClick={() => void clearUnpinned()}
          >
            {clearing ? t("clearingUnpinned") : t("clearUnpinnedBtn")}
          </button>
        </div>
      </div>

      {error && <p className="error-text">{error}</p>}

      {clearMsg && (
        <p className={clearMsg.ok ? "ok-text" : "error-text"}>{clearMsg.text}</p>
      )}

      {sessions !== null && q && (
        <p className="hint">
          {t("sessionsFiltered", { a: filtered.length, b: all.length })}
        </p>
      )}

      {sessions !== null && all.length === 0 && !error && (
        <div className="empty-state">
          <p>{t("sessionsEmpty")}</p>
          <p className="hint">{t("sessionsEmptyHint")}</p>
        </div>
      )}

      {sessions !== null && all.length > 0 && visible.length === 0 && (
        <div className="empty-state">
          <p>{t("noSearchResults", { q: search.trim() })}</p>
        </div>
      )}

      <div className="session-list">
        {visible.map((s) => (
          <div
            key={s.session_id}
            className={`session-row${s.pinned ? " pinned" : ""}`}
          >
            <div className="session-main">
              <div className="session-chips">
                <span className={`cli-chip ${s.engine === "claude" ? "claude" : "codex"}`}>
                  {s.engine === "claude" ? "Claude" : "Codex"}
                </span>
                <span className="badge">
                  {s.profile_name}
                  {s.provider_type ? ` · ${providerLabel(s.provider_type, t)}` : ""}
                </span>
                {s.open && (
                  <span className="open-chip" title={t("sessionAlreadyOpen")}>
                    {t("sessionOpen")}
                  </span>
                )}
              </div>
              {editingId === s.session_id ? (
                <div className="session-title-edit">
                  <input
                    autoFocus
                    value={titleDraft}
                    placeholder={t("sessionTitlePh")}
                    onChange={(e) => setTitleDraft(e.target.value)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter") void saveTitle(s);
                      if (e.key === "Escape") setEditingId(null);
                    }}
                  />
                  <button
                    type="button"
                    className="btn primary small"
                    disabled={busy === s.session_id}
                    onClick={() => void saveTitle(s)}
                  >
                    {t("save")}
                  </button>
                  <button
                    type="button"
                    className="btn secondary small"
                    onClick={() => setEditingId(null)}
                  >
                    {t("cancel")}
                  </button>
                </div>
              ) : (
                <p className="session-title">{s.title || s.preview || "…"}</p>
              )}
              {s.cwd && (
                <p className="session-cwd mono">
                  <span className="muted">{t("sessionCwd")} </span>
                  {s.cwd}
                </p>
              )}
            </div>
            <div className="session-side">
              <span className="muted">{relativeTime(s.modified, t)}</span>
              <div className="session-actions">
                <button
                  type="button"
                  className={`icon-btn${s.pinned ? " active" : ""}`}
                  title={s.pinned ? t("sessionUnpin") : t("sessionPin")}
                  disabled={busy === s.session_id}
                  onClick={() => void togglePin(s)}
                >
                  📌
                </button>
                <button
                  type="button"
                  className="icon-btn"
                  title={t("sessionRename")}
                  onClick={() => startRename(s)}
                >
                  ✎
                </button>
                <button
                  type="button"
                  className="icon-btn danger"
                  title={t("sessionDelete")}
                  disabled={busy === s.session_id}
                  onClick={() => void remove(s)}
                >
                  🗑
                </button>
              </div>
              {s.resumable ? (
                <button
                  type="button"
                  className="btn primary small"
                  disabled={
                    busy === s.session_id || s.open || cliMissing(s.engine)
                  }
                  title={
                    cliMissing(s.engine)
                      ? t("launchCliMissing", {
                          cli: s.engine === "claude" ? "Claude" : "Codex",
                        })
                      : undefined
                  }
                  onClick={() => void resume(s)}
                >
                  {s.open
                    ? t("sessionOpen")
                    : busy === s.session_id
                      ? t("resuming")
                      : t("sessionResume")}
                </button>
              ) : (
                <span className="muted">{t("sessionNoProfile")}</span>
              )}
            </div>
            {result && result.sessionId === s.session_id && (
              <p className={result.ok ? "ok-text" : "error-text"}>{result.text}</p>
            )}
          </div>
        ))}
      </div>

      {sessions !== null && all.length > 0 && pages > 1 && (
        <div className="pager">
          <button
            type="button"
            className="btn secondary small"
            disabled={clampedPage <= 1}
            onClick={() => setPage(clampedPage - 1)}
          >
            {t("prevPage")}
          </button>
          <span className="muted">
            {t("pageOf", { a: clampedPage, b: pages })}
          </span>
          <button
            type="button"
            className="btn secondary small"
            disabled={clampedPage >= pages}
            onClick={() => setPage(clampedPage + 1)}
          >
            {t("nextPage")}
          </button>
        </div>
      )}
    </div>
  );
}
