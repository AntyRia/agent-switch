// Launch modal: a lightweight dialog over the profile list. The profile
// is fixed (whichever card's Launch button was clicked); the user only
// picks the workspace folder (native picker) and confirms.

import { useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { api, errorMessage, type LaunchResult, type ProfileView } from "./api";
import { useI18n } from "./i18n";

interface LaunchModalProps {
  /** The profile whose card's Launch button was clicked. */
  profile: ProfileView;
  /** Whether the profile's CLI binary is installed (from get_status). */
  cliFound: boolean;
  /** Re-runs the CLI checks (unlocks the button after an install). */
  onRecheck: () => void;
  /** Called once the terminal launch was prepared (the modal closes). */
  onLaunched: (r: LaunchResult) => void;
  onClose: () => void;
}

export default function LaunchModal({
  profile,
  cliFound,
  onRecheck,
  onLaunched,
  onClose,
}: LaunchModalProps) {
  const { t } = useI18n();
  const [workspace, setWorkspace] = useState("");
  const [launching, setLaunching] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const isClaude = profile.cli === "claude";

  // Native folder picker (Tauri dialog plugin) — the workspace field is
  // filled by selection, not typed by hand.
  async function pickDir() {
    try {
      const dir = await open({
        directory: true,
        multiple: false,
        defaultPath: workspace.trim() || undefined,
      });
      if (typeof dir === "string") setWorkspace(dir);
    } catch (e) {
      setError(errorMessage(e));
    }
  }

  async function runLaunch() {
    if (launching || !cliFound) return;
    setLaunching(true);
    setError(null);
    try {
      const ws = workspace.trim();
      const r = await api.launchProfile(profile.id, ws === "" ? null : ws);
      onLaunched(r);
    } catch (e) {
      setError(errorMessage(e));
      setLaunching(false);
    }
  }

  return (
    <div className="modal-overlay" onClick={onClose}>
      <div className="modal" onClick={(e) => e.stopPropagation()}>
        <h3>{isClaude ? t("launchTitleClaude") : t("launchTitleCodex")}</h3>
        <p className="hint launch-profile-line">
          {profile.name} <span className="mono muted">({profile.id})</span>
        </p>

        <label className="field">
          <span>{t("fWorkspace")}</span>
          <div className="key-row">
            <input
              type="text"
              value={workspace}
              readOnly
              placeholder={t("wsPh")}
            />
            <button
              type="button"
              className="btn secondary small"
              onClick={() => void pickDir()}
            >
              {t("pickDir")}
            </button>
          </div>
          {workspace !== "" && (
            <span className="hint">
              <button
                type="button"
                className="btn link"
                onClick={() => setWorkspace("")}
              >
                {t("clearWs")}
              </button>
            </span>
          )}
        </label>

        {!cliFound && (
          <div className="cli-missing-box">
            <p className="fail-text">
              {t("launchCliMissing", { cli: isClaude ? "Claude" : "Codex" })}
            </p>
            <pre className="install-cmd">
              {isClaude
                ? "npm install -g @anthropic-ai/claude-code"
                : "npm install -g @openai/codex"}
            </pre>
            <button
              type="button"
              className="btn secondary small"
              onClick={onRecheck}
            >
              {t("aboutRecheck")}
            </button>
          </div>
        )}

        {error && <p className="error-text">{t("launchFailed", { err: error })}</p>}

        <div className="button-row">
          <button
            type="button"
            className="btn secondary"
            onClick={onClose}
            disabled={launching}
          >
            {t("cancel")}
          </button>
          <button
            type="button"
            className="btn primary"
            title={!cliFound ? t("launchCliMissing", { cli: isClaude ? "Claude" : "Codex" }) : undefined}
            onClick={() => void runLaunch()}
            disabled={launching || !cliFound}
          >
            {launching
              ? t("launching")
              : isClaude
                ? t("launchBtnClaude")
                : t("launchBtnCodex")}
          </button>
        </div>
      </div>
    </div>
  );
}
