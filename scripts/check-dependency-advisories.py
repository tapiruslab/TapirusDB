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


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, default=Path("target/dependency-advisories.json"))
    args = parser.parse_args()
    lock = (Path(__file__).resolve().parents[1] / "Cargo.lock").read_bytes()
    packages = [p for p in tomllib.loads(lock.decode())["package"] if p.get("source", "").startswith("registry+")]
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
                findings.append({"package": package["name"], "version": package["version"], "id": identifier, "summary": details.get("summary"), "withdrawn": details.get("withdrawn"), "database_specific": details.get("database_specific", {}), "url": "https://osv.dev/vulnerability/" + identifier})
    report = {"checked_at": datetime.now(timezone.utc).isoformat(), "source": "https://api.osv.dev", "scope": "all Cargo.lock registry packages, including optional/dev/other-target dependencies; no reachability analysis", "cargo_lock_sha256": hashlib.sha256(lock).hexdigest(), "packages_checked": len(packages), "findings": findings}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(report, indent=2))
    if any(not f["withdrawn"] for f in findings):
        raise SystemExit("Dependency advisories require review; see the generated report")


if __name__ == "__main__":
    main()
