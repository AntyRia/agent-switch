// Settings page: the global launch settings (settings.toml, shared with
// the CLI) — dangerous mode, proxy, terminal, session pool location —
// plus a read-only tail of the app log. Each item saves itself (保存修改):
// text fields show a flat display state until clicked (the edit state
// carries the accent border + glow), and every item carries a status chip
// so the user can see at a glance whether it is active and healthy.

import { useCallback, useEffect, useState, type ReactNode } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import {
  api,
  errorMessage,
  type LogsResult,
  type PoolMigrationResult,
  type PoolStatus,
  type SettingsData,
  type SettingsDraft,
  type TerminalInfo,
} from "./api";
import { useI18n } from "./i18n";

/** How many log lines the viewer shows (backend default is the same). */
const LOG_LINES = 200;

type ChipTone = "ok" | "warn" | "err" | "muted";
type ItemKey = "dangerous" | "proxy" | "terminal" | "pool";
interface ItemState {
  busy: boolean;
  ok: boolean;
  error: string | null;
}

function StatusChip({ tone, text }: { tone: ChipTone; text: string }) {
  return <span className={`status-chip ${tone}`}>{text}</span>;
}

/** A text field with a clear non-edit (flat, dashed) and edit (accent
 *  border + glow, autofocused) state. Enter commits, Esc cancels. The
 *  value lives in the parent (the item's working copy). */
function EditableField(props: {
  label: string;
  value: string;
  placeholder: string;
  mono?: boolean;
  hint?: string;
  status?: { tone: ChipTone; text: string };
  /** A trailing action (e.g. the browse button) in both states. */
  trailing?: ReactNode;
  onValue: (v: string) => void;
  onEnter: () => void;
  onEsc: () => void;
}) {
  const { t } = useI18n();
  const [editing, setEditing] = useState(false);
  const start = () => setEditing(true);
  const cancel = () => {
    props.onEsc();
    setEditing(false);
  };
  return (
    <div className="field">
      <span className="field-label-row">
        <span>{props.label}</span>
        {props.status && (
          <StatusChip tone={props.status.tone} text={props.status.text} />
        )}
      </span>
      {editing ? (
        <div className="key-row">
          <input
            autoFocus
            type="text"
            className={`editing${props.mono ? " mono" : ""}`}
            value={props.value}
            placeholder={props.placeholder}
            onChange={(e) => props.onValue(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") props.onEnter();
              else if (e.key === "Escape") cancel();
            }}
          />
          {props.trailing}
          <button
            type="button"
            className="btn secondary small"
            onClick={cancel}
          >
            {t("cancel")}
          </button>
        </div>
      ) : (
        <div
          className="field-display"
          role="button"
          tabIndex={0}
          onClick={start}
          onKeyDown={(e) => e.key === "Enter" && start()}
        >
          <span className={props.mono ? "mono field-display-value" : "field-display-value"}>
            {props.value === "" ? <em>{props.placeholder}</em> : props.value}
          </span>
          {props.trailing}
          <span className="edit-icon" title={t("editHint")}>
            ✎
          </span>
        </div>
      )}
      {props.hint && <p className="hint">{props.hint}</p>}
    </div>
  );
}

/** The per-item 保存修改 button + its success flash / error line. */
function ItemActions(props: {
  label: string;
  busyLabel?: string;
  state: ItemState;
  disabled?: boolean;
  onSave: () => void;
}) {
  const { t } = useI18n();
  return (
    <>
      <div className="settings-item-actions">
        <button
          type="button"
          className="btn primary small"
          onClick={props.onSave}
          disabled={props.state.busy || props.disabled}
        >
          {props.state.busy
            ? (props.busyLabel ?? t("saving"))
            : props.state.ok
              ? t("savedFlash")
              : props.label}
        </button>
      </div>
      {props.state.error && (
        <p className="error-text">{props.state.error}</p>
      )}
    </>
  );
}

export default function SettingsPage() {
  const { t, lang } = useI18n();
  const [data, setData] = useState<SettingsData | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  // Working copies — an item is applied only by ITS OWN save button; the
  // other items' values come from the last-saved `data`.
  const [dangerous, setDangerous] = useState(false);
  const [proxyHost, setProxyHost] = useState("");
  const [proxyPort, setProxyPort] = useState("");
  const [terminalMode, setTerminalMode] = useState<"auto" | "custom">("auto");
  const [terminalPath, setTerminalPath] = useState("");
  const [terminalPathOk, setTerminalPathOk] = useState(true);
  const [poolPath, setPoolPath] = useState("");
  const [terminals, setTerminals] = useState<TerminalInfo[]>([]);
  const [poolStatus, setPoolStatus] = useState<PoolStatus | null>(null);
  const [poolReport, setPoolReport] = useState<PoolMigrationResult | null>(null);
  const [items, setItems] = useState<Record<ItemKey, ItemState>>({
    dangerous: { busy: false, ok: false, error: null },
    proxy: { busy: false, ok: false, error: null },
    terminal: { busy: false, ok: false, error: null },
    pool: { busy: false, ok: false, error: null },
  });
  const [logs, setLogs] = useState<LogsResult | null>(null);
  const [logError, setLogError] = useState<string | null>(null);

  const patchItem = useCallback(
    (key: ItemKey, patch: Partial<ItemState>) => {
      setItems((s) => ({ ...s, [key]: { ...s[key], ...patch } }));
    },
    []
  );

  const loadLogs = useCallback(async () => {
    try {
      setLogs(await api.getLogs(LOG_LINES));
      setLogError(null);
    } catch (e) {
      setLogError(errorMessage(e));
    }
  }, []);

  const refreshPool = useCallback(async () => {
    try {
      setPoolStatus(await api.getPoolStatus());
    } catch {
      // Non-fatal: the chips simply stay as they were.
    }
  }, []);

  useEffect(() => {
    let cancelled = false;
    Promise.all([api.getSettings(), api.detectTerminals(), api.getPoolStatus()])
      .then(([s, terms, pool]) => {
        if (cancelled) return;
        setData(s);
        setDangerous(s.dangerous_mode);
        setProxyHost(s.proxy_host);
        setProxyPort(s.proxy_port !== 0 ? String(s.proxy_port) : "");
        setTerminalMode(s.terminal ? "custom" : "auto");
        setTerminalPath(s.terminal);
        setPoolPath(s.pool_dir);
        setTerminals(terms);
        setPoolStatus(pool);
        if (s.terminal !== "") {
          api
            .pathExists(s.terminal)
            .then((ok) => {
              if (!cancelled) setTerminalPathOk(ok);
            })
            .catch(() => undefined);
        } else {
          setTerminalPathOk(true);
        }
      })
      .catch((e) => {
        if (!cancelled) setLoadError(errorMessage(e));
      });
    void loadLogs();
    return () => {
      cancelled = true;
    };
  }, [loadLogs]);

  /** A full draft from the last-saved values + this item's working copy. */
  const fullDraft = useCallback(
    (patch: Partial<SettingsDraft>): SettingsDraft => ({
      dangerous_mode: data?.dangerous_mode ?? false,
      proxy_host: data?.proxy_host ?? "",
      proxy_port: data?.proxy_port ?? 0,
      terminal: data?.terminal ?? "",
      pool_dir: data?.pool_dir ?? "",
      ...patch,
    }),
    [data]
  );

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
        patchItem("terminal", { ok: false, error: null });
      }
    } catch {
      // Dialog unavailable: the user can type the path instead.
    }
  }

  // Native folder picker for the pool location.
  async function pickPoolDir() {
    try {
      const picked = await open({
        directory: true,
        multiple: false,
        title: t("poolTitle"),
      });
      if (typeof picked === "string" && picked !== "") {
        setPoolPath(picked);
        patchItem("pool", { ok: false, error: null });
      }
    } catch {
      // Dialog unavailable: the user can type the path instead.
    }
  }

  async function saveDangerous() {
    patchItem("dangerous", { busy: true, ok: false, error: null });
    try {
      await api.saveSettings(fullDraft({ dangerous_mode: dangerous }));
      setData((d) => d && { ...d, dangerous_mode: dangerous });
      patchItem("dangerous", { busy: false, ok: true });
    } catch (e) {
      patchItem("dangerous", {
        busy: false,
        error: t("settingsSaveFail", { err: errorMessage(e) }),
      });
    }
  }

  async function saveProxy() {
    const host = proxyHost.trim();
    let port = 0;
    if (host !== "") {
      // A proxy without a usable port would be silently inert — refuse it.
      if (!/^\d+$/.test(proxyPort.trim()) || Number(proxyPort.trim()) > 65535) {
        patchItem("proxy", { ok: false, error: t("proxyPortInvalid") });
        return;
      }
      port = Number(proxyPort.trim());
    }
    patchItem("proxy", { busy: true, ok: false, error: null });
    try {
      await api.saveSettings(fullDraft({ proxy_host: host, proxy_port: port }));
      setData((d) => d && { ...d, proxy_host: host, proxy_port: port });
      setProxyHost(host);
      setProxyPort(port !== 0 ? String(port) : "");
      patchItem("proxy", { busy: false, ok: true });
    } catch (e) {
      patchItem("proxy", {
        busy: false,
        error: t("settingsSaveFail", { err: errorMessage(e) }),
      });
    }
  }

  async function saveTerminal() {
    const path = terminalMode === "custom" ? terminalPath.trim() : "";
    if (terminalMode === "custom" && path !== "") {
      // A missing pinned path would surprise the next launch — refuse it.
      const ok = await api.pathExists(path);
      setTerminalPathOk(ok);
      if (!ok) {
        patchItem("terminal", { ok: false, error: t("terminalStatusBad") });
        return;
      }
    }
    patchItem("terminal", { busy: true, ok: false, error: null });
    try {
      await api.saveSettings(fullDraft({ terminal: path }));
      setData((d) => d && { ...d, terminal: path });
      setTerminalPath(path);
      patchItem("terminal", { busy: false, ok: true });
    } catch (e) {
      patchItem("terminal", {
        busy: false,
        error: t("settingsSaveFail", { err: errorMessage(e) }),
      });
    }
  }

  /** Save the new pool location and migrate: the backend persists the
   *  path, stops the open agent-switch sessions, then moves the homes. */
  async function savePool() {
    const p = poolPath.trim();
    const current = data?.pool_dir ?? "";
    if (p === current) {
      patchItem("pool", { ok: false, error: t("poolNoChange") });
      return;
    }
    const defaultDir = poolStatus?.default_dir ?? "";
    const target = p === "" || (defaultDir !== "" && p.toLowerCase() === defaultDir.toLowerCase())
      ? defaultDir
      : p;
    if (target === "") {
      patchItem("pool", { ok: false, error: t("poolMigrateFail", { err: "—" }) });
      return;
    }
    const openSessions = poolStatus?.open_sessions ?? [];
    patchItem("pool", { busy: true, ok: false, error: null });
    setPoolReport(null);
    try {
      const listed = openSessions
        .slice(0, 5)
        .map((s) => `${s.session_id.slice(0, 8)} (${s.activity})`)
        .join(", ");
      const more =
        openSessions.length > 5
          ? (lang === "zh" ? ` … 还有 ${openSessions.length - 5} 个` : ` … and ${openSessions.length - 5} more`)
          : "";
      if (
        !window.confirm(
          t("poolMigrateConfirm", {
            path: target,
            n: openSessions.length,
            list: openSessions.length > 0 ? listed + more : "—",
          })
        )
      ) {
        patchItem("pool", { busy: false });
        return;
      }
      const result = await api.migratePoolTo(target);
      setPoolReport(result);
      const s = await api.getSettings();
      setData(s);
      setPoolPath(s.pool_dir);
      await refreshPool();
      if (result.failures.length > 0) {
        patchItem("pool", {
          busy: false,
          error: t("poolFailures", { n: result.failures.length }),
        });
      } else {
        patchItem("pool", { busy: false, ok: true });
      }
    } catch (e) {
      patchItem("pool", {
        busy: false,
        error: t("poolMigrateFail", { err: errorMessage(e) }),
      });
    }
  }

  if (loadError) {
    return (
      <div className="panel">
        <p className="error-text">{t("settingsLoadFail", { err: loadError })}</p>
      </div>
    );
  }

  const stoppedProcesses = poolReport
    ? poolReport.stopped.reduce((n, s) => n + s.processes.length, 0)
    : 0;
  const stoppedSessions = poolReport
    ? poolReport.stopped.filter((s) => s.session_id !== "").length
    : 0;

  const terminalStatus =
    terminalMode === "auto"
      ? { tone: "muted" as ChipTone, text: t("terminalStatusAuto", { n: terminals.length }) }
      : terminalPath === ""
        ? { tone: "muted" as ChipTone, text: t("terminalStatusBad") }
        : terminalPathOk
          ? { tone: "ok" as ChipTone, text: t("terminalStatusOk") }
          : { tone: "err" as ChipTone, text: t("terminalStatusBad") };

  const poolStatusChip: { tone: ChipTone; text: string } = poolStatus
    ? !poolStatus.exists
      ? { tone: "warn", text: t("poolStatusMissing") }
      : poolStatus.custom
        ? { tone: "ok", text: t("poolStatusCustom") }
        : { tone: "muted", text: t("poolStatusDefault") }
    : { tone: "muted", text: "…" };

  return (
    <div className="list-page settings-page">
      <div className="list-head">
        <h2>{t("settingsTitle")}</h2>
      </div>

      <section className="panel settings-section">
        <h3>{t("settingsRunTitle")}</h3>

        {/* Dangerous mode — its own save. */}
        <div className="settings-item">
          <label className="check-row settings-check-label">
            <input
              type="checkbox"
              checked={dangerous}
              onChange={(e) => {
                setDangerous(e.target.checked);
                patchItem("dangerous", { ok: false, error: null });
              }}
            />
            <span>{t("fDangerous")}</span>
            {data && (
              <StatusChip
                tone={data.dangerous_mode ? "ok" : "muted"}
                text={data.dangerous_mode ? t("statusOn") : t("statusOff")}
              />
            )}
          </label>
          <p className="hint">{t("dangerousHint")}</p>
          <ItemActions
            label={t("saveChanges")}
            state={items.dangerous}
            disabled={data === null}
            onSave={() => void saveDangerous()}
          />
        </div>

        {/* Proxy host + port — one shared save. */}
        <div className="settings-item">
          <EditableField
            label={t("fProxyHost")}
            value={proxyHost}
            placeholder={t("proxyHostPh")}
            status={
              data
                ? data.proxy_host !== ""
                  ? { tone: "ok", text: t("proxyStatusOn", { host: data.proxy_host, port: data.proxy_port }) }
                  : { tone: "muted", text: t("proxyStatusOff") }
                : undefined
            }
            onValue={(v) => {
              setProxyHost(v);
              patchItem("proxy", { ok: false, error: null });
            }}
            onEnter={() => void saveProxy()}
            onEsc={() => {
              setProxyHost(data?.proxy_host ?? "");
              setProxyPort(data && data.proxy_port !== 0 ? String(data.proxy_port) : "");
            }}
          />
          <EditableField
            label={t("fProxyPort")}
            value={proxyPort}
            placeholder={t("proxyPh")}
            onValue={(v) => {
              setProxyPort(v);
              patchItem("proxy", { ok: false, error: null });
            }}
            onEnter={() => void saveProxy()}
            onEsc={() =>
              setProxyPort(data && data.proxy_port !== 0 ? String(data.proxy_port) : "")
            }
          />
          <p className="hint">{t("proxyHint")}</p>
          <ItemActions
            label={t("saveChanges")}
            state={items.proxy}
            disabled={data === null}
            onSave={() => void saveProxy()}
          />
        </div>
      </section>

      <section className="panel settings-section">
        <h3>{t("settingsTerminalTitle")}</h3>
        <div className="radio-row">
          <label>
            <input
              type="radio"
              name="terminal-mode"
              checked={terminalMode === "auto"}
              onChange={() => {
                setTerminalMode("auto");
                patchItem("terminal", { ok: false, error: null });
              }}
            />
            <span>{t("terminalAuto")}</span>
          </label>
          <label>
            <input
              type="radio"
              name="terminal-mode"
              checked={terminalMode === "custom"}
              onChange={() => {
                setTerminalMode("custom");
                patchItem("terminal", { ok: false, error: null });
              }}
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
          <EditableField
            label={t("terminalCustom")}
            value={terminalPath}
            placeholder={t("terminalPh")}
            mono
            status={terminalStatus}
            hint={t("terminalHint")}
            trailing={
              <button
                type="button"
                className="btn secondary small"
                onClick={(e) => {
                  e.stopPropagation();
                  void pickTerminal();
                }}
              >
                {t("pickTerminal")}
              </button>
            }
            onValue={(v) => {
              setTerminalPath(v);
              patchItem("terminal", { ok: false, error: null });
            }}
            onEnter={() => void saveTerminal()}
            onEsc={() => {
              setTerminalPath(data?.terminal ?? "");
              setTerminalMode((data?.terminal ?? "") !== "" ? "custom" : "auto");
            }}
          />
        )}
        {terminalMode === "auto" && <p className="hint">{t("terminalHint")}</p>}
        <ItemActions
          label={t("saveChanges")}
          state={items.terminal}
          disabled={data === null}
          onSave={() => void saveTerminal()}
        />
      </section>

      <section className="panel settings-section">
        <h3>{t("poolTitle")}</h3>
        {poolStatus && (
          <div className="pool-current">
            <span className="pool-current-label">{t("poolCurrentLabel")}</span>
            <code className="mono break-all pool-path">{poolStatus.current_dir}</code>
            <StatusChip
              tone="muted"
              text={t("poolContents", { homes: poolStatus.entries, sessions: poolStatus.sessions })}
            />
            {poolStatus.legacy_entries > 0 && (
              <StatusChip tone="warn" text={t("poolLegacy", { n: poolStatus.legacy_entries })} />
            )}
            {poolStatus.open_sessions.length > 0 && (
              <StatusChip tone="warn" text={t("poolOpen", { n: poolStatus.open_sessions.length })} />
            )}
          </div>
        )}
        <div className="settings-item">
          <EditableField
            label={t("poolPathLabel")}
            value={poolPath}
            placeholder={poolStatus?.default_dir ?? t("poolStatusDefault")}
            mono
            status={poolStatusChip}
            hint={t("poolHint")}
            trailing={
              <button
                type="button"
                className="btn secondary small"
                onClick={(e) => {
                  e.stopPropagation();
                  void pickPoolDir();
                }}
              >
                {t("pickDir")}
              </button>
            }
            onValue={(v) => {
              setPoolPath(v);
              patchItem("pool", { ok: false, error: null });
            }}
            onEnter={() => void savePool()}
            onEsc={() => setPoolPath(data?.pool_dir ?? "")}
          />
          {poolReport && (
            <div className={`migrate-report${poolReport.failures.length > 0 ? " err" : ""}`}>
              <div>✓ {t("poolMigrated", { n: poolReport.moved.length })}</div>
              {stoppedProcesses > 0 && (
                <div>
                  {t("poolStopped", { n: stoppedProcesses, sessions: stoppedSessions })}
                </div>
              )}
              {poolReport.failures.map((f) => (
                <div key={f} className="mono break-all">
                  · {f}
                </div>
              ))}
            </div>
          )}
          <ItemActions
            label={t("poolSave")}
            busyLabel={t("poolMigrating")}
            state={items.pool}
            disabled={data === null}
            onSave={() => void savePool()}
          />
        </div>
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
              {t("logPathLabel")}:{" "}
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

      {data && (
        <p className="hint">
          {t("logPathLabel")}: <code className="mono break-all">{data.path}</code>
        </p>
      )}
    </div>
  );
}
