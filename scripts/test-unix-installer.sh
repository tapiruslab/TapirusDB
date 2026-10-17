#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
test_dir="$(mktemp -d "${TMPDIR:-/tmp}/tapirus-install-test.XXXXXXXX")"
trap 'rm -rf -- "$test_dir"' EXIT
mkdir -p "$test_dir/payload"
cp "$root/target/debug/tapirus" "$test_dir/payload/tapirus"
archive="$test_dir/release.tar.gz"
tar -czf "$archive" -C "$test_dir/payload" .
if command -v sha256sum >/dev/null 2>&1; then digest="$(sha256sum "$archive" | awk '{print $1}')"; else digest="$(shasum -a 256 "$archive" | awk '{print $1}')"; fi
printf '%s  release.tar.gz\n' "$digest" > "$archive.sha256"
TAPIRUS_INSTALL_DIR="$test_dir/installed" TAPIRUS_ARCHIVE_PATH="$archive" TAPIRUS_CHECKSUM_PATH="$archive.sha256" bash "$root/install.sh"
test -x "$test_dir/installed/bin/tapirus"
printf '%064d  release.tar.gz\n' 0 > "$archive.sha256"
if TAPIRUS_INSTALL_DIR="$test_dir/tampered" TAPIRUS_ARCHIVE_PATH="$archive" TAPIRUS_CHECKSUM_PATH="$archive.sha256" bash "$root/install.sh"; then echo 'Invalid checksum was accepted' >&2; exit 1; fi
test ! -e "$test_dir/tampered/bin/tapirus"
echo 'Unix installer: isolated archive install and checksum rejection passed.'
