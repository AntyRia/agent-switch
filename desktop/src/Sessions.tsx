// Sessions pool — resumable conversations found in the per-profile
// isolated runtimes. Each row carries chips (which CLI, which channel),
// a preview of the first user message, the original workspace, a
// relative last-active time, and a one-click Resume.

import { useCallback, useEffect, useState } from "react";
import { api, errorMessage, type Session } from "./api";
import { useI18n, type TKey } from "./i18n";

/** "relay"/"vllm" → localized chip label; legacy values pass through raw. */
function providerLabel(
  v: string,
  t: (key: TKey, vars?: Record<string, string | number>) => string,
): string {
  if (v === "relay") return t("typeRelay");
  if (v === "vllm") return t("typeVllm");
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

interface ResumeResult {
  sessionId: string;
  ok: boolean;
  text: string;
}

export default function SessionsPage() {
  const { t } = useI18n();
  const [sessions, setSessions] = useState<Session[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [result, setResult] = useState<ResumeResult | null>(null);

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

  async function resume(s: Session) {
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

  return (
    <div className="list-page">
      <div className="list-head">
        <h2>{t("sessionsTitle")}</h2>
        <button type="button" className="btn secondary" onClick={() => void load()}>
          {t("sessionRefresh")}
        </button>
      </div>

      {error && <p className="error-text">{error}</p>}

      {sessions !== null && sessions.length === 0 && !error && (
        <div className="empty-state">
          <p>{t("sessionsEmpty")}</p>
          <p className="hint">{t("sessionsEmptyHint")}</p>
        </div>
      )}

      <div className="session-list">
        {sessions?.map((s) => (
          <div key={s.session_id} className="session-row">
            <div className="session-main">
              <div className="session-chips">
                <span className={`cli-chip ${s.engine === "claude" ? "claude" : "codex"}`}>
                  {s.engine === "claude" ? "Claude" : "Codex"}
                </span>
                <span className="badge">
                  {s.profile_name}
                  {s.provider_type ? ` · ${providerLabel(s.provider_type, t)}` : ""}
                </span>
              </div>
              <p className="session-preview">{s.preview || "…"}</p>
              {s.cwd && (
                <p className="session-cwd mono">
                  <span className="muted">{t("sessionCwd")} </span>
                  {s.cwd}
                </p>
              )}
            </div>
            <div className="session-side">
              <span className="muted">{relativeTime(s.modified, t)}</span>
              {s.resumable ? (
                <button
                  type="button"
                  className="btn primary small"
                  disabled={busy === s.session_id}
                  onClick={() => void resume(s)}
                >
                  {busy === s.session_id ? t("resuming") : t("sessionResume")}
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
    </div>
  );
}
