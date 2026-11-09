"""Collect locally verified SDK artifacts into a candidate bundle with hashes.

This does not publish packages, tag a release, or imply all host architectures
were built locally. Native targets are identified by the actual supplied files.
"""
import argparse
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import platform
import subprocess
import tomllib
import zipfile

ROOT = Path(__file__).resolve().parents[1]


def git(*args):
    return subprocess.check_output(["git", *args], cwd=ROOT, text=True).strip()


def validate_native_archive(archive, version, lock_hash, source_head):
    prefix = archive.name.removesuffix(".tar.gz").removesuffix(".zip")
    record = archive.parent / f"{prefix}.build.json"
    if not record.is_file():
        raise SystemExit(f"Missing native build record: {record}")
    build = json.loads(record.read_text(encoding="utf-8"))
    if (build.get("version"), build.get("cargoLockSha256"), build.get("sourceHead")) != (version, lock_hash, source_head):
        raise SystemExit("Native archive build record does not match the current version, lockfile and source HEAD")
    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    if not any(item.get("name") == archive.name and item.get("sha256") == digest and item.get("bytes") == archive.stat().st_size for item in build.get("artifacts", [])):
        raise SystemExit("Native archive does not match its build record")
    sidecar = archive.with_name(archive.name + ".sha256")
    if not sidecar.is_file() or sidecar.read_text(encoding="ascii").strip() != f"{digest}  {archive.name}":
        raise SystemExit("Native archive SHA-256 sidecar is missing or incorrect")
    return record, sidecar


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--native-archive", type=Path, required=True)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--freshness-report", type=Path, default=ROOT / "target/stack-packages.json")
    parser.add_argument("--advisory-report", type=Path, default=ROOT / "target/dependency-advisories.json")
    args = parser.parse_args()
    version = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))["package"]["version"]
    lock_hash = hashlib.sha256((ROOT / "Cargo.lock").read_bytes()).hexdigest()
    source_head = git("rev-parse", "HEAD")
    for report in (args.freshness_report, args.advisory_report):
        data = json.loads(report.read_text(encoding="utf-8"))
        if data.get("cargo_lock_sha256") != lock_hash:
            raise SystemExit(f"Report does not match current Cargo.lock: {report}")
    advisories = json.loads(args.advisory_report.read_text(encoding="utf-8"))
    if any(not finding.get("withdrawn") and finding.get("release_blocking", True) for finding in advisories.get("findings", [])):
        raise SystemExit("Candidate bundle contains active production dependency advisory findings")
    freshness = json.loads(args.freshness_report.read_text(encoding="utf-8"))
    if freshness.get("source_head") != source_head or freshness.get("engine_version") != version:
        raise SystemExit("Registry report does not match current source HEAD and engine version")
    if not freshness.get("registry_checks") or not freshness.get("publication") or any(check["status"] != "checked" for check in freshness["registry_checks"]) or any(check["status"] not in ("checked", "not_found") for check in freshness["publication"]):
        raise SystemExit("Complete the online registry checks before bundling")
    native_record, native_sidecar = validate_native_archive(args.native_archive, version, lock_hash, source_head)
    directory = ROOT / "target/stack-packages"
    wheels = list((directory / "python").glob(f"tapirus-{version}-*.whl"))
    if len(wheels) != 1:
        raise SystemExit("Expected exactly one locally verified native wheel")
    artifacts = [
        *[(f"rust/{name}-{version}.crate", ROOT / f"target/package/{name}-{version}.crate") for name in ("tapirus", "tapirus-micro", "tapirus-ffi", "tapirus-wasm")],
        (f"node/tapirus-{version}.tgz", directory / f"node/tapirus-{version}.tgz"),
        (f"wasm/tapirus-wasm-{version}.tgz", directory / f"wasm/tapirus-wasm-{version}.tgz"),
        (f"python/{wheels[0].name}", wheels[0]),
        (f"python/tapirus-{version}.tar.gz", directory / f"python/tapirus-{version}.tar.gz"),
        (f"go/tapirus-go-{version}.zip", directory / f"go/tapirus-go-{version}.zip"),
        (f"php/tapirus-php-{version}.zip", directory / f"php/tapirus-php-{version}.zip"),
        (f"native/{args.native_archive.name}", args.native_archive),
    ]
    for name, file in artifacts:
        if not file.is_file():
            raise SystemExit(f"Missing candidate artifact: {name} ({file})")
    source_files = git("ls-files", "--cached", "--others", "--exclude-standard").splitlines()
    source_hashes = {name: hashlib.sha256((ROOT / name).read_bytes()).hexdigest() for name in sorted(set(source_files)) if (ROOT / name).is_file()}
    manifest = {
        "kind": "local-candidate-not-published",
        "engine_version": version,
        "packaged_at": datetime.now(timezone.utc).isoformat(),
        "source_head": source_head,
        "source_dirty": bool(git("status", "--porcelain")),
        "source_file_sha256": source_hashes,
        "cargo_lock_sha256": lock_hash,
        "local_host": platform.platform(),
        "scope": "Local artifacts only; native wheel/addon/CLI target names identify built hosts. Rust sibling crates were checked together with local path patches, not installed from crates.io. Go was installed from an isolated local file proxy. Registry publication remains separate.",
        "artifacts": [{"path": name, "bytes": file.stat().st_size, "sha256": hashlib.sha256(file.read_bytes()).hexdigest()} for name, file in artifacts],
    }
    output = args.output or ROOT / f"dist/tapirusdb-{version}-stack-candidate.zip"
    output.parent.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(output, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        for name, file in artifacts:
            archive.write(file, name)
        archive.writestr("BUILD.json", json.dumps(manifest, indent=2) + "\n")
        archive.writestr("SHA256SUMS", ''.join(f"{item['sha256']}  {item['path']}\n" for item in manifest["artifacts"]))
        archive.write(ROOT / "docs/STACK_SUPPORT.md", "STACK_SUPPORT.md")
        for name, file in [("freshness.json", args.freshness_report), ("advisories.json", args.advisory_report)]:
            archive.write(file, f"evidence/{name}")
        for file in (native_record, native_sidecar, args.native_archive.parent / f"{native_record.name.removesuffix('.build.json')}.cdx.json"):
            if file.is_file():
                archive.write(file, f"native/{file.name}")
    digest = hashlib.sha256(output.read_bytes()).hexdigest()
    output.with_suffix(output.suffix + ".sha256").write_text(f"{digest}  {output.name}\n", encoding="utf-8")
    print(f"Packaged {len(artifacts)} local candidate artifacts: {output}\nsha256={digest}\nsource_dirty={manifest['source_dirty']}")


if __name__ == "__main__":
    main()
