"""Installed-wheel consumer checks for lifecycle, sessions, migration and agents."""
import json
from pathlib import Path
import sqlite3
import tempfile
from contextlib import contextmanager
import tapirus
from tapirus_tools.agent import ingest, answer
from tapirus_tools.embeddings import Embedder
from tapirus_tools.evaluate import evaluate
from tapirus_tools.migrate import import_sqlite, export_sqlite, MigrationError


@contextmanager
def sqlite_connection(path):
    connection = sqlite3.connect(path)
    try:
        with connection:
            yield connection
    finally:
        connection.close()


def rejects(action):
    try:
        action()
    except (ValueError, tapirus.TapirusError):
        return
    raise AssertionError("Expected rejection")


def main():
    with tempfile.TemporaryDirectory(prefix="tapirus-ai-tools-") as directory:
        root = Path(directory)
        limited_path = root / "limited.tapir"
        with tapirus.connect(str(limited_path), memory_config={"max_documents": 2}) as limited:
            limited.store_memory_batch([{"content": "first"}, {"content": "second"}])
        rejects(lambda: tapirus.connect(str(limited_path), memory_config={"max_documents": 1}))
        with tapirus.connect(str(limited_path), memory_config={"max_documents": 2}) as limited:
            rejects(lambda: limited.store_memory({"content": "third"}))
        with tapirus.connect(":memory:", memory_config={"max_documents": 1}) as limited:
            limited.store_memory({"content": "first"})
            rejects(lambda: limited.store_memory({"content": "second"}))
        with tapirus.connect(str(root / "lifecycle.tapir")) as db:
            doc = {"content": "Engine maintenance guide", "external_id": "a", "embedding": [1, 0],
                   "embedding_model": "fixture-v1", "source_version": "v1", "namespace": "sdk"}
            source = db.upsert_memory(doc)
            assert db.upsert_memory(dict(doc, content="Updated engine maintenance", expected_revision=1)) == source
            rejects(lambda: db.upsert_memory(dict(doc, expected_revision=1)))
            rejects(lambda: db.upsert_memory_batch([dict(doc, external_id="new"), dict(doc, embedding=[1, 0, 0])]))
            assert db.memory_stats()["documents"] == 1
            assert db.prepare_memory_indexes()["indexed_vectors"] == 1
            result = db.search_memories_detailed({"embedding": [1, 0], "embedding_model": "fixture-v1"})
            assert result["results"][0]["entry"]["revision"] == 2
            db.store_memory({"content": "Expired source", "namespace": "old", "expires_at": 1})
            assert not db.search_memories({"query": "Expired", "namespace": "old"})
            assert db.purge_expired_memories() == 1
            assert db.storage_health()["reopen_required"] is False
        sessions = tapirus.SessionDatabase.open(str(root / "sessions.tapir"))
        a, b = sessions.session(), sessions.session()
        a.execute("CREATE TABLE session_records (id INTEGER)")
        a.begin()
        a.execute("INSERT INTO session_records VALUES (1)")
        rejects(lambda: b.query("SELECT * FROM session_records"))
        rejects(b.commit)
        a.rollback()
        with b:
            b.execute("INSERT INTO session_records VALUES (2)")
            b.upsert_memory({"external_id": "m", "content": "session source"})
        assert a.memory_stats()["documents"] == 1
        try:
            with a:
                a.execute("INSERT INTO session_records VALUES (3)")
                raise RuntimeError("abort")
        except RuntimeError:
            pass
        assert b.query("SELECT * FROM session_records") == [{"id": 2}]
        a.begin()
        a.execute("INSERT INTO session_records VALUES (4)")
        a.close()
        assert b.query("SELECT * FROM session_records") == [{"id": 2}]
        b.close()
        sessions.close()
        sqlite_path = root / "source.sqlite"
        rows = [(1, "O'Reilly ? ; -- 雨", 9007199254740993, b"\x00\xff", 1.25),
                (2, None, -9223372036854775808, b"", None),
                (3, "duplicate", 9223372036854775807, None, -3.5)]
        with sqlite_connection(sqlite_path) as sql:
            sql.execute("CREATE TABLE Samples (id INTEGER PRIMARY KEY, note TEXT, big INTEGER, raw BLOB, value REAL)")
            sql.executemany("INSERT INTO Samples VALUES (?, ?, ?, ?, ?)", rows)
        target = root / "migrated.tapir"
        dry = import_sqlite(sqlite_path, target, dry_run=True)
        assert dry["compatible"] and not target.exists(), dry
        report = import_sqlite(sqlite_path, target)
        assert report["verified"]["Samples"]["rows"] == 3
        roundtrip = root / "roundtrip.sqlite"
        export = export_sqlite(target, roundtrip)
        assert next(iter(export["verified"].values()))["rows"] == 3
        with sqlite_connection(roundtrip) as sql:
            assert sql.execute("SELECT * FROM samples ORDER BY id").fetchall() == rows
        rejects(lambda: import_sqlite(sqlite_path, target))
        busy = root / "busy.tapir"
        busy.with_suffix(".tapir-lock").write_text(str(__import__("os").getpid()))
        rejects(lambda: import_sqlite(sqlite_path, busy))
        assert not busy.exists() and busy.with_suffix(".tapir-lock").is_file()
        orphan = root / "orphan.tapir"
        orphan.with_suffix(".tapir-wal").write_bytes(b"existing unrelated WAL")
        rejects(lambda: import_sqlite(sqlite_path, orphan))
        assert not orphan.exists() and not orphan.with_suffix(".tapir-lock").exists()
        incompatible = root / "incompatible.sqlite"
        with sqlite_connection(incompatible) as sql:
            sql.execute("CREATE TABLE guarded (id INTEGER PRIMARY KEY, note TEXT CHECK(length(note)>0))")
            sql.execute("CREATE VIEW guarded_view AS SELECT * FROM guarded")
        refused = import_sqlite(incompatible, root / "refused.tapir")
        assert not refused["compatible"] and len(refused["issues"]) == 2
        assert not (root / "refused.tapir").exists()
        dynamic = root / "dynamic.sqlite"
        with sqlite_connection(dynamic) as sql:
            sql.execute("CREATE TABLE mixed (number INTEGER)")
            sql.execute("INSERT INTO mixed VALUES ('not an integer')")
        rejects(lambda: import_sqlite(dynamic, root / "dynamic.tapir"))
        assert not (root / "dynamic.tapir").exists()
        manual = root / "manual.txt"
        manual.write_text("Engine calibration requires an empty chamber. " * 15, encoding="utf-8")
        embedder = Embedder("hash", dimensions=64)
        with tapirus.connect(str(root / "agent.tapir")) as db:
            first = ingest(db, [manual], embedder, chunk_size=120, overlap=20)
            assert first["chunks"] > 1
            manual.write_text("Engine calibration uses reference sensor R7.", encoding="utf-8")
            updated = ingest(db, [manual], embedder, chunk_size=120, overlap=20)
            assert updated["chunks"] == 1 and updated["removed_stale_chunks"] == first["chunks"] - 1
            result = answer(db, "Engine calibration reference sensor", embedder)
            assert len(result["sources"]) == 1 and "R7" in result["sources"][0]["content"]
            assert result["embedding_model"] == "tapirus-tools/sha256-word-v1/64"
            assert db.memory_stats()["documents"] == 1
        # Exercise actual benchmark output and RSS collection with the installed SDK.
        dataset = Path(__file__).resolve().parents[1] / "eval/device-memory-dev-v1.json"
        report = evaluate(dataset, root / "eval.json", Embedder("lexical"))
        assert report["documents"] == 20 and report["metrics"]["queries"] == 40
        assert report["peak_process_rss_bytes"] > 0
        print(json.dumps({"version": tapirus.__version__, "checks": "lifecycle/revision/atomic batch, sessions, verified SQLite scalar roundtrip/rejection/atomicity, updated agent chunks, source retrieval, evaluation and RSS"}))


if __name__ == "__main__":
    main()
