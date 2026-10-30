"""Synthetic positive and negative tests for the release-asset verifier."""

import hashlib
import importlib.util
import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest
import zipfile


ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("verify_release_assets", ROOT / "scripts/verify-release-assets.py")
verifier = importlib.util.module_from_spec(spec)
spec.loader.exec_module(verifier)
SOURCE_HEAD = "a" * 40


def make_archive(path: Path, names: set[str]) -> bytes:
    if path.name.endswith(".tar.gz"):
        with tarfile.open(path, "w:gz") as archive:
            for name in sorted(names):
                content = name.encode("utf-8")
                member = tarfile.TarInfo(name)
                member.size = len(content)
                archive.addfile(member, io.BytesIO(content))
    else:
        with zipfile.ZipFile(path, "w") as archive:
            for name in sorted(names):
                archive.writestr(name, name.encode("utf-8"))
    return path.read_bytes()


class ReleaseAssetTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="tapirus-release-test-")
        self.directory = Path(self.temporary.name)
        self.version = verifier.tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))["package"]["version"]
        self.lock_hash = hashlib.sha256((ROOT / "Cargo.lock").read_bytes()).hexdigest()
        self.make_bundle()

    def tearDown(self):
        self.temporary.cleanup()

    def make_bundle(self):
        for target, (base, extension) in verifier.TARGETS.items():
            windows = target.endswith("windows-msvc")
            for edition in (None, *verifier.EDITIONS):
                prefix = base if edition is None else f"{base}-{edition}"
                names = [f"{prefix}.tar.gz"] + ([f"{prefix}.zip"] if windows else [])
                if edition is None:
                    payload = {"tapirus.exe" if windows else "tapirus", "tapirus.dll" if windows else f"libtapirus.{extension}", "include/tapirus.h", "LICENSE", "README.md"}
                    if windows:
                        payload.add("tapirus.lib")
                else:
                    payload = {f"tapirus-{edition}" + (".exe" if windows else ""), "LICENSE", "EDITIONS.md"}
                artifacts = []
                for name in names:
                    data = make_archive(self.directory / name, payload)
                    digest = hashlib.sha256(data).hexdigest()
                    (self.directory / f"{name}.sha256").write_text(f"{digest}  {name}\n", encoding="ascii")
                    artifacts.append({"name": name, "sha256": digest, "bytes": len(data)})
                (self.directory / f"{prefix}.build.json").write_text(json.dumps({"version": self.version, "target": target, "sourceHead": SOURCE_HEAD, "workingTreeDirty": False, "cargoLockSha256": self.lock_hash, "artifacts": artifacts}), encoding="utf-8")
                (self.directory / f"{prefix}.cdx.json").write_text(json.dumps({"bomFormat": "CycloneDX", "metadata": {"component": {"name": "tapirus", "version": self.version, "bom-ref": "pkg:cargo/tapirus"}, "properties": [{"name": "tapirus:sourceHead", "value": SOURCE_HEAD}]}, "components": [{"name": "fixture-dependency"}], "dependencies": [{"ref": "pkg:cargo/tapirus", "dependsOn": ["fixture-dependency"]}]}), encoding="utf-8")

    def test_complete_release_passes(self):
        self.assertEqual(verifier.verify(self.directory, SOURCE_HEAD), 30)

    def test_missing_asset_is_rejected(self):
        (self.directory / "tapirus-linux-x86_64-vector.tar.gz").unlink()
        with self.assertRaises((FileNotFoundError, ValueError)):
            verifier.verify(self.directory, SOURCE_HEAD)

    def test_corrupted_archive_is_rejected(self):
        with (self.directory / "tapirus-linux-x86_64.tar.gz").open("ab") as archive:
            archive.write(b"tampered")
        with self.assertRaisesRegex(ValueError, "checksum"):
            verifier.verify(self.directory, SOURCE_HEAD)

    def test_dirty_source_record_is_rejected(self):
        path = self.directory / "tapirus-linux-x86_64.build.json"
        record = json.loads(path.read_text(encoding="utf-8"))
        record["workingTreeDirty"] = True
        path.write_text(json.dumps(record), encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "Dirty source"):
            verifier.verify(self.directory, SOURCE_HEAD)

    def test_empty_dependency_inventory_is_rejected(self):
        path = self.directory / "tapirus-linux-x86_64.cdx.json"
        inventory = json.loads(path.read_text(encoding="utf-8"))
        inventory["components"] = []
        path.write_text(json.dumps(inventory), encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "Empty dependency inventory"):
            verifier.verify(self.directory, SOURCE_HEAD)


if __name__ == "__main__":
    unittest.main()
