// Launch page: pick a profile + workspace, open the profile's CLI
// (Codex or Claude, per the profile's `cli` field) in a new terminal.

import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { api, errorMessage, type LaunchResult, type ProfileView } from "./api";
import { useI18n } from "./i18n";

interface LaunchProps {
  profiles: ProfileView[];
  /** Profile to preselect (set when arriving from a card's Launch button). */
  preselect: string | null;
  onBack: () => void;
}

export default function Launch({ profiles, preselect, onBack }: LaunchProps) {
  const { t } = useI18n();
  const [selectedId, setSelectedId] = useState<string>(
    preselect ?? profiles[0]?.id ?? "",
  );
  const [workspace, setWorkspace] = useState("");
  const [launching, setLaunching] = useState(false);
  const [result, setResult] = useState<LaunchResult | null>(null);
  const [error, setError] = useState<string | null>(null);

  // Keep the selection valid if the profile list arrives after mount or
  // shrinks (e.g. a profile was deleted externally).
  useEffect(() => {
    if (profiles.length === 0) return;
    if (!profiles.some((p) => p.id === selectedId)) {
      const fallback =
        preselect && profiles.some((p) => p.id === preselect)
          ? preselect
          : profiles[0].id;
      setSelectedId(fallback);
    }
  }, [profiles, selectedId, preselect]);

  const selected = profiles.find((p) => p.id === selectedId) ?? null;
  const isClaude = selected?.cli === "claude";

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
    if (!selectedId || launching) return;
    setLaunching(true);
    setError(null);
    setResult(null);
    try {
      const ws = workspace.trim();
      const r = await api.launchProfile(selectedId, ws === "" ? null : ws);
      setResult(r);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setLaunching(false);
    }
  }

  return (
    <div className="panel launch">
      <div className="editor-head">
        <h2>
          {isClaude ? t("launchTitleClaude") : t("launchTitleCodex")}
        </h2>
      </div>

      {profiles.length === 0 ? (
        <p className="hint">{t("launchNoProfiles")}</p>
      ) : (
        <>
          <div className="field-grid">
            <label className="field">
              <span>{t("fProfile")}</span>
              <select
                value={selectedId}
                onChange={(e) => setSelectedId(e.target.value)}
              >
                {profiles.map((p) => (
                  <option key={p.id} value={p.id}>
                    {p.name} ({p.id}) — {p.cli}
                  </option>
                ))}
              </select>
            </label>

            <label className="field">
              <span>{t("fWorkspace")}</span>
              <div className="key-row">
                <input
                  type="text"
                  value={workspace}
                  readOnly
                  placeholder={t("wsPh")}
                />
                <button type="button" className="btn secondary small" onClick={pickDir}>
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
          </div>

          <div className="button-row">
            <button
              type="button"
              className="btn primary"
              onClick={runLaunch}
              disabled={launching || !selectedId}
            >
              {launching
                ? t("launching")
                : isClaude
                  ? t("launchBtnClaude")
                  : t("launchBtnCodex")}
            </button>
          </div>
        </>
      )}

      {error && <p className="error-text">{t("launchFailed", { err: error })}</p>}

      {result && (
        <div className="result-panel">
          <h3>{t("launchPrepared")}</h3>
          <dl>
            <dt>runtime_id</dt>
            <dd className="mono">{result.runtime_id}</dd>
            <dt>script_path</dt>
            <dd className="mono break-all">{result.script_path}</dd>
          </dl>
          {result.warning && (
            <p className="warning-box">⚠ {result.warning}</p>
          )}
          <p className="hint">
            {isClaude ? t("launchStartedClaude") : t("launchStartedCodex")}
          </p>
        </div>
      )}

      <div className="button-row">
        <button type="button" className="btn secondary" onClick={onBack}>
          {t("backToList")}
        </button>
      </div>
    </div>
  );
}
