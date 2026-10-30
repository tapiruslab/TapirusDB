"""Fail closed unless a complete TapirusDB release bundle matches this source tree."""

import argparse
import hashlib
import json
from pathlib import Path
import re
import tarfile
import tomllib
import zipfile


ROOT = Path(__file__).resolve().parents[1]
TARGETS = {
    "x86_64-unknown-linux-gnu": ("tapirus-linux-x86_64", "so"),
    "aarch64-unknown-linux-gnu": ("tapirus-linux-aarch64", "so"),
    "x86_64-apple-darwin": ("tapirus-macos-x86_64", "dylib"),
    "aarch64-apple-darwin": ("tapirus-macos-aarch64", "dylib"),
    "x86_64-pc-windows-msvc": ("tapirus-windows-x86_64", "dll"),
}
EDITIONS = ("lite", "document", "graph", "vector")


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def read_json(path: Path) -> dict:
    return json.loads(path.read_text(encoding="utf-8"))


def archive_files(path: Path) -> set[str]:
    if path.name.endswith(".tar.gz"):
        with tarfile.open(path, "r:gz") as archive:
            entries = [(member.name, member.isfile(), member.isdir()) for member in archive]
    else:
        with zipfile.ZipFile(path) as archive:
            entries = [(member.filename, not member.is_dir(), member.is_dir()) for member in archive.infolist()]
    files = set()
    for name, is_file, is_dir in entries:
        require("\\" not in name and not name.startswith("/"), f"Unsafe archive path in {path.name}: {name}")
        parts = [part for part in name.split("/") if part not in ("", ".")]
        require(".." not in parts and all(":" not in part for part in parts), f"Unsafe archive path in {path.name}: {name}")
        require(is_file or is_dir, f"Nonregular archive entry in {path.name}: {name}")
        if is_file:
            normalized = "/".join(parts)
            require(normalized not in files, f"Duplicate archive entry in {path.name}: {name}")
            files.add(normalized)
    return files


def verify(directory: Path, source_head: str) -> int:
    require(re.fullmatch(r"[a-f0-9]{40}", source_head) is not None, "Expected a full source commit SHA")
    require(directory.is_dir(), f"Missing release directory: {directory}")
    version = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))["package"]["version"]
    lock_hash = hashlib.sha256((ROOT / "Cargo.lock").read_bytes()).hexdigest()
    expected_files = set()
    archive_count = 0

    for target, (base, library_extension) in TARGETS.items():
        windows = target.endswith("windows-msvc")
        for edition in (None, *EDITIONS):
            prefix = base if edition is None else f"{base}-{edition}"
            archive_names = [f"{prefix}.tar.gz"]
            if windows:
                archive_names.append(f"{prefix}.zip")
            build_name = f"{prefix}.build.json"
            bom_name = f"{prefix}.cdx.json"
            expected_files.update((build_name, bom_name))

            build = read_json(directory / build_name)
            require(build.get("version") == version, f"Wrong version in {build_name}")
            require(build.get("target") == target, f"Wrong target in {build_name}")
            require(build.get("sourceHead") == source_head, f"Wrong source commit in {build_name}")
            require(build.get("workingTreeDirty") is False, f"Dirty source in {build_name}")
            require(build.get("cargoLockSha256") == lock_hash, f"Wrong lockfile in {build_name}")
            recorded = {item.get("name"): item for item in build.get("artifacts", [])}
            require(set(recorded) == set(archive_names), f"Incomplete artifact record in {build_name}")

            bom = read_json(directory / bom_name)
            require(bom.get("bomFormat") == "CycloneDX", f"Invalid inventory in {bom_name}")
            root_component = bom.get("metadata", {}).get("component", {})
            require(root_component.get("name") == "tapirus" and root_component.get("version") == version, f"Wrong inventory root in {bom_name}")
            require(bool(bom.get("components")), f"Empty dependency inventory in {bom_name}")
            require(any(item.get("ref") == root_component.get("bom-ref") and item.get("dependsOn") for item in bom.get("dependencies", [])), f"Missing root dependency graph in {bom_name}")
            properties = bom.get("metadata", {}).get("properties", [])
            require(any(item.get("name") == "tapirus:sourceHead" and item.get("value") == source_head for item in properties), f"Wrong inventory source in {bom_name}")

            if edition is None:
                executable = "tapirus.exe" if windows else "tapirus"
                library = "tapirus.dll" if windows else f"libtapirus.{library_extension}"
                payload = {executable, library, "include/tapirus.h", "LICENSE", "README.md"}
                if windows:
                    payload.add("tapirus.lib")
            else:
                executable = f"tapirus-{edition}" + (".exe" if windows else "")
                payload = {executable, "LICENSE", "EDITIONS.md"}

            for name in archive_names:
                path = directory / name
                data = path.read_bytes()
                digest = hashlib.sha256(data).hexdigest()
                checksum_name = f"{name}.sha256"
                manifest = (directory / checksum_name).read_text(encoding="ascii")
                require(manifest == f"{digest}  {name}\n", f"Bad checksum manifest: {checksum_name}")
                require(recorded[name].get("sha256") == digest, f"Wrong recorded digest: {name}")
                require(recorded[name].get("bytes") == len(data), f"Wrong recorded size: {name}")
                require(archive_files(path) == payload, f"Unexpected archive payload: {name}")
                expected_files.update((name, checksum_name))
                archive_count += 1

    actual_files = {item.name for item in directory.iterdir() if item.is_file()}
    require(actual_files == expected_files, f"Release asset set differs: missing={sorted(expected_files - actual_files)}, extra={sorted(actual_files - expected_files)}")
    print(f"Verified {archive_count} archives and {len(expected_files)} total assets for {version} at {source_head}")
    return archive_count


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--directory", required=True, type=Path)
    parser.add_argument("--source-head", required=True)
    args = parser.parse_args()
    verify(args.directory, args.source_head)


if __name__ == "__main__":
    main()
