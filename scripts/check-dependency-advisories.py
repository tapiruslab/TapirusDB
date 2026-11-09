"""Query OSV for all registry packages in Cargo.lock; no secrets are submitted.

Only public package names/versions are sent. This checks known advisories rather
than code reachability, exploitability, or the absence of undisclosed issues.
"""
import argparse
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import tomllib
from urllib.request import Request, urlopen


def request(endpoint, payload=None):
    data = None if payload is None else json.dumps(payload).encode()
    req = Request("https://api.osv.dev/v1/" + endpoint, data=data, headers={"Content-Type": "application/json", "User-Agent": "TapirusDB-dependency-validation"})
    with urlopen(req, timeout=45) as response:
        return json.load(response)


def dependency_name(value):
    """Return the package name from a Cargo.lock dependency entry."""
    return value.split(" ", 1)[0]


def manifest_dependencies(data, sections):
    names = set()
    for section in sections:
        for alias, dependency in data.get(section, {}).items():
            if isinstance(dependency, dict):
                names.add(dependency.get("package", alias))
            else:
                names.add(alias)
    return names


def workspace_manifests(root, root_data):
    paths = [root / "Cargo.toml"]
    for member in root_data.get("workspace", {}).get("members", []):
        pattern = root / member
        candidates = root.glob(member) if any(char in member for char in "*?[") else [pattern]
        for candidate in candidates:
            manifest = candidate / "Cargo.toml"
            if manifest.is_file() and manifest not in paths:
                paths.append(manifest)
    return [tomllib.loads(path.read_text(encoding="utf-8")) for path in paths]


def dependency_scopes(root, root_data, packages):
    """Classify locked packages as production, development-only or unreachable.

    Cargo.lock does not retain dependency kind on each edge, so workspace
    manifests seed the graph and registry package edges are followed normally.
    This is a release-policy classification, not a reachability or exploitability
    proof.
    """
    nodes_by_name = {}
    for package in packages:
        nodes_by_name.setdefault(package["name"], []).append(package)
    workspace = {}
    for data in workspace_manifests(root, root_data):
        package = data.get("package")
        if package:
            workspace[package["name"]] = data

    production = set()
    development = set()
    pending = []
    for data in workspace.values():
        for name in manifest_dependencies(data, ("dependencies", "build-dependencies")):
            pending.append((name, "production"))
        for name in manifest_dependencies(data, ("dev-dependencies",)):
            pending.append((name, "development"))

    visited = set()
    while pending:
        name, scope = pending.pop()
        for package in nodes_by_name.get(name, []):
            key = (package["name"], package["version"])
            if scope == "production":
                production.add(key)
            else:
                development.add(key)
            visit_key = (key, scope)
            if visit_key in visited:
                continue
            visited.add(visit_key)
            manifest = workspace.get(package["name"])
            if manifest:
                sections = ("dependencies", "build-dependencies") if scope == "production" else ("dependencies", "build-dependencies", "dev-dependencies")
                children = manifest_dependencies(manifest, sections)
            else:
                children = {dependency_name(value) for value in package.get("dependencies", [])}
            pending.extend((child, scope) for child in children)

    scopes = {}
    for package in packages:
        key = (package["name"], package["version"])
        if key in production:
            scopes[key] = "production"
        elif key in development:
            scopes[key] = "development-only"
        else:
            scopes[key] = "unreachable"
    return scopes


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, default=Path("target/dependency-advisories.json"))
    parser.add_argument("--fail-dev-only", action="store_true", help="also fail for findings classified as development-only")
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    lock = (root / "Cargo.lock").read_bytes()
    root_data = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))
    lock_data = tomllib.loads(lock.decode())
    packages = [p for p in lock_data["package"] if p.get("source", "").startswith("registry+")]
    scopes = dependency_scopes(root, root_data, lock_data["package"])
    findings = []
    for start in range(0, len(packages), 50):
        batch = packages[start:start + 50]
        queries = [{"package": {"ecosystem": "crates.io", "name": p["name"]}, "version": p["version"]} for p in batch]
        pending = list(enumerate(queries))
        ids = [set() for _ in batch]
        while pending:
            results = request("querybatch", {"queries": [q for _, q in pending]})["results"]
            if len(results) != len(pending):
                raise RuntimeError("OSV returned an incomplete batch")
            more = []
            for (index, query), result in zip(pending, results):
                ids[index].update(v["id"] for v in result.get("vulns", []))
                if result.get("next_page_token"):
                    more.append((index, {**query, "page_token": result["next_page_token"]}))
            pending = more
        for package, identifiers in zip(batch, ids):
            for identifier in sorted(identifiers):
                details = request("vulns/" + identifier)
                key = (package["name"], package["version"])
                dependency_scope = scopes.get(key, "unreachable")
                findings.append({"package": package["name"], "version": package["version"], "id": identifier, "summary": details.get("summary"), "withdrawn": details.get("withdrawn"), "database_specific": details.get("database_specific", {}), "url": "https://osv.dev/vulnerability/" + identifier, "dependency_scope": dependency_scope, "release_blocking": dependency_scope == "production"})
    report = {"checked_at": datetime.now(timezone.utc).isoformat(), "source": "https://api.osv.dev", "scope": "all Cargo.lock registry packages, including optional/dev/other-target dependencies; workspace dependency scope is classified from manifests and is not reachability or exploitability analysis", "policy": "active production findings block by default; development-only findings remain visible and require explicit review", "cargo_lock_sha256": hashlib.sha256(lock).hexdigest(), "packages_checked": len(packages), "findings": findings}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(report, indent=2))
    blocking = [f for f in findings if not f["withdrawn"] and (f["release_blocking"] or args.fail_dev_only)]
    if blocking:
        raise SystemExit("Dependency advisories require review; see the generated report")


if __name__ == "__main__":
    main()
