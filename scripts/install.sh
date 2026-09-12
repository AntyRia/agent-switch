#!/usr/bin/env sh
# Installs the newest published CLI package for this platform (macOS/Linux).
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/AntyRia/agent-switch/main/scripts/install.sh | sh
#
# Downloads the latest release asset named
#   agent-switch-<version>-<arch>-apple-darwin.zip   (macOS)
#   agent-switch-<version>-<arch>-unknown-linux-gnu.zip (Linux)
# and installs it to ~/.local/bin. Set AGENT_SWITCH_INSTALL_DIR to override.

set -eu

REPO="AntyRia/agent-switch"
OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
ARCH="$(uname -m)"
case "$ARCH" in
  arm64|aarch64) ARCH="aarch64" ;;
  x86_64|amd64) ARCH="x86_64" ;;
  *) echo "unsupported architecture: $ARCH" >&2; exit 1 ;;
esac
case "$OS" in
  darwin) TARGET="${ARCH}-apple-darwin" ;;
  linux)  TARGET="${ARCH}-unknown-linux-gnu" ;;
  *) echo "unsupported OS: $OS (need macOS or Linux)" >&2; exit 1 ;;
esac

# Node.js is also required for the npm-installed Codex / Claude CLIs.
if ! command -v node >/dev/null 2>&1; then
  echo "This installer requires Node.js. Install it from https://nodejs.org or download the CLI ZIP from https://github.com/${REPO}/releases." >&2
  exit 1
fi

API="https://api.github.com/repos/${REPO}/releases"
echo "Finding the latest ${TARGET} package from ${REPO} ..."
PAGE=1
URL=
while [ -z "$URL" ]; do
  RELEASES="$(curl -fsSL --connect-timeout 10 --max-time 60 -H 'User-Agent: agent-switch-install' "$API?per_page=100&page=$PAGE")"
  # JavaScript template literals below are evaluated by Node.js.
  # shellcheck disable=SC2016
  RESULT="$(printf '%s\n' "$RELEASES" | node -e '
    const releases = JSON.parse(require("fs").readFileSync(0, "utf8"));
    if (!Array.isArray(releases)) throw new Error("Expected a GitHub release list");
    for (const release of releases) {
      if (release.draft || release.prerelease) continue;
      const version = release.tag_name.replace(/^v/, "");
      const name = `agent-switch-${version}-${process.argv[1]}.zip`;
      const asset = release.assets.find(asset => asset.name === name);
      if (asset) { console.log(asset.browser_download_url); process.exit(0); }
    }
    console.log(releases.length ? "next-page" : "no-package");
  ' "$TARGET")"
  case "$RESULT" in
    next-page) PAGE=$((PAGE + 1)) ;;
    no-package) break ;;
    *) URL="$RESULT" ;;
  esac
done
if [ -z "$URL" ]; then
  echo "No published package for ${TARGET}. See https://github.com/${REPO}/releases or build from source." >&2
  exit 1
fi

BIN_DIR="${AGENT_SWITCH_INSTALL_DIR:-${HOME}/.local/bin}"
mkdir -p "$BIN_DIR"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT INT TERM

ASSET="${URL##*/}"
echo "Downloading ${ASSET} ..."
curl -fsSL -o "${TMP}/as.zip" "$URL"
unzip -oq "${TMP}/as.zip" -d "$TMP"
install -m 0755 "${TMP}/agent-switch" "${BIN_DIR}/agent-switch"

case ":${PATH}:" in
  *":${BIN_DIR}:"*) ;;
  *)
    # This script runs in a subshell, so it cannot change the PATH of the
    # shell that invoked it. Persist the export for future shells (best
    # effort, idempotent), and print the line for the current one.
    # Custom destinations may be temporary; leave shell configuration alone.
    if [ -z "${AGENT_SWITCH_INSTALL_DIR:-}" ]; then
      for rc in "${HOME}/.bashrc" "${HOME}/.zshrc" "${HOME}/.profile"; do
        if [ -f "$rc" ] && ! grep -qs "agent-switch PATH" "$rc"; then
          # Preserve $PATH for the shell that later sources this file.
          # shellcheck disable=SC2016
          { printf '\n# agent-switch PATH\nexport PATH="%s:$PATH"\n' "$BIN_DIR"; } >> "$rc" 2>/dev/null || true
        fi
      done
    fi
    echo "Note: ${BIN_DIR} is not on your PATH yet."
    echo "  current shell:  export PATH=\"${BIN_DIR}:\$PATH\""
    echo "  future shells:  ensure the export is present in your shell startup file."
    ;;
esac

"${BIN_DIR}/agent-switch" --version
echo "agent-switch installed: ${BIN_DIR}/agent-switch"
echo "Next: agent-switch add   (create your first profile)"
