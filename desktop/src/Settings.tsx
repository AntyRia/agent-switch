// Settings page: the global launch settings (settings.toml, shared with
// the CLI) — dangerous mode, proxy, terminal — plus a read-only tail of
// the app log for troubleshooting. Changes apply to EVERY launch (new
// conversations and session resumes alike).

import { useCallback, useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import {
  api,
  errorMessage,
  type LogsResult,
  type SettingsData,
  type TerminalInfo,
} from "./api";
import { useI18n } from "./i18n";

/** How many log lines the viewer shows (backend default is the same). */
const LOG_LINES = 200;

export default function SettingsPage() {
  const { t, lang } = useI18n();
  const [data, setData] = useState<SettingsData | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  // Working copy of the settings; applied only on Save.
  const [dangerous, setDangerous] = useState(false);
  const [proxyHost, setProxyHost] = useState("");
  const [proxyPort, setProxyPort] = useState("");
  const [terminalMode, setTerminalMode] = useState<"auto" | "custom">("auto");
  const [terminalPath, setTerminalPath] = useState("");
  const [terminals, setTerminals] = useState<TerminalInfo[]>([]);
  const [saving, setSaving] = useState(false);
  const [saveOk, setSaveOk] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [logs, setLogs] = useState<LogsResult | null>(null);
  const [logError, setLogError] = useState<string | null>(null);

  const loadLogs = useCallback(async () => {
    try {
      setLogs(await api.getLogs(LOG_LINES));
      setLogError(null);
    } catch (e) {
      setLogError(errorMessage(e));
    }
  }, []);

  useEffect(() => {
    let cancelled = false;
    Promise.all([api.getSettings(), api.detectTerminals()])
      .then(([s, terms]) => {
        if (cancelled) return;
        setData(s);
        setDangerous(s.dangerous_mode);
        setProxyHost(s.proxy_host);
        setProxyPort(s.proxy_port !== 0 ? String(s.proxy_port) : "");
        setTerminalMode(s.terminal ? "custom" : "auto");
        setTerminalPath(s.terminal);
        setTerminals(terms);
      })
      .catch((e) => {
        if (!cancelled) setLoadError(errorMessage(e));
      });
    void loadLogs();
    return () => {
      cancelled = true;
    };
  }, [loadLogs]);

  // Native file picker for the terminal executable. Outside Tauri (e.g.
  // a plain browser tab) the promise rejects — manual entry still works.
  async function pickTerminal() {
    try {
      const picked = await open({
        directory: false,
        multiple: false,
        title: t("pickTerminal"),
      });
      if (typeof picked === "string" && picked !== "") {
        setTerminalPath(picked);
        setTerminalMode("custom");
      }
    } catch {
      // Dialog unavailable: the user can type the path instead.
    }
  }

  async function runSave() {
    setSaving(true);
    setSaveError(null);
    setSaveOk(false);
    const host = proxyHost.trim();
    let port = 0;
    if (host !== "") {
      // A proxy without a usable port would be silently inert — refuse it.
      if (!/^\d+$/.test(proxyPort.trim()) || Number(proxyPort.trim()) > 65535) {
        setSaveError(t("proxyPortInvalid"));
        setSaving(false);
        return;
      }
      port = Number(proxyPort.trim());
    }
    const terminal = terminalMode === "custom" ? terminalPath.trim() : "";
    try {
      await api.saveSettings({
        dangerous_mode: dangerous,
        proxy_host: host,
        proxy_port: port,
        terminal,
      });
      setSaveOk(true);
    } catch (e) {
      setSaveError(t("settingsSaveFail", { err: errorMessage(e) }));
    } finally {
      setSaving(false);
    }
  }

  if (loadError) {
    return (
      <div className="panel">
        <p className="error-text">{t("settingsLoadFail", { err: loadError })}</p>
      </div>
    );
  }

  return (
    <div className="list-page settings-page">
      <div className="list-head">
        <h2>{t("settingsTitle")}</h2>
      </div>

      <section className="panel settings-section">
        <h3>{t("settingsRunTitle")}</h3>
        <label className="check-row">
          <input
            type="checkbox"
            checked={dangerous}
            onChange={(e) => setDangerous(e.target.checked)}
          />
          <span>{t("fDangerous")}</span>
        </label>
        <p className="hint">{t("dangerousHint")}</p>

        <div className="field-grid">
          <label className="field">
            <span>{t("fProxyHost")}</span>
            <input
              type="text"
              value={proxyHost}
              placeholder={t("proxyHostPh")}
              onChange={(e) => setProxyHost(e.target.value)}
            />
          </label>
          <label className="field">
            <span>{t("fProxyPort")}</span>
            <input
              type="text"
              inputMode="numeric"
              value={proxyPort}
              placeholder={t("proxyPh")}
              onChange={(e) => setProxyPort(e.target.value)}
            />
          </label>
        </div>
        <p className="hint">{t("proxyHint")}</p>
      </section>

      <section className="panel settings-section">
        <h3>{t("settingsTerminalTitle")}</h3>
        <div className="radio-row">
          <label>
            <input
              type="radio"
              name="terminal-mode"
              checked={terminalMode === "auto"}
              onChange={() => setTerminalMode("auto")}
            />
            <span>{t("terminalAuto")}</span>
          </label>
          <label>
            <input
              type="radio"
              name="terminal-mode"
              checked={terminalMode === "custom"}
              onChange={() => setTerminalMode("custom")}
            />
            <span>{t("terminalCustom")}</span>
          </label>
        </div>
        {terminalMode === "auto" ? (
          terminals.length > 0 ? (
            <p className="hint">
              {t("terminalAutoHint", {
                list: terminals
                  .map((x) => x.label)
                  .join(lang === "zh" ? "、" : ", "),
              })}
            </p>
          ) : (
            <p className="hint">{t("noTerminals")}</p>
          )
        ) : (
          <div className="key-row">
            <input
              type="text"
              className="mono"
              value={terminalPath}
              placeholder={t("terminalPh")}
              onChange={(e) => setTerminalPath(e.target.value)}
            />
            <button
              type="button"
              className="btn secondary small"
              onClick={() => void pickTerminal()}
            >
              {t("pickTerminal")}
            </button>
          </div>
        )}
        <p className="hint">{t("terminalHint")}</p>
      </section>

      <section className="panel settings-section">
        <div className="settings-log-head">
          <h3>{t("settingsLogTitle")}</h3>
          <button
            type="button"
            className="btn secondary small"
            onClick={() => void loadLogs()}
          >
            {t("logRefresh")}
          </button>
        </div>
        {logError && (
          <p className="error-text">
            {t("settingsLogLoadFail", { err: logError })}
          </p>
        )}
        {logs && (
          <>
            <p className="hint">
              {t("logPathLabel")}：{" "}
              <code className="mono break-all">{logs.path ?? "—"}</code>
            </p>
            <pre className="log-box">
              {logs.lines.length > 0
                ? logs.lines.join("\n")
                : t("logEmpty")}
            </pre>
          </>
        )}
      </section>

      {saveError && <p className="error-text">{saveError}</p>}
      {saveOk && !saveError && (
        <p className="ok-text">{t("settingsSaved")}</p>
      )}

      <div className="button-row">
        <button
          type="button"
          className="btn primary"
          onClick={() => void runSave()}
          disabled={saving || data === null}
        >
          {saving ? t("settingsSaving") : t("settingsSave")}
        </button>
      </div>

      {data && (
        <p className="hint">
          {t("logPathLabel")}: <code className="mono break-all">{data.path}</code>
        </p>
      )}
    </div>
  );
}
