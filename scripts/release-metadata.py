"""Hash packaged releases and inventory Cargo's resolved all-feature dependency graph.

The CycloneDX inventory records declared licenses; it is not an IP ownership audit.
"""
import argparse
import glob
import hashlib
import json
import subprocess
import tomllib
from pathlib import Path
from urllib.parse import quote

ROOT = Path(__file__).resolve().parents[1]


def command(*args):
    return subprocess.check_output(args, cwd=ROOT, text=True).strip()


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--artifact-glob", required=True)
    parser.add_argument("--target", required=True)
    parser.add_argument("--output-prefix", required=True)
    parser.add_argument("--offline", action="store_true")
    args = parser.parse_args()
    archives = sorted(p for p in map(Path, glob.glob(args.artifact_glob)) if p.name.endswith((".zip", ".tar.gz")))
    if not archives:
        raise SystemExit("No packaged release archives matched")
    metadata = json.loads(command("cargo", "metadata", "--locked", "--all-features", "--format-version=1", *(["--offline"] if args.offline else [])))
    lock = tomllib.loads((ROOT / "Cargo.lock").read_text(encoding="utf-8"))
    manifest = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))
    checksums = {(p["name"], p["version"]): p["checksum"] for p in lock["package"] if "checksum" in p}
    references = {p["id"]: f'pkg:cargo/{quote(p["name"])}@{quote(p["version"])}' for p in metadata["packages"]}
    root_package = next(p for p in metadata["packages"] if Path(p["manifest_path"]).resolve() == ROOT / "Cargo.toml")
    components = []
    for package in metadata["packages"]:
        component = {"type": "library", "name": package["name"], "version": package["version"], "bom-ref": references[package["id"]], "purl": references[package["id"]]}
        if package.get("license"):
            component["licenses"] = [{"expression": package["license"]}]
        checksum = checksums.get((package["name"], package["version"]))
        if checksum:
            component["hashes"] = [{"alg": "SHA-256", "content": checksum}]
        components.append(component)
    root_component = next(c for c in components if c["bom-ref"] == references[root_package["id"]])
    components.remove(root_component)
    head = command("git", "rev-parse", "HEAD")
    dirty = bool(command("git", "status", "--porcelain"))
    sbom = {"bomFormat": "CycloneDX", "specVersion": "1.5", "version": 1,
            "metadata": {"component": root_component, "properties": [{"name": "tapirus:sourceHead", "value": head}, {"name": "tapirus:inventoryScope", "value": "Cargo all-features resolved graph; not limited to binary-linked packages"}]},
            "components": sorted(components, key=lambda c: c["bom-ref"]),
            "dependencies": [{"ref": references[node["id"]], "dependsOn": sorted({references[dependency["pkg"]] for dependency in node["deps"]})} for node in metadata["resolve"]["nodes"]]}
    prefix = Path(args.output_prefix)
    prefix.parent.mkdir(parents=True, exist_ok=True)
    write_json(Path(str(prefix) + ".cdx.json"), sbom)
    artifacts = []
    for archive in archives:
        digest = hashlib.sha256(archive.read_bytes()).hexdigest()
        Path(str(archive) + ".sha256").write_text(f"{digest}  {archive.name}\n", encoding="ascii")
        artifacts.append({"name": archive.name, "sha256": digest, "bytes": archive.stat().st_size})
    write_json(Path(str(prefix) + ".build.json"), {"version": manifest["package"]["version"], "target": args.target, "sourceHead": head,
               "workingTreeDirty": dirty, "rustc": command("rustc", "--version"), "cargoLockSha256": hashlib.sha256((ROOT / "Cargo.lock").read_bytes()).hexdigest(), "artifacts": artifacts})
    print(f"Generated checksums, build metadata, and CycloneDX inventory for {len(artifacts)} archive(s)")


if __name__ == "__main__":
    main()
