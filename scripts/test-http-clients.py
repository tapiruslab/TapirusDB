"""Run Go/PHP against an actual CLI daemon and verify data after process restart."""
import argparse
from contextlib import contextmanager
import json
import os
from pathlib import Path
import secrets
import socket
import subprocess
import tempfile
import time
import tomllib
from urllib.request import Request, urlopen

ROOT = Path(__file__).resolve().parents[1]


def api(endpoint, token, sql):
    request = Request(endpoint + "/api/sql", data=json.dumps({"sql": sql, "params": []}).encode(), headers={"Content-Type": "application/json", "Authorization": "Bearer " + token})
    with urlopen(request, timeout=10) as response:
        return json.load(response)


@contextmanager
def daemon(binary, database, token, log):
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        port = probe.getsockname()[1]
    process = subprocess.Popen([str(binary), "serve", "--host", "127.0.0.1", "--port", str(port), "--api-key", token, str(database)], cwd=ROOT, stdout=log, stderr=log)
    endpoint = f"http://127.0.0.1:{port}"
    try:
        deadline = time.monotonic() + 20
        while True:
            if process.poll() is not None:
                raise RuntimeError(f"Daemon exited with status {process.returncode}")
            try:
                with urlopen(endpoint + "/health", timeout=1) as response:
                    if json.load(response).get("status") == "ok":
                        break
            except OSError:
                pass
            if time.monotonic() >= deadline:
                raise RuntimeError("Daemon readiness timed out")
            time.sleep(0.1)
        yield endpoint
    finally:
        if process.poll() is None:
            process.terminate()
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=10)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=ROOT / ("target/debug/tapirus.exe" if os.name == "nt" else "target/debug/tapirus"))
    parser.add_argument("--go", default="go")
    parser.add_argument("--php", default="php")
    parser.add_argument("--php-extension-dir")
    args = parser.parse_args()
    version = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))["package"]["version"]
    token = secrets.token_hex(24)
    with tempfile.TemporaryDirectory(prefix="tapirus-http-clients-") as directory, tempfile.TemporaryFile() as log:
        database = Path(directory) / "consumer.tapir"
        environment = {**os.environ, "TAPIRUS_TEST_TOKEN": token, "TAPIRUS_TEST_VERSION": version, "GOCACHE": str(ROOT / "target/go-build-cache"), "GOPATH": str(ROOT / "target/go-path"), "GOTOOLCHAIN": "local"}
        with daemon(args.binary.resolve(), database, token, log) as endpoint:
            environment["TAPIRUS_TEST_ENDPOINT"] = endpoint
            subprocess.run([args.go, "test", "-count=1", "-v", "./..."], cwd=ROOT / "sdks/go", env=environment, check=True)
            php = [args.php]
            if args.php_extension_dir:
                php += ["-d", "extension_dir=" + args.php_extension_dir, "-d", "extension=curl"]
            subprocess.run([*php, str(ROOT / "sdks/php/test.php")], cwd=ROOT, env=environment, check=True)
        with daemon(args.binary.resolve(), database, token, log) as endpoint:
            for table in ("go_consumer", "php_consumer"):
                rows = api(endpoint, token, f"SELECT * FROM {table}")["rows"]
                if rows != [{"id": 1, "note": "quote ' ? ; -- source", "big": 9007199254740993}]:
                    raise AssertionError(f"{table} did not survive process restart: {rows}")
        print(f"Go and PHP clients: native daemon {version}, independent process restart and exact persisted rows passed.")


if __name__ == "__main__":
    main()
