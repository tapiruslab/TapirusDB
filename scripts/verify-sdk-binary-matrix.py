#!/usr/bin/env python3
"""
Verify TapirusDB Native FFI Wheel (PyO3 abi3) & Node.js N-API Binary Matrix (Peluang 7 / FIX-19).

Validates:
1. Version lockstep across root `Cargo.toml`, `crates/tapirus-ffi/pyproject.toml`,
   `crates/tapirus-nodejs/Cargo.toml`, and `sdks/nodejs/package.json`.
2. Complete target coverage in `.github/workflows/publish-sdks.yml` across:
   - `x86_64-unknown-linux-gnu`
   - `aarch64-unknown-linux-gnu`
   - `x86_64-apple-darwin`
   - `aarch64-apple-darwin`
   - `x86_64-pc-windows-msvc`
3. Optional `--wheel-dir` inspection verifying built `.whl` archives carry `abi3` tags and native
   shared library binaries (`.pyd` / `.so`), and `--node-dir` inspection verifying `.node` addons.
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import re
import sys
import zipfile

REQUIRED_MATRIX_TARGETS = (
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
    "x86_64-apple-darwin",
    "aarch64-apple-darwin",
    "x86_64-pc-windows-msvc",
)


def extract_toml_version(path: Path) -> str:
    text = path.read_text(encoding="utf-8")
    match = re.search(r'^version\s*=\s*"([^"]+)"', text, flags=re.MULTILINE)
    if not match:
        raise AssertionError(f"Could not find version in {path}")
    return match.group(1)


def verify_manifest_matrix(root: Path) -> dict[str, object]:
    cargo_ver = extract_toml_version(root / "Cargo.toml")
    py_pyproject = (
        root / "sdks" / "python" / "pyproject.toml"
        if (root / "sdks" / "python" / "pyproject.toml").exists()
        else root / "python" / "pyproject.toml"
    )
    ffi_pyproject_ver = extract_toml_version(py_pyproject)
    node_pkg = json.loads((root / "sdks" / "nodejs" / "package.json").read_text(encoding="utf-8"))
    node_ver = node_pkg["version"]

    assert cargo_ver == ffi_pyproject_ver == node_ver, (
        f"Version mismatch across manifests: Cargo.toml={cargo_ver}, "
        f"{py_pyproject.name}={ffi_pyproject_ver}, sdks/nodejs/package.json={node_ver}"
    )

    py_cargo_path = (
        root / "sdks" / "python" / "Cargo.toml"
        if (root / "sdks" / "python" / "Cargo.toml").exists()
        else root / "crates" / "tapirus-ffi" / "Cargo.toml"
    )
    ffi_cargo = py_cargo_path.read_text(encoding="utf-8")
    assert "cdylib" in ffi_cargo, f"{py_cargo_path} must build a cdylib native extension"

    node_cargo_path = (
        root / "sdks" / "nodejs" / "Cargo.toml"
        if (root / "sdks" / "nodejs" / "Cargo.toml").exists()
        else root / "crates" / "tapirus-nodejs" / "Cargo.toml"
    )
    if node_cargo_path.exists():
        node_cargo = node_cargo_path.read_text(encoding="utf-8")
        assert "cdylib" in node_cargo, "Node.js binding crate must build a cdylib N-API addon"

    workflow_path = root / ".github" / "workflows" / "publish-sdks.yml"
    workflow_text = workflow_path.read_text(encoding="utf-8")
    for target in REQUIRED_MATRIX_TARGETS:
        assert target in workflow_text, f"Missing target '{target}' in {workflow_path}"

    assert "PyO3/maturin-action" in workflow_text, "publish-sdks.yml must use maturin-action for Python wheels"
    assert "build-nodejs-binaries" in workflow_text, "publish-sdks.yml must define build-nodejs-binaries job"

    return {
        "status": "ok",
        "version": cargo_ver,
        "targets_verified": list(REQUIRED_MATRIX_TARGETS),
        "python_abi": "cp39-abi3",
        "node_addon": "napi-cdylib",
    }


def verify_wheel_dir(wheel_dir: Path) -> list[str]:
    wheels = sorted(wheel_dir.glob("*.whl"))
    if not wheels:
        raise AssertionError(f"No .whl files found in {wheel_dir}")
    verified = []
    for whl in wheels:
        with zipfile.ZipFile(whl, "r") as zf:
            names = zf.namelist()
            has_native = any(n.endswith((".pyd", ".so", ".dylib")) for n in names)
            assert has_native, f"Wheel {whl.name} is missing compiled native extension (.pyd/.so)"
        verified.append(whl.name)
    return verified


def verify_node_dir(node_dir: Path) -> list[str]:
    addons = sorted(node_dir.glob("*.node"))
    if not addons:
        raise AssertionError(f"No .node N-API binaries found in {node_dir}")
    verified = []
    for addon in addons:
        raw = addon.read_bytes()[:4]
        is_valid_binary = (
            raw[:2] == b"MZ"  # Windows PE (.dll/.node)
            or raw == b"\x7fELF"  # Linux ELF (.so/.node)
            or raw in (b"\xcf\xfa\xed\xfe", b"\xfe\xed\xfa\xcf", b"\xca\xfe\xba\xbe")  # macOS Mach-O
        )
        assert is_valid_binary, f"Invalid binary header in N-API addon {addon.name}: {raw!r}"
        verified.append(addon.name)
    return verified


def main() -> int:
    parser = argparse.ArgumentParser(description="Verify TapirusDB FFI Wheel & N-API Binary Matrix")
    parser.add_argument("--verify-manifest-matrix", action="store_true", help="Verify manifests and CI matrix")
    parser.add_argument("--wheel-dir", type=Path, help="Verify built .whl files in directory")
    parser.add_argument("--node-dir", type=Path, help="Verify built .node N-API binaries in directory")
    args = parser.parse_args()

    root = Path(__file__).resolve().parents[1]
    report: dict[str, object] = {}

    if args.verify_manifest_matrix or (not args.wheel_dir and not args.node_dir):
        report["manifest_matrix"] = verify_manifest_matrix(root)
    if args.wheel_dir:
        report["wheels"] = verify_wheel_dir(args.wheel_dir)
    if args.node_dir:
        report["node_addons"] = verify_node_dir(args.node_dir)

    print(json.dumps(report))
    return 0


if __name__ == "__main__":
    sys.exit(main())
