#!/usr/bin/env sh
# Installs the agent-switch CLI from the latest GitHub release (macOS/Linux).
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

API="https://api.github.com/repos/${REPO}/releases/latest"
echo "Querying latest release of ${REPO} ..."
JSON="$(curl -fsSL -H 'User-Agent: agent-switch-install' "$API")"
TAG="$(printf '%s\n' "$JSON" | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -n1)"
VERSION="${TAG#v}"
ASSET="agent-switch-${VERSION}-${TARGET}.zip"
# Keep case patterns out of command substitution: macOS /bin/sh can misparse
# them when this script is read from a pipe, even when `sh -n` succeeds.
URL="$(printf '%s\n' "$JSON" | sed -n 's/.*"browser_download_url": *"\([^"]*\)".*/\1/p' | awk -v asset="$ASSET" '
  length($0) >= length(asset) + 1 && substr($0, length($0) - length(asset)) == "/" asset { print; exit }
')"
if [ -z "${TAG:-}" ] || [ -z "${URL:-}" ]; then
  echo "Asset '${ASSET}' not found in release '${TAG:-unknown}'. Check the release assets or build from source." >&2
  exit 1
fi

BIN_DIR="${AGENT_SWITCH_INSTALL_DIR:-${HOME}/.local/bin}"
mkdir -p "$BIN_DIR"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT INT TERM

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
