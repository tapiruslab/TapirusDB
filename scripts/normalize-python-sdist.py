"""Prune unrelated workspace entries from Maturin's source-package lockfile.

Requires Python 3.12+ for safe tar extraction. Registry versions/checksums must
remain a subset of the original lockfile. Source code and other members retain
their original bytes; Cargo then verifies the normalized package with --locked.
"""
import argparse
import copy
import gzip
import hashlib
import io
from pathlib import Path
import subprocess
import tarfile
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parents[1]


def registry_packages(lock):
    return {(p["name"], p["version"], p["source"]): p for p in lock["package"] if p.get("source", "").startswith("registry+")}


def normalize(archive, check):
    target = ROOT / "target"
    target.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="python-sdist-normalize-", dir=target) as directory:
        destination = Path(directory)
        with tarfile.open(archive) as source:
            members = source.getmembers()
            roots = {Path(member.name).parts[0] for member in members}
            if len(roots) != 1:
                raise RuntimeError("Expected one source-package root")
            source.extractall(destination, filter="data")
        package = destination / next(iter(roots))
        lock_path = package / "Cargo.lock"
        old_lock = tomllib.loads(lock_path.read_text(encoding="utf-8"))
        manifest = package / "sdks/python/Cargo.toml"
        metadata = ["cargo", "metadata", "--offline", "--format-version=1", "--manifest-path", str(manifest)]
        subprocess.run(metadata, stdout=subprocess.DEVNULL, check=True)
        normalized = lock_path.read_bytes()
        new_lock = tomllib.loads(normalized.decode())
        original_registry = registry_packages(old_lock)
        for key, value in registry_packages(new_lock).items():
            if original_registry.get(key) != value:
                raise RuntimeError(f"Normalization changed a registry dependency: {key}")
        subprocess.run([*metadata, "--locked"], stdout=subprocess.DEVNULL, check=True)
        if check:
            subprocess.run(["cargo", "check", "--locked", "--offline", "--manifest-path", str(manifest), "--target-dir", str(target)], check=True)
        with tempfile.NamedTemporaryFile(prefix=archive.name + ".", suffix=".tmp", dir=archive.parent, delete=False) as temporary:
            replacement = Path(temporary.name)
        try:
            with tarfile.open(archive) as source, replacement.open("wb") as output, gzip.GzipFile(filename="", fileobj=output, mode="wb", mtime=0) as compressed, tarfile.open(fileobj=compressed, mode="w") as result:
                lock_member = next(iter(roots)) + "/Cargo.lock"
                for member in members:
                    if member.name == lock_member:
                        member = copy.copy(member)
                        member.size = len(normalized)
                        result.addfile(member, io.BytesIO(normalized))
                    else:
                        result.addfile(member, source.extractfile(member) if member.isfile() else None)
            # Confirm no archive member other than Cargo.lock changed.
            with tarfile.open(archive) as before, tarfile.open(replacement) as after:
                if before.getnames() != after.getnames():
                    raise RuntimeError("Archive member list changed")
                for member in members:
                    if member.isfile() and member.name != lock_member and before.extractfile(member).read() != after.extractfile(member.name).read():
                        raise RuntimeError(f"Archive contents changed: {member.name}")
            replacement.replace(archive)
        finally:
            replacement.unlink(missing_ok=True)
        print(f"Normalized {archive.name}: {len(old_lock['package'])} -> {len(new_lock['package'])} lock entries, registry versions/checksums preserved, sha256={hashlib.sha256(archive.read_bytes()).hexdigest()}")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--sdist", type=Path, required=True)
    parser.add_argument("--offline", action="store_true")
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    archives = sorted(args.sdist.glob("*.tar.gz")) if args.sdist.is_dir() else [args.sdist]
    if not archives or any(not archive.is_file() for archive in archives):
        raise SystemExit("No source distributions found")
    if not args.offline:
        subprocess.run(["cargo", "fetch", "--locked"], cwd=ROOT, check=True)
    for archive in archives:
        normalize(archive.resolve(), args.check)


if __name__ == "__main__":
    main()
