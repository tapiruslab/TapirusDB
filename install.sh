#!/usr/bin/env bash
# Verified prebuilt installation; use TAPIRUS_FROM_SOURCE=1 to build a release tag.
set -euo pipefail
TAPIRUS_VERSION="${TAPIRUS_VERSION:-1.0.1}"
INSTALL_DIR="${TAPIRUS_INSTALL_DIR:-$HOME/.tapirus}"
[[ "$TAPIRUS_VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[A-Za-z0-9.-]+)?$ ]] || { echo 'Invalid release version' >&2; exit 1; }
work_dir="$(mktemp -d "${TMPDIR:-/tmp}/tapirus-install.XXXXXXXX")"
trap 'rm -rf -- "$work_dir"' EXIT
case "$(uname -s)" in Linux) platform=linux; extension=so;; Darwin) platform=macos; extension=dylib;; *) echo 'Use a supported Unix host or install.ps1 on Windows.' >&2; exit 1;; esac
case "$(uname -m)" in x86_64|amd64) architecture=x86_64;; aarch64|arm64) architecture=aarch64;; *) echo 'No prebuilt release for this architecture.' >&2; exit 1;; esac
if [ "${TAPIRUS_FROM_SOURCE:-0}" = 1 ]; then
    git clone --branch "v$TAPIRUS_VERSION" --depth 1 https://github.com/tapiruslab/TapirusDB.git "$work_dir/source"
    (cd "$work_dir/source" && cargo build --locked --release -p tapirus -p tapirus-ffi)
    payload="$work_dir/source/target/release"
    header="$work_dir/source/include/tapirus.h"
else
    name="tapirus-${platform}-${architecture}.tar.gz"
    archive="${TAPIRUS_ARCHIVE_PATH:-$work_dir/$name}"
    checksum="${TAPIRUS_CHECKSUM_PATH:-$archive.sha256}"
    if [ -z "${TAPIRUS_ARCHIVE_PATH:-}" ]; then
        base="https://github.com/tapiruslab/TapirusDB/releases/download/v$TAPIRUS_VERSION"
        curl -fL --retry 2 "$base/$name" -o "$archive"
        curl -fL --retry 2 "$base/$name.sha256" -o "$checksum"
    fi
    expected="$(awk -v file="$(basename "$archive")" '$2 == file || $2 == "*"file {print $1}' "$checksum")"
    [[ "$expected" =~ ^[a-fA-F0-9]{64}$ ]] || { echo 'Missing or invalid SHA-256 manifest entry.' >&2; exit 1; }
    if command -v sha256sum >/dev/null 2>&1; then actual="$(sha256sum "$archive" | awk '{print $1}')"; else actual="$(shasum -a 256 "$archive" | awk '{print $1}')"; fi
    [ "$(printf '%s' "$expected" | tr '[:upper:]' '[:lower:]')" = "$actual" ] || { echo 'Archive checksum mismatch. Installation stopped.' >&2; exit 1; }
    payload="$work_dir/payload"
    mkdir -p "$payload"
    if tar -tzf "$archive" | awk '/^\// || /(^|\/)\.\.(\/|$)/ {bad=1} END {exit !bad}'; then echo 'Archive contains an unsafe path.' >&2; exit 1; fi
    tar -xzf "$archive" -C "$payload"
    header="$payload/include/tapirus.h"
fi
[ -f "$payload/tapirus" ] || { echo 'Release archive is missing the executable.' >&2; exit 1; }
chmod +x "$payload/tapirus"
version_output="$("$payload/tapirus" --version)"
case "$version_output" in *"$TAPIRUS_VERSION"*) ;; *) echo 'Executable version does not match the requested release.' >&2; exit 1;; esac
mkdir -p "$INSTALL_DIR/bin" "$INSTALL_DIR/lib" "$INSTALL_DIR/include"
cp "$payload/tapirus" "$INSTALL_DIR/bin/tapirus"
if [ -f "$payload/libtapirus.$extension" ]; then cp "$payload/libtapirus.$extension" "$INSTALL_DIR/lib/"; fi
if [ -f "$header" ]; then cp "$header" "$INSTALL_DIR/include/tapirus.h"; fi
printf 'Installed TapirusDB v%s at %s/bin/tapirus\n' "$TAPIRUS_VERSION" "$INSTALL_DIR"
printf 'Add to your shell PATH: export PATH=%q:$PATH\n' "$INSTALL_DIR/bin"
