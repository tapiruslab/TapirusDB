"""Measure current TapirusDB edition binaries using real CLI file operations.

Run from any directory. By default, rebuilds every edition from this checkout
with the same size-oriented Cargo profile. Results are local observations, not
cross-platform claims or substitutes for device-level power-loss testing.
"""

from __future__ import annotations

import argparse
import ctypes
import hashlib
import json
import os
import platform
import re
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
EDITIONS = {
    "lite": ("tapirus-lite", "lite"),
    "document": ("tapirus-document", "document"),
    "graph": ("tapirus-graph", "graph-only"),
    "vector": ("tapirus-vector", "vector-only"),
    "quad": ("tapirus", None),
}


def output(command: list[str]) -> str:
    return subprocess.check_output(command, cwd=ROOT, text=True, stderr=subprocess.DEVNULL).strip()


def current_rss(pid: int) -> int | None:
    if sys.platform == "win32":
        class Counters(ctypes.Structure):
            _fields_ = [
                ("cb", ctypes.c_uint32),
                ("PageFaultCount", ctypes.c_uint32),
                ("PeakWorkingSetSize", ctypes.c_size_t),
                ("WorkingSetSize", ctypes.c_size_t),
                ("QuotaPeakPagedPoolUsage", ctypes.c_size_t),
                ("QuotaPagedPoolUsage", ctypes.c_size_t),
                ("QuotaPeakNonPagedPoolUsage", ctypes.c_size_t),
                ("QuotaNonPagedPoolUsage", ctypes.c_size_t),
                ("PagefileUsage", ctypes.c_size_t),
                ("PeakPagefileUsage", ctypes.c_size_t),
                ("PrivateUsage", ctypes.c_size_t),
            ]

        kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        psapi = ctypes.WinDLL("psapi", use_last_error=True)
        kernel.OpenProcess.argtypes = (ctypes.c_uint32, ctypes.c_int, ctypes.c_uint32)
        kernel.OpenProcess.restype = ctypes.c_void_p
        kernel.CloseHandle.argtypes = (ctypes.c_void_p,)
        psapi.GetProcessMemoryInfo.argtypes = (ctypes.c_void_p, ctypes.c_void_p, ctypes.c_uint32)
        handle = kernel.OpenProcess(0x1000, 0, pid)
        if not handle:
            return None
        try:
            counters = Counters()
            counters.cb = ctypes.sizeof(Counters)
            if psapi.GetProcessMemoryInfo(handle, ctypes.byref(counters), counters.cb):
                return int(counters.PeakWorkingSetSize)
        finally:
            kernel.CloseHandle(handle)
        return None
    if sys.platform.startswith("linux"):
        try:
            for line in Path(f"/proc/{pid}/status").read_text().splitlines():
                if line.startswith("VmHWM:"):
                    return int(line.split()[1]) * 1024
        except (FileNotFoundError, PermissionError, ValueError):
            return None
        return None
    if sys.platform == "darwin":
        try:
            return int(subprocess.check_output(["ps", "-o", "rss=", "-p", str(pid)], text=True).strip()) * 1024
        except (subprocess.CalledProcessError, ValueError):
            return None
    return None


def run(binary: Path, *args: str) -> dict:
    started = time.perf_counter()
    process = subprocess.Popen([str(binary), *args], cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    peak: int | None = None
    done = threading.Event()

    def sample() -> None:
        nonlocal peak
        while not done.is_set():
            value = current_rss(process.pid)
            if value is not None:
                peak = max(peak or 0, value)
            if process.poll() is not None:
                break
            done.wait(0.002)

    sampler = threading.Thread(target=sample, daemon=True)
    sampler.start()
    try:
        stdout, stderr = process.communicate(timeout=30)
    except subprocess.TimeoutExpired:
        process.kill()
        process.communicate()
        raise RuntimeError(f"Timed out: {binary.name} {args[:2]}") from None
    finally:
        done.set()
        sampler.join(timeout=1)
    if process.returncode != 0:
        raise RuntimeError(f"{binary.name} failed ({process.returncode}): {stderr.strip()}")
    return {
        "elapsed_ms": round((time.perf_counter() - started) * 1000, 3),
        "peak_working_set_bytes": peak,
        "stdout": stdout.strip(),
    }


def require_json_text(result: dict, expected: str) -> None:
    parsed = json.loads(result["stdout"])
    if expected not in json.dumps(parsed, ensure_ascii=False):
        raise RuntimeError(f"Reopen did not return expected content: {expected!r}")


def smoke(name: str, binary: Path, directory: Path) -> dict:
    database = directory / f"{name}.tapir"
    operations: dict[str, dict] = {}
    path = str(database)
    if name == "lite":
        operations["create"] = run(binary, path, "--exec", "CREATE TABLE notes (id INTEGER PRIMARY KEY, body TEXT)")
        operations["insert"] = run(binary, path, "--exec", "INSERT INTO notes VALUES (1, 'persisted')")
        operations["reopen_read"] = run(binary, path, "--query", "SELECT body FROM notes WHERE id = 1")
        require_json_text(operations["reopen_read"], "persisted")
    elif name == "document":
        operations["insert"] = run(binary, path, "notes", "insert", '{"body":"persisted"}')
        operations["reopen_read"] = run(binary, path, "notes", "list")
        require_json_text(operations["reopen_read"], "persisted")
    elif name == "graph":
        operations["node_1"] = run(binary, path, "node", "1", "Person", '{"name":"Alex Chen"}')
        operations["node_2"] = run(binary, path, "node", "2", "Person", '{"name":"Sarah Connor"}')
        operations["edge"] = run(binary, path, "edge", "1", "2", "KNOWS")
        operations["reopen_read"] = run(binary, path, "neighbors", "1")
        require_json_text(operations["reopen_read"], "KNOWS")
    elif name == "vector":
        operations["insert"] = run(binary, path, "notes", "2", "insert", "1", "[1.0,0.0]", '{"body":"persisted"}')
        operations["reopen_read"] = run(binary, path, "notes", "2", "search", "[1.0,0.0]", "1")
        require_json_text(operations["reopen_read"], "persisted")
    else:
        operations["create"] = run(binary, path, "-c", "CREATE TABLE notes (id INTEGER PRIMARY KEY, body TEXT)")
        operations["insert"] = run(binary, path, "-c", "INSERT INTO notes VALUES (1, 'persisted')")
        operations["reopen_read"] = run(binary, path, "--json", "SELECT body FROM notes WHERE id = 1")
        require_json_text(operations["reopen_read"], "persisted")
    if not database.is_file():
        raise RuntimeError(f"No persisted database was created by {name}")
    return {
        "database_bytes": database.stat().st_size,
        "operations": {
            step: {key: value for key, value in result.items() if key != "stdout"}
            for step, result in operations.items()
        },
        "peak_working_set_bytes": max(
            (result["peak_working_set_bytes"] for result in operations.values() if result["peak_working_set_bytes"] is not None),
            default=None,
        ),
        "reopen_verified": True,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--profile", choices=("release", "min-size"), default="min-size")
    parser.add_argument("--no-build", action="store_true", help="Use existing binaries; source provenance will be unverified")
    parser.add_argument("--output", type=Path, default=ROOT / "target" / "edition-footprint.json")
    args = parser.parse_args()

    bin_dir = ROOT / "target" / args.profile
    package_version = re.search(r'^version\s*=\s*"([^"]+)"', (ROOT / "Cargo.toml").read_text(encoding="utf-8"), re.MULTILINE)
    if package_version is None:
        raise RuntimeError("Could not read the TapirusDB package version")
    if not args.no_build:
        for name, (binary_name, feature) in EDITIONS.items():
            command = ["cargo", "build", "--locked", "--offline", "--profile", args.profile, "-p", "tapirus"]
            if feature:
                command += ["--no-default-features", "--features", feature]
            command += ["--bin", binary_name]
            print(f"Building {name}: {' '.join(command)}", flush=True)
            subprocess.run(command, cwd=ROOT, check=True)

    report = {
        "scope": "Local CLI binary footprint and small persisted reopen smoke; not device qualification or performance comparison",
        "host": {"os": platform.system(), "arch": platform.machine(), "python": platform.python_version()},
        "source_head": output(["git", "rev-parse", "HEAD"]),
        "source_dirty": bool(output(["git", "status", "--porcelain"])),
        "rustc": output(["rustc", "--version"]),
        "profile": args.profile,
        "built_in_this_run": not args.no_build,
        "editions": {},
    }
    with tempfile.TemporaryDirectory(prefix="tapirus-footprint-") as temporary:
        for name, (binary_name, _) in EDITIONS.items():
            binary = bin_dir / (binary_name + (".exe" if os.name == "nt" else ""))
            if not binary.is_file():
                raise FileNotFoundError(binary)
            version = run(binary, "--version")["stdout"]
            if package_version.group(1) not in version:
                raise RuntimeError(f"Unexpected {name} version: {version}")
            entry = {
                "version": version,
                "binary_bytes": binary.stat().st_size,
                "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
                **smoke(name, binary, Path(temporary)),
            }
            report["editions"][name] = entry
            print(f"{name}: {entry['binary_bytes']} bytes; peak working set {entry['peak_working_set_bytes']}; reopen verified", flush=True)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(f"Report: {args.output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
