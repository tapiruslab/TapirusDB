"""Build a Go module zip and install it through an isolated local file proxy.

This packages a local candidate, not a public Go tag. No sumdb or remote proxy
is consulted: the candidate is intentionally absent from public registries.
"""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
import tomllib
import zipfile

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--go", default="go")
    args = parser.parse_args()
    version = "v" + tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))["package"]["version"]
    source = ROOT / "sdks/go"
    module = next(line.split()[1] for line in (source / "go.mod").read_text().splitlines() if line.startswith("module "))
    escaped = ''.join('!' + char.lower() if char.isupper() else char for char in module)
    output = ROOT / "target/stack-packages/go"
    proxy = output / "proxy"
    distribution = proxy / escaped / "@v"
    distribution.mkdir(parents=True, exist_ok=True)
    archive = distribution / (version + ".zip")
    files = sorted(path for path in source.iterdir() if path.is_file() and (path.suffix == ".go" or path.name in {"go.mod", "README.md", "LICENSE"}))
    with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED) as result:
        for path in files:
            result.writestr(f"{module}@{version}/{path.name}", path.read_bytes())
    (distribution / (version + ".mod")).write_bytes((source / "go.mod").read_bytes())
    built = datetime.now(timezone.utc).isoformat().replace("+00:00", "Z")
    (distribution / (version + ".info")).write_text(json.dumps({"Version": version, "Time": built}), encoding="utf-8")
    (distribution / "list").write_text(version + "\n", encoding="utf-8")
    environment = {**os.environ, "GOPROXY": proxy.as_uri(), "GOSUMDB": "off", "GONOPROXY": "none", "GOPRIVATE": "", "GONOSUMDB": "*", "GOTOOLCHAIN": "local", "GOWORK": "off", "GOCACHE": str(ROOT / "target/go-build-cache")}
    with tempfile.TemporaryDirectory(prefix="tapirus-go-consumer-", dir=ROOT / "target") as temporary:
        consumer = Path(temporary)
        # Use a new cache so an earlier source snapshot cannot satisfy this test.
        environment["GOMODCACHE"] = str(ROOT / "target/go-module-consumers" / consumer.name)
        (consumer / "go.mod").write_text(f"module tapirus-local-consumer\n\ngo 1.20\n\nrequire {module} {version}\n", encoding="utf-8")
        (consumer / "main.go").write_text(f'package main\nimport ("fmt"; tapirus "{module}")\nfunc main() {{ fmt.Printf("Installed client: %T\\n", tapirus.NewClient()) }}\n', encoding="utf-8")
        subprocess.run([args.go, "mod", "download", module], cwd=consumer, env=environment, check=True)
        subprocess.run([args.go, "run", "."], cwd=consumer, env=environment, check=True)
    target = output / f"tapirus-go-{version[1:]}.zip"
    target.write_bytes(archive.read_bytes())
    checksum = hashlib.sha256(target.read_bytes()).hexdigest()
    (target.with_suffix(".zip.sha256")).write_text(f"{checksum}  {target.name}\n", encoding="utf-8")
    print(f"Go {version}: actual module zip installed from a fresh local file proxy and imported by a consumer. {target}")


if __name__ == "__main__":
    main()
