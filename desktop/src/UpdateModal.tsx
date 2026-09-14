// Built-in update dialog: confirm → download progress bar → restart.
// The backend (start_update) downloads, verifies and prepares the
// platform install while emitting "update-progress" events; on success
// the app exits and the detached platform installer swaps the app and
// relaunches it (profiles / sessions / settings are never touched).

import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  api,
  errorMessage,
  type UpdateInfo,
  type UpdateProgress,
} from "./api";
import { useI18n } from "./i18n";

type Phase =
  | { name: "confirm" }
  | { name: "running"; pct: number | null } // null = past download (verify/spawn)
  | { name: "done" }
  | { name: "error"; message: string };

interface UpdateModalProps {
  info: UpdateInfo;
  /** Dismissed via "Later" (the parent remembers this version). */
  onClose: () => void;
}

export default function UpdateModal({ info, onClose }: UpdateModalProps) {
  const { t } = useI18n();
  const [phase, setPhase] = useState<Phase>({ name: "confirm" });

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    listen<UpdateProgress>("update-progress", (e) => {
      const { done, total } = e.payload;
      setPhase(
        total > 0
          ? { name: "running", pct: Math.min(100, Math.round((done * 100) / total)) }
          : { name: "running", pct: null },
      );
    })
      .then((u) => {
        unlisten = u;
      })
      .catch(() => {
        // Outside the Tauri shell: no progress events; the promise result
        // still drives the state text.
      });
    return () => {
      unlisten?.();
    };
  }, []);

  // The backend returned: the platform installer owns the process now.
  // Brief "restarting" state, then exit — the installer relaunches the
  // new version.
  useEffect(() => {
    if (phase.name !== "done") return;
    const timer = window.setTimeout(() => {
      void api.exitApp().catch(() => {});
    }, 400);
    return () => window.clearTimeout(timer);
  }, [phase]);

  async function start() {
    setPhase({ name: "running", pct: 0 });
    try {
      await api.startUpdate();
      setPhase({ name: "done" });
    } catch (e) {
      setPhase({ name: "error", message: errorMessage(e) });
    }
  }

  const busy = phase.name === "running" || phase.name === "done";

  return (
    <div className="modal-overlay" role="dialog" aria-modal="true">
      <div className="modal update-modal">
        <h3>{t("updateModalTitle")}</h3>

        {phase.name === "confirm" && (
          <p className="hint">{t("updateModalBody", { latest: info.latest ?? "" })}</p>
        )}

        {phase.name === "running" && (
          <>
            <div
              className="progress"
              role="progressbar"
              aria-valuenow={phase.pct ?? 100}
              aria-valuemin={0}
              aria-valuemax={100}
            >
              <div className="progress-bar" style={{ width: `${phase.pct ?? 100}%` }} />
            </div>
            <p className="hint">
              {phase.pct === null
                ? t("updateFinishing")
                : t("updateDownloading", { pct: phase.pct })}
            </p>
          </>
        )}

        {phase.name === "done" && <p className="hint">{t("updateRestart")}</p>}

        {phase.name === "error" && (
          <>
            <p className="error-text">{t("updateFail", { err: phase.message })}</p>
            <div className="modal-actions">
              <button type="button" className="btn secondary" onClick={onClose}>
                {t("updateLater")}
              </button>
              <button type="button" className="btn primary" onClick={() => void start()}>
                {t("updateRetry")}
              </button>
            </div>
          </>
        )}

        {phase.name === "confirm" && (
          <div className="modal-actions">
            <button
              type="button"
              className="btn secondary"
              onClick={onClose}
              disabled={busy}
            >
              {t("updateLater")}
            </button>
            <button
              type="button"
              className="btn primary"
              onClick={() => void start()}
              disabled={busy}
            >
              {t("updateNow")}
            </button>
          </div>
        )}
      </div>
    </div>
  );
}
