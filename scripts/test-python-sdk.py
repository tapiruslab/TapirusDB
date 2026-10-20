"""Exercise the installed native wheel, including a separate writer process."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import zipfile


def raises(error, action):
    try:
        action()
    except error:
        return
    raise AssertionError(f"Expected {error}")


def exercise(path, writer=False):
    import tapirus

    expected_version = json.loads((Path(__file__).resolve().parents[1] / "sdks/nodejs/package.json").read_text())["version"]
    assert tapirus.__version__ == expected_version
    if writer:
        db = tapirus.connect(str(path))
        db.execute("CREATE TABLE records (id INTEGER PRIMARY KEY, note TEXT, big INTEGER, raw BLOB)")
        db.execute("INSERT INTO records VALUES (?, ?, ?, ?)", [1, "quote ' ? ; -- <script>", 9007199254740993, b"\x00\xff"])
        db.store_memory({"content": "Tapirs live in tropical forests", "embedding": [1.0, 0.0], "namespace": "wildlife", "session_id": "alpha", "tags": ["source:manual"]})
        db.store_memory({"content": "Separate namespace", "embedding": [1.0, 0.0], "namespace": "private"})
        collection = db.collection("docs")
        doc_id = collection.insert_one({"title": "original", "n": 7})
        assert collection.find_by_id(doc_id)["title"] == "original"
        assert collection.update(doc_id, {"title": "updated"})
        assert collection.find_one({"title": "updated"})["title"] == "updated"
        assert collection.count() == 1
        db.execute("BEGIN")
        db.store_memory({"content": "must rollback", "namespace": "rolledback"})
        db.query("ROLLBACK")
        assert db.search_memories({"query": "rollback", "namespace": "rolledback"}) == []
        raises(ValueError, lambda: db.execute("SELECT ?", {"wrong": 1}))
        raises(ValueError, lambda: db.execute("SELECT ?", [float("nan")]))
        raises(OverflowError, lambda: db.execute("SELECT ?", [2**100]))
        raises(ValueError, lambda: db.store_memory({"content": "bad", "embedding": [float("nan")]}))
        raises(ValueError, lambda: db.vector_search("records; DROP TABLE records", "embedding", [1.0]))
        raises(ValueError, lambda: db.graph_algorithm("PAGERANK; DROP TABLE records"))
        db.close()
        db.close()
        raises(tapirus.ConnectionError, lambda: db.query("SELECT 1"))
        raises(tapirus.ConnectionError, collection.count)
        return
    with tapirus.connect(str(path)) as db:
        rows = db.query("SELECT * FROM records WHERE id = ?", (1,))
        assert rows == [{"id": 1, "note": "quote ' ? ; -- <script>", "big": 9007199254740993, "raw": b"\x00\xff"}], rows
        assert db.query("SELECT COUNT(*) AS count FROM records") == [{"count": 1}]
        assert db.query("SELECT '?' AS literal")[0]["literal"] == "?"
        assert db.collection("docs").find_all()[0]["title"] == "updated"
        found = db.search_memories({"embedding": [1.0, 0.0], "namespace": "wildlife", "session_id": "alpha", "tags": ["source:manual"]})
        assert len(found) == 1 and found[0]["entry"]["content"] == "Tapirs live in tropical forests", found
        assert "semantic_score" in found[0]
        assert db.forget_memory(found[0]["entry"]["id"])
        assert db.search_memories({"query": "tapirs", "namespace": "wildlife"}) == []
        db.execute("CREATE TABLE vectors (id INTEGER PRIMARY KEY, embedding VECTOR(2))")
        db.execute("INSERT INTO vectors VALUES (?, ?)", [1, [1.0, 0.0]])
        assert db.vector_search("vectors", "embedding", [1.0, 0.0], 1)[0]["id"] == 1
    raises(tapirus.ConnectionError, lambda: db.execute("SELECT 1"))
    encrypted = str(path.with_name("encrypted.tapir"))
    with tapirus.connect(encrypted, "local-test-password") as db:
        db.execute("CREATE TABLE secrets (id INTEGER)")
        db.execute("INSERT INTO secrets VALUES (?)", [7])
    raises(tapirus.ConnectionError, lambda: tapirus.connect(encrypted, "wrong-password"))
    with tapirus.connect(encrypted, "local-test-password") as db:
        assert db.query("SELECT id FROM secrets") == [{"id": 7}]
    print(json.dumps({"python": sys.version.split()[0], "nativeModule": tapirus.__file__, "version": tapirus.__version__, "checks": "SQL parameters, exact integers, blobs, vectors, AI scopes/rollback, collection lifecycle, close, process reopen, encryption"}))


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--wheel", type=Path)
    parser.add_argument("--child", type=Path)
    parser.add_argument("--reader", type=Path)
    args = parser.parse_args()
    if args.wheel and args.wheel.is_dir():
        expected_version = json.loads((Path(__file__).resolve().parents[1] / "sdks/nodejs/package.json").read_text())["version"]
        wheels = list(args.wheel.glob(f"tapirus-{expected_version}-*.whl"))
        if len(wheels) != 1:
            raise SystemExit(f"Expected exactly one {expected_version} wheel in the directory")
        args.wheel = wheels[0]
    if args.child or args.reader:
        exercise(args.child or args.reader, writer=bool(args.child))
        return
    with tempfile.TemporaryDirectory(prefix="tapirus-python-consumer-") as directory:
        destination = Path(directory)
        env = os.environ.copy()
        if args.wheel:
            with zipfile.ZipFile(args.wheel) as archive:
                normalize_license = lambda content: b"\n".join(line.rstrip() for line in content.replace(b"\r\n", b"\n").split(b"\n"))
                assert normalize_license(archive.read("CORE_LICENSE")) == normalize_license((Path(__file__).resolve().parents[1] / "LICENSE").read_bytes())
                assert any(name.endswith((".pyd", ".so")) for name in archive.namelist())
            subprocess.run([sys.executable, "-m", "pip", "install", "--no-index", "--no-deps", "--target", str(destination / "package"), str(args.wheel.resolve())], check=True)
            env["PYTHONPATH"] = str(destination / "package")
        script = str(Path(__file__).resolve())
        database = str(destination / "consumer.tapir")
        for mode in ["--child", "--reader"]:
            subprocess.run([sys.executable, script, mode, database], cwd=destination, env=env, check=True)
        subprocess.run([sys.executable, str(Path(script).with_name("test-ai-tools.py"))], cwd=destination, env=env, check=True)


if __name__ == "__main__":
    main()
