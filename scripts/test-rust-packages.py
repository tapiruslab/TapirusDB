"""Compile extracted .crate artifacts with local unpublished sibling crates.

Cargo 1.98.1's workspace package verifier can fail with 'no hash listed' for an
unpublished sibling. This checks the actual packaged source in a temporary
workspace without changing archive bytes or claiming a registry installation.
Registry names, versions and checksums must remain a subset of Cargo.lock.
"""
import argparse
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parents[1]
NAMES = ("tapirus", "tapirus-micro", "tapirus-ffi", "tapirus-wasm")


def registry(lock):
    return {(p["name"], p["version"], p["source"]): p.get("checksum") for p in lock["package"] if p.get("source", "").startswith("registry+")}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--directory", type=Path, default=ROOT / "target/package")
    args = parser.parse_args()
    version = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))["package"]["version"]
    expected = registry(tomllib.loads((ROOT / "Cargo.lock").read_text(encoding="utf-8")))
    with tempfile.TemporaryDirectory(prefix="rust-package-consumer-", dir=ROOT / "target") as temporary:
        directory = Path(temporary)
        for name in NAMES:
            with tarfile.open(args.directory / f"{name}-{version}.crate") as archive:
                if f"{name}-{version}/LICENSE" not in archive.getnames():
                    raise RuntimeError(f"Packaged license missing: {name}")
                if any(Path(member.name).parts[0] != f"{name}-{version}" for member in archive.getmembers()):
                    raise RuntimeError(f"Unexpected package root: {name}")
                archive.extractall(directory, filter="data")
        # This harness is outside the extracted archives. Cargo patches only
        # sibling registry dependencies to their actual extracted packages.
        members = [f"{name}-{version}" for name in NAMES]
        harness = '[workspace]\nresolver = "3"\nmembers = [' + ', '.join(f'"{member}"' for member in members) + ']\n\n[patch.crates-io]\n'
        harness += ''.join(f'{name} = {{ path = "{name}-{version}" }}\n' for name in NAMES)
        (directory / "Cargo.toml").write_text(harness, encoding="utf-8")
        shutil.copyfile(ROOT / "Cargo.lock", directory / "Cargo.lock")
        subprocess.run(["cargo", "metadata", "--offline", "--format-version=1"], cwd=directory, stdout=subprocess.DEVNULL, check=True)
        actual = registry(tomllib.loads((directory / "Cargo.lock").read_text(encoding="utf-8")))
        if any(expected.get(key) != checksum for key, checksum in actual.items()):
            raise RuntimeError("Extracted crates changed a locked registry dependency")
        subprocess.run(["cargo", "check", "--locked", "--offline", "--workspace", "--all-features", "--target-dir", str(ROOT / "target/rust-package-build")], cwd=directory, check=True)
        print(f"Verified {len(NAMES)} extracted Rust {version} packages; sibling path patches only, registry versions/checksums preserved.")


if __name__ == "__main__":
    main()
