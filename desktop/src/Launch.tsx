// Launch modal: a lightweight dialog over the profile list. The profile
// is fixed (whichever card's Launch button was clicked); the user only
// picks the workspace folder (native picker) and confirms.

import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import {
  api,
  errorMessage,
  type LaunchResult,
  type ProfileView,
  type SettingsData,
} from "./api";
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
  /** Close the modal and open the Settings page (proxy not configured). */
  onGoSettings: () => void;
  onClose: () => void;
}

export default function LaunchModal({
  profile,
  cliFound,
  onRecheck,
  onLaunched,
  onGoSettings,
  onClose,
}: LaunchModalProps) {
  const { t } = useI18n();
  const [workspace, setWorkspace] = useState("");
  // Proxy choice for THIS launch: false (default) = direct connection,
  // true = the proxy configured in Settings (a configured proxy is
  // required — the launch button is held until one exists).
  const [useProxy, setUseProxy] = useState(false);
  const [settings, setSettings] = useState<SettingsData | null>(null);
  const [launching, setLaunching] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const isClaude = profile.cli === "claude";

  // The proxy hint/requirement needs the global proxy setting.
  useEffect(() => {
    api
      .getSettings()
      .then(setSettings)
      .catch(() => setSettings(null));
  }, []);

  const proxyConfigured =
    !!settings && settings.proxy_host.trim() !== "" && settings.proxy_port > 0;

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
    if (launching || !cliFound || (useProxy && !proxyConfigured)) return;
    setLaunching(true);
    setError(null);
    try {
      const ws = workspace.trim();
      const r = await api.launchProfile(
        profile.id,
        ws === "" ? null : ws,
        useProxy,
      );
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

        <div className="field">
          <span>{t("fConnMode")}</span>
          <div
            className="proxy-seg"
            role="group"
            title={t("connModeTip")}
          >
            <button
              type="button"
              className={useProxy ? "" : "on"}
              onClick={() => setUseProxy(false)}
            >
              {t("connDirect")}
            </button>
            <button
              type="button"
              className={useProxy ? "on" : ""}
              onClick={() => setUseProxy(true)}
            >
              {t("connProxy")}
            </button>
          </div>
          {useProxy
            ? proxyConfigured ? (
                <span className="proxy-mode-hint mono">
                  {t("proxyVia", {
                    host: settings!.proxy_host.trim(),
                    port: settings!.proxy_port,
                  })}
                </span>
              ) : (
                <div className="proxy-warn">
                  <span>{t("proxyNotConfigured")}</span>
                  <button
                    type="button"
                    className="btn secondary small"
                    onClick={onGoSettings}
                  >
                    {t("goSettings")}
                  </button>
                </div>
              )
            : <span className="proxy-mode-hint">{t("connDirectHint")}</span>}
        </div>

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
            title={
              !cliFound
                ? t("launchCliMissing", { cli: isClaude ? "Claude" : "Codex" })
                : useProxy && !proxyConfigured
                  ? t("proxyNotConfigured")
                  : undefined
            }
            onClick={() => void runLaunch()}
            disabled={launching || !cliFound || (useProxy && !proxyConfigured)}
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
