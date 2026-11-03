"""Inventory SDK dependencies and query official registries without changing them.

Use --online for a dated freshness/publication report. A newer version is a
migration candidate, not proof of compatibility. Network failures are errors,
never a successful or empty freshness check. Python 3.11+; no dependencies.
"""
import argparse
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import re
import subprocess
import tomllib
from urllib.error import HTTPError
from urllib.parse import quote
from urllib.request import Request, urlopen

ROOT = Path(__file__).resolve().parents[1]


def read_toml(path):
    return tomllib.loads((ROOT / path).read_text(encoding="utf-8"))


def fetch(url):
    request = Request(url, headers={"User-Agent": "TapirusDB-stack-validation", "Accept": "application/json"})
    with urlopen(request, timeout=30) as response:
        return json.load(response)


def inventory():
    workspace = read_toml("Cargo.toml")
    lock = read_toml("Cargo.lock")
    locked = {}
    for package in lock["package"]:
        if package.get("source", "").startswith("registry+"):
            locked.setdefault(package["name"], set()).add(package["version"])
    entries = []
    for member in workspace["workspace"]["members"]:
        path = Path(member) / "Cargo.toml"
        manifest = read_toml(path)
        tables = [(key, manifest.get(key, {})) for key in ("dependencies", "dev-dependencies", "build-dependencies")]
        for target, config in manifest.get("target", {}).items():
            tables.extend((f"target.{target}.{key}", config.get(key, {})) for key in ("dependencies", "dev-dependencies", "build-dependencies"))
        for table, dependencies in tables:
            for alias, dependency in dependencies.items():
                if isinstance(dependency, dict) and "path" in dependency:
                    continue
                name = dependency.get("package", alias) if isinstance(dependency, dict) else alias
                requirement = dependency.get("version") if isinstance(dependency, dict) else dependency
                entries.append({"ecosystem": "crates.io", "name": name, "manifest": path.as_posix(), "section": table, "requirement": requirement, "locked": sorted(locked.get(name, []))})
    for directory in ("sdks/nodejs", "packages/npm", "crates/tapirus-wasm", "examples/typescript", "examples/cloudflare-worker"):
        path = f"{directory}/package.json"
        manifest = json.loads((ROOT / path).read_text(encoding="utf-8"))
        for section in ("dependencies", "devDependencies", "optionalDependencies"):
            for name, requirement in manifest.get(section, {}).items():
                if requirement.startswith(("file:", "../", "workspace:")):
                    continue
                entries.append({"ecosystem": "npm", "name": name, "manifest": path, "section": section, "requirement": requirement, "locked": []})
    project = read_toml("sdks/python/pyproject.toml")
    requirements = [("build-system", project["build-system"]["requires"])]
    requirements.extend((f"extra.{extra}", values) for extra, values in project["project"].get("optional-dependencies", {}).items())
    for section, values in requirements:
        for requirement in values:
            name = re.match(r"[A-Za-z0-9_.-]+", requirement)[0]
            entries.append({"ecosystem": "PyPI", "name": name, "manifest": "sdks/python/pyproject.toml", "section": section, "requirement": requirement[len(name):], "locked": []})
    return workspace["package"]["version"], entries


def latest(key):
    ecosystem, name = key
    base = {"ecosystem": ecosystem, "name": name}
    url = {"crates.io": f"https://crates.io/api/v1/crates/{quote(name, safe='')}", "npm": f"https://registry.npmjs.org/{quote(name, safe='')}/latest", "PyPI": f"https://pypi.org/pypi/{quote(name, safe='')}/json"}[ecosystem]
    try:
        data = fetch(url)
        if ecosystem == "crates.io":
            version = data["crate"]["max_stable_version"]
            metadata = next(v for v in data["versions"] if v["num"] == version)
            info = {"latest_stable": version, "rust_version": metadata.get("rust_version"), "released_at": metadata.get("created_at")}
        elif ecosystem == "npm":
            info = {"latest_stable": data["version"], "engines": data.get("engines", {})}
        else:
            info = {"latest_stable": data["info"]["version"], "requires_python": data["info"].get("requires_python")}
        return {**base, "status": "checked", "url": url, **info}
    except Exception as error:
        return {**base, "status": "error", "url": url, "error": str(error)}


def publication(label, url, extract):
    try:
        return {"name": label, "url": url, "status": "checked", **extract(fetch(url))}
    except HTTPError as error:
        if error.code == 404:
            return {"name": label, "url": url, "status": "not_found"}
        return {"name": label, "url": url, "status": "error", "error": str(error)}
    except Exception as error:
        return {"name": label, "url": url, "status": "error", "error": str(error)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--online", action="store_true")
    parser.add_argument("--output", type=Path, default=ROOT / "target/stack-packages.json")
    args = parser.parse_args()
    version, entries = inventory()
    report = {
        "checked_at": datetime.now(timezone.utc).isoformat(),
        "source_head": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
        "source_dirty": bool(subprocess.check_output(["git", "status", "--porcelain", "--untracked-files=normal"], cwd=ROOT, text=True).strip()),
        "engine_version": version,
        "cargo_lock_sha256": hashlib.sha256((ROOT / "Cargo.lock").read_bytes()).hexdigest(),
        "scope": "Tracked workspace Cargo manifests, npm SDK/example dependencies, native Python build/ONNX extras. Go and PHP clients have no third-party runtime dependencies. Excludes independent ignored Studio/site repositories. No dependency reachability or semantic-version compatibility inference.",
        "dependencies": entries,
        "registry_checks": [],
        "publication": [],
    }
    if args.online:
        with ThreadPoolExecutor(max_workers=4) as pool:
            report["registry_checks"] = list(pool.map(latest, sorted({(entry["ecosystem"], entry["name"]) for entry in entries})))
        checks = [
            ("npm tapirus", "https://registry.npmjs.org/tapirus/latest", lambda d: {"version": d["version"], "tarball": d["dist"]["tarball"]}),
            ("PyPI tapirus", "https://pypi.org/pypi/tapirus/json", lambda d: {"version": d["info"]["version"], "files": [f["filename"] for f in d["urls"]]}),
            ("crates.io tapirus", "https://crates.io/api/v1/crates/tapirus", lambda d: {"version": d["crate"]["max_stable_version"]}),
            ("npm @tapirus/wasm", "https://registry.npmjs.org/%40tapirus%2Fwasm/latest", lambda d: {"version": d["version"]}),
            ("GitHub latest release", "https://api.github.com/repos/tapiruslab/TapirusDB/releases/latest", lambda d: {"version": d["tag_name"], "assets": [a["name"] for a in d["assets"]]}),
            (f"GitHub v{version}", f"https://api.github.com/repos/tapiruslab/TapirusDB/releases/tags/v{version}", lambda d: {"version": d["tag_name"], "draft": d["draft"]}),
            ("Packagist tapiruslab/tapirusdb", "https://repo.packagist.org/p2/tapiruslab/tapirusdb.json", lambda d: {"versions": [p["version"] for p in d["packages"]["tapiruslab/tapirusdb"]]}),
            ("Go module", "https://proxy.golang.org/github.com/tapiruslab/!tapirus!d!b/sdks/go/@latest", lambda d: {"version": d["Version"]}),
        ]
        report["publication"] = [publication(*check) for check in checks]
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(f"Engine {version}: {len(entries)} dependency declarations; {len(report['registry_checks'])} registry checks. Report: {args.output}")
    for check in report["registry_checks"]:
        print(f"{check['ecosystem']} {check['name']}: {check.get('latest_stable', check.get('error'))}" + (f" (Rust {check['rust_version']})" if check.get("rust_version") else ""))
    for check in report["publication"]:
        print(f"{check['name']}: {check.get('version', check['status'])}")
    if any(check["status"] == "error" for check in report["registry_checks"] + report["publication"]):
        raise SystemExit("Registry queries failed; freshness/publication verification is incomplete")


if __name__ == "__main__":
    main()
