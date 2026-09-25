#!/usr/bin/env bash
# ==============================================================================
# TapirusDB Universal Installer for Linux, macOS, BSD, and ARM
# Architected by Ahmad Faiz • Tapirus Tech Lab (TapirusDB.com)
# ==============================================================================
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/tapiruslab/TapirusDB/main/install.sh | bash
# ==============================================================================

set -euo pipefail

TAPIRUS_VERSION="1.0.0"
REPO="tapiruslab/TapirusDB"
INSTALL_DIR="${TAPIRUS_INSTALL_DIR:-$HOME/.tapirus}"
BIN_DIR="$INSTALL_DIR/bin"
LIB_DIR="$INSTALL_DIR/lib"
INCLUDE_DIR="$INSTALL_DIR/include"

RED='\033[0;31m'
GREEN='\033[0;32m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m' # No Color

echo -e "${CYAN}${BOLD}"
cat << 'EOF'
  ___________           .__                     ________  __________
  \__    ___/____  ____ |__|______ __ __  ______\______ \ \______   \
    |    |  \__  \ \____ \|  \_  __ \  |  \/  ___/ |    |  \ |    |  _/
    |    |   / __ \|  |_> >  ||  | \/  |  /\___ \  |    `   \|    |   \
    |____|  (____  /   __/|__||__|  |____//____  >/_______  /|______  /
                 \/|__|                        \/         \/        \/
EOF
echo -e "${NC}"
echo -e "${BLUE}${BOLD}==> TapirusDB Universal Installer (v${TAPIRUS_VERSION})${NC}"
echo -e "The Safe-Rust Embedded Quad-Model AI Database Engine."
echo ""

# 1. Detect Operating System and Architecture
OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
ARCH="$(uname -m)"

case "$ARCH" in
    x86_64|amd64)
        TARGET_ARCH="x86_64"
        ;;
    aarch64|arm64)
        TARGET_ARCH="aarch64"
        ;;
    armv7l|armhf)
        TARGET_ARCH="armv7"
        ;;
    *)
        echo -e "${RED}Error: Unsupported architecture: $ARCH${NC}"
        exit 1
        ;;
esac

case "$OS" in
    linux*)
        PLATFORM="linux"
        LIB_EXT="so"
        ;;
    darwin*)
        PLATFORM="macos"
        LIB_EXT="dylib"
        ;;
    freebsd*|openbsd*)
        PLATFORM="bsd"
        LIB_EXT="so"
        ;;
    msys*|cygwin*|mingw*)
        PLATFORM="windows"
        LIB_EXT="dll"
        ;;
    *)
        echo -e "${RED}Error: Unsupported operating system: $OS${NC}"
        exit 1
        ;;
esac

echo -e "${GREEN}✓ Detected System:${NC} $PLATFORM ($TARGET_ARCH)"

# 2. Create Destination Directories
mkdir -p "$BIN_DIR" "$LIB_DIR" "$INCLUDE_DIR"

# 3. Build or Download TapirusDB
if command -v cargo >/dev/null 2>&1; then
    echo -e "${BLUE}==> Rust toolchain detected. Building optimized native binaries from source...${NC}"
    TMP_DIR="$(mktemp -d)"
    trap 'rm -rf "$TMP_DIR"' EXIT

    git clone --depth 1 "https://github.com/$REPO.git" "$TMP_DIR/tapirus" >/dev/null 2>&1 || {
        echo -e "${RED}Failed to clone repository. Check internet connection.${NC}"
        exit 1
    }

    cd "$TMP_DIR/tapirus"
    echo -e "${CYAN}--> Compiling tapirus CLI and C FFI shared library in release mode...${NC}"
    cargo build --release --workspace >/dev/null 2>&1

    # Copy artifacts
    cp target/release/tapirus "$BIN_DIR/tapirus"
    chmod +x "$BIN_DIR/tapirus"

    if [ "$PLATFORM" = "macos" ]; then
        [ -f target/release/libtapirus.dylib ] && cp target/release/libtapirus.dylib "$LIB_DIR/libtapirus.dylib"
        [ -f target/release/libtapirus.so ] && cp target/release/libtapirus.so "$LIB_DIR/libtapirus.so"
    else
        [ -f target/release/libtapirus.so ] && cp target/release/libtapirus.so "$LIB_DIR/libtapirus.so"
    fi

    cp include/tapirus.h "$INCLUDE_DIR/tapirus.h"
else
    echo -e "${BLUE}==> Fetching pre-compiled release for $PLATFORM-$TARGET_ARCH...${NC}"
    RELEASE_URL="https://github.com/$REPO/releases/download/v${TAPIRUS_VERSION}/tapirus-${PLATFORM}-${TARGET_ARCH}.tar.gz"
    
    TMP_TAR="$(mktemp)"
    if curl -fsSL "$RELEASE_URL" -o "$TMP_TAR" 2>/dev/null; then
        tar -xzf "$TMP_TAR" -C "$INSTALL_DIR"
        rm -f "$TMP_TAR"
    else
        echo -e "${RED}Pre-built binary not found on GitHub Release. Please install Rust (https://rustup.rs) and rerun this installer.${NC}"
        exit 1
    fi
fi

# 4. Configure Shell PATH
SHELL_NAME="$(basename "${SHELL:-bash}")"
RC_FILE=""

case "$SHELL_NAME" in
    zsh)
        RC_FILE="$HOME/.zshrc"
        ;;
    bash)
        if [ "$PLATFORM" = "macos" ]; then
            RC_FILE="$HOME/.bash_profile"
        else
            RC_FILE="$HOME/.bashrc"
        fi
        ;;
    fish)
        RC_FILE="$HOME/.config/fish/config.fish"
        ;;
    *)
        RC_FILE="$HOME/.profile"
        ;;
esac

EXPORT_LINE="export PATH=\"$BIN_DIR:\$PATH\""
EXPORT_LIB="export LD_LIBRARY_PATH=\"$LIB_DIR:\${LD_LIBRARY_PATH:-}\""

if [ -f "$RC_FILE" ]; then
    if ! grep -q "$BIN_DIR" "$RC_FILE"; then
        echo "" >> "$RC_FILE"
        echo "# TapirusDB" >> "$RC_FILE"
        echo "$EXPORT_LINE" >> "$RC_FILE"
        echo "$EXPORT_LIB" >> "$RC_FILE"
        echo -e "${GREEN}✓ Updated PATH in ${RC_FILE}${NC}"
    fi
fi

echo ""
echo -e "${GREEN}${BOLD}🎉 TapirusDB successfully installed!${NC}"
echo -e "Binary installed to:  ${BOLD}$BIN_DIR/tapirus${NC}"
echo -e "Shared library in:    ${BOLD}$LIB_DIR/libtapirus.${LIB_EXT}${NC}"
echo -e "C Header in:          ${BOLD}$INCLUDE_DIR/tapirus.h${NC}"
echo ""
echo -e "To start using TapirusDB immediately, run:"
echo -e "  ${CYAN}export PATH=\"$BIN_DIR:\$PATH\"${NC}"
echo -e "  ${CYAN}tapirus --help${NC}"
echo ""
