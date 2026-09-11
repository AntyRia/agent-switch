// About page: app identity + a friendly health check of the two local
// CLIs (Codex / Claude) with install guidance, plus the data location.

import { openUrl } from "@tauri-apps/plugin-opener";
import { type Status } from "./api";
import { useI18n, type TKey } from "./i18n";

interface AboutProps {
  status: Status | null;
  /** Triggers the parent's list+status refresh (re-runs the CLI checks). */
  onRecheck: () => void;
}

interface CliCardProps {
  name: string;
  found: boolean;
  version: string | null;
  installCmd: string;
  t: (key: TKey, vars?: Record<string, string | number>) => string;
}

function CliCard({ name, found, version, installCmd, t }: CliCardProps) {
  return (
    <div className={`cli-status ${found ? "ok" : "fail"}`}>
      <div className="cli-status-head">
        <span className="cli-dot" aria-hidden="true" />
        <h3>{name}</h3>
      </div>
      {found ? (
        <p className="cli-status-state">
          <span className="ok-text">✓ {t("aboutInstalled")}</span>
          {version && <span className="mono"> · {version}</span>}
        </p>
      ) : (
        <>
          <p className="cli-status-state">
            <span className="fail-text">✗ {t("aboutMissing")}</span>
          </p>
          <p className="hint">{t("aboutInstallHint")}</p>
          <pre className="install-cmd">{installCmd}</pre>
        </>
      )}
    </div>
  );
}

/** Open an external URL in the system browser (opener plugin); a plain
 *  window.open keeps it working outside the Tauri shell. */
async function openExternal(url: string) {
  try {
    await openUrl(url);
  } catch {
    window.open(url, "_blank");
  }
}

export default function AboutPage({ status, onRecheck }: AboutProps) {
  const { t } = useI18n();
  return (
    <div className="about-page">
      <div className="about-hero panel">
        <img className="about-logo" src="/app-icon.png" alt="Agent Switch" />
        <div className="about-hero-text">
          <h2>
            Agent Switch
            {status && (
              <span className="about-version">v{status.version}</span>
            )}
          </h2>
          <p className="hint">{t("aboutDesc")}</p>
        </div>
      </div>

      <section className="about-section">
        <div className="about-section-head">
          <h2>{t("aboutCliTitle")}</h2>
          <button type="button" className="btn secondary small" onClick={onRecheck}>
            {t("aboutRecheck")}
          </button>
        </div>
        {status === null ? (
          <p className="hint">{t("statusLoading")}</p>
        ) : (
          <div className="cli-grid">
            <CliCard
              name={t("aboutCodexName")}
              found={status.codex_found}
              version={status.codex_version}
              installCmd="npm install -g @openai/codex"
              t={t}
            />
            <CliCard
              name={t("aboutClaudeName")}
              found={status.claude_found}
              version={status.claude_version}
              installCmd="npm install -g @anthropic-ai/claude-code"
              t={t}
            />
          </div>
        )}
      </section>

      <section className="about-section">
        <h2>{t("aboutDataTitle")}</h2>
        {status && (
          <p className="hint">
            {t("aboutDataDir")} <code className="break-all">{status.config_dir}</code>
          </p>
        )}
      </section>

      <section className="about-section">
        <h2>{t("aboutOssTitle")}</h2>
        <p className="hint">{t("aboutOssBody")}</p>
        <a
          className="thanks-row"
          href="https://github.com/AntyRia/agent-switch"
          onClick={(e) => {
            e.preventDefault();
            void openExternal("https://github.com/AntyRia/agent-switch");
          }}
        >
          <code className="mono">{t("aboutOssLink")}</code>
        </a>
      </section>

      <section className="about-section">
        <h2>{t("aboutThanksTitle")}</h2>
        <a
          className="thanks-row"
          href="https://hyperroute.cc/"
          onClick={(e) => {
            e.preventDefault();
            void openExternal("https://hyperroute.cc/");
          }}
        >
          <span>{t("aboutThanksHyperRoute")}</span>
          <span className="thanks-ext" aria-hidden="true">↗</span>
        </a>
        <a
          className="thanks-row"
          href="https://linux.do/"
          onClick={(e) => {
            e.preventDefault();
            void openExternal("https://linux.do/");
          }}
        >
          <span>{t("aboutThanksLdo")}</span>
          <span className="thanks-ext" aria-hidden="true">↗</span>
        </a>
      </section>
    </div>
  );
}
