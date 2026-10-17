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
parser = argparse.ArgumentParser()
parser.add_argument("--tag")
args = parser.parse_args()
for member in manifest["workspace"]["members"]:
    package = tomllib.loads((root / member / "Cargo.toml").read_text(encoding="utf-8"))["package"]
    if package["version"] != expected:
        raise SystemExit(f'{member}: version {package["version"]} does not match engine {expected}')
for member in ["sdks/nodejs", "packages/npm"]:
    package = json.loads((root / member / "package.json").read_text(encoding="utf-8"))
    if package["version"] != expected:
        raise SystemExit(f'{member}: npm version does not match engine {expected}')
python_project = tomllib.loads((root / "sdks/python/pyproject.toml").read_text(encoding="utf-8"))["project"]
if python_project["version"] != expected:
    raise SystemExit('sdks/python: wheel version does not match engine')
for file, pattern in [("install.ps1", r"\$Version = '([^']+)'"), ("install.sh", r"TAPIRUS_VERSION:-([^}]+)")]:
    match = re.search(pattern, (root / file).read_text(encoding="utf-8"))
    if not match or match[1] != expected:
        raise SystemExit(f'{file}: installer default does not match engine {expected}')
reference = os.environ.get("GITHUB_REF", "")
tag = args.tag or (reference.removeprefix("refs/tags/") if reference.startswith("refs/tags/") else None)
if tag and tag != f"v{expected}":
    raise SystemExit(f'Release tag {tag} does not match v{expected}')
print(f'Engine, workspace SDKs, npm packages and installer defaults agree on {expected}')
