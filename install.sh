#!/usr/bin/env bash
set -euo pipefail

REPO="starc007/deadcheck"
BIN_NAME="deadcheck"
INSTALL_DIR="/usr/local/bin"

# ---------------------------------------------------------------------------
# Detect OS and architecture
# ---------------------------------------------------------------------------

OS="$(uname -s)"
ARCH="$(uname -m)"

case "$OS" in
  Linux)
    case "$ARCH" in
      x86_64) TARGET="x86_64-unknown-linux-gnu" ;;
      *)
        echo "Unsupported Linux architecture: $ARCH" >&2
        exit 1
        ;;
    esac
    ;;
  Darwin)
    case "$ARCH" in
      arm64)  TARGET="aarch64-apple-darwin" ;;
      x86_64) TARGET="x86_64-apple-darwin" ;;
      *)
        echo "Unsupported macOS architecture: $ARCH" >&2
        exit 1
        ;;
    esac
    ;;
  *)
    echo "Unsupported OS: $OS. For Windows, download the .zip from GitHub Releases." >&2
    exit 1
    ;;
esac

# ---------------------------------------------------------------------------
# Resolve the version to install
# ---------------------------------------------------------------------------

if [ -n "${DEADCHECK_VERSION:-}" ]; then
  VERSION="$DEADCHECK_VERSION"
else
  echo "Fetching latest release..."
  VERSION="$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
    | grep '"tag_name"' \
    | sed -E 's/.*"tag_name": *"([^"]+)".*/\1/')"
fi

if [ -z "$VERSION" ]; then
  echo "Could not determine latest version. Set DEADCHECK_VERSION to install a specific version." >&2
  exit 1
fi

echo "Installing deadcheck ${VERSION} for ${TARGET}..."

# ---------------------------------------------------------------------------
# Download and install
# ---------------------------------------------------------------------------

ARCHIVE="${BIN_NAME}-${TARGET}.tar.gz"
URL="https://github.com/${REPO}/releases/download/${VERSION}/${ARCHIVE}"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

echo "Downloading ${URL}..."
curl -fsSL "$URL" -o "${TMP}/${ARCHIVE}"

tar -xzf "${TMP}/${ARCHIVE}" -C "$TMP"

# Install to /usr/local/bin (may need sudo)
if [ -w "$INSTALL_DIR" ]; then
  mv "${TMP}/${BIN_NAME}" "${INSTALL_DIR}/${BIN_NAME}"
else
  echo "Needs sudo to write to ${INSTALL_DIR}..."
  sudo mv "${TMP}/${BIN_NAME}" "${INSTALL_DIR}/${BIN_NAME}"
fi

chmod +x "${INSTALL_DIR}/${BIN_NAME}"

echo ""
echo "deadcheck ${VERSION} installed to ${INSTALL_DIR}/${BIN_NAME}"
echo "Run: deadcheck --help"
