"""Reject mixed engine/SDK versions and tags before packaging or publishing."""
import argparse
import json
import os
import re
import tomllib
from pathlib import Path

root = Path(__file__).resolve().parents[1]
manifest = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))
expected = manifest["package"]["version"]
normalize_license = lambda text: '\n'.join(line.rstrip() for line in text.splitlines()).strip()
core_license = normalize_license((root / 'LICENSE').read_text(encoding='utf-8'))
for member in ['crates/tapirus-ffi', 'crates/tapirus-micro', 'crates/tapirus-wasm']:
    if normalize_license((root / member / 'LICENSE').read_text(encoding='utf-8')) != core_license:
        raise SystemExit(f'{member}: packaged core license differs from root LICENSE')
parser = argparse.ArgumentParser()
parser.add_argument("--tag")
args = parser.parse_args()
for member in manifest["workspace"]["members"]:
    package = tomllib.loads((root / member / "Cargo.toml").read_text(encoding="utf-8"))["package"]
    if package["version"] != expected:
        raise SystemExit(f'{member}: version {package["version"]} does not match engine {expected}')
    if package.get("rust-version") != manifest["package"]["rust-version"]:
        raise SystemExit(f'{member}: Rust minimum version differs from the engine')
    dependency = tomllib.loads((root / member / "Cargo.toml").read_text(encoding="utf-8")).get("dependencies", {}).get("tapirus")
    if isinstance(dependency, dict) and "path" in dependency and dependency.get("version") != expected:
        raise SystemExit(f'{member}: native dependency version does not match engine {expected}')
    for name, dependency in tomllib.loads((root / member / "Cargo.toml").read_text(encoding="utf-8")).get("dependencies", {}).items():
        if isinstance(dependency, dict) and "path" in dependency and dependency.get("version") != expected:
            raise SystemExit(f'{member}: internal dependency {name} does not match engine {expected}')
for member in ["sdks/nodejs", "packages/npm", "crates/tapirus-wasm"]:
    package = json.loads((root / member / "package.json").read_text(encoding="utf-8"))
    if package["version"] != expected:
        raise SystemExit(f'{member}: npm version does not match engine {expected}')
python_project = tomllib.loads((root / "sdks/python/pyproject.toml").read_text(encoding="utf-8"))["project"]
if python_project["version"] != expected:
    raise SystemExit('sdks/python: wheel version does not match engine')
legacy_python = tomllib.loads((root / "python/pyproject.toml").read_text(encoding="utf-8"))["project"]
legacy_module = (root / "python/tapirus/__init__.py").read_text(encoding="utf-8")
legacy_version = re.search(r'__version__\s*=\s*"([^"]+)"', legacy_module)
if legacy_python["version"] != expected or not legacy_version or legacy_version[1] != expected:
    raise SystemExit('python: legacy package metadata does not match engine')
if legacy_python["name"] == python_project["name"]:
    raise SystemExit('python: legacy distribution must not replace the native SDK on PyPI')
native_python = tomllib.loads((root / "sdks/python/Cargo.toml").read_text(encoding="utf-8"))
ffi = tomllib.loads((root / "crates/tapirus-ffi/Cargo.toml").read_text(encoding="utf-8"))
if native_python["lib"]["name"] == ffi["lib"]["name"]:
    raise SystemExit('Python extension and C ABI must not overwrite the same native library')
wasm = json.loads((root / "crates/tapirus-wasm/package.json").read_text(encoding="utf-8"))
if wasm["name"] != "@tapirus/wasm" or not wasm.get("private"):
    raise SystemExit('WASM source manifest must be private; publish the generated @tapirus/wasm pkg instead')
for directory in ["examples/typescript", "examples/cloudflare-worker"]:
    example = json.loads((root / directory / "package.json").read_text(encoding="utf-8"))
    if example["version"] != expected or not example.get("private"):
        raise SystemExit(f'{directory}: example must match engine version and remain private')
typescript = json.loads((root / "examples/typescript/package.json").read_text(encoding="utf-8"))
if typescript["dependencies"].get("tapirus") != "file:../../sdks/nodejs":
    raise SystemExit('TypeScript example must use the authoritative native SDK')
for file, pattern in [("sdks/go/tapirus.go", r'TapirusDB-Go/([0-9.]+)'), ("sdks/php/src/Client.php", r'TapirusDB-PHP/([0-9.]+)')]:
    match = re.search(pattern, (root / file).read_text(encoding="utf-8"))
    if not match or match[1] != expected:
        raise SystemExit(f'{file}: HTTP client version does not match engine {expected}')
lock = tomllib.loads((root / "Cargo.lock").read_text(encoding="utf-8"))
members = {tomllib.loads((root / member / "Cargo.toml").read_text(encoding="utf-8"))["package"]["name"] for member in manifest["workspace"]["members"]}
for package in lock["package"]:
    if package["name"] in members and "source" not in package and package["version"] != expected:
        raise SystemExit(f'Cargo.lock: {package["name"]} version does not match engine')
for file, pattern in [("install.ps1", r"\$Version = '([^']+)'"), ("install.sh", r"TAPIRUS_VERSION:-([^}]+)")]:
    match = re.search(pattern, (root / file).read_text(encoding="utf-8"))
    if not match or match[1] != expected:
        raise SystemExit(f'{file}: installer default does not match engine {expected}')
reference = os.environ.get("GITHUB_REF", "")
tag = args.tag or (reference.removeprefix("refs/tags/") if reference.startswith("refs/tags/") else None)
if tag and tag != f"v{expected}":
    raise SystemExit(f'Release tag {tag} does not match v{expected}')
print(f'Engine, workspace SDKs, npm packages and installer defaults agree on {expected}')
