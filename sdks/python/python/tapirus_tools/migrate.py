"""Conservative, verified SQLite migration. No third-party Python dependencies.

The accepted subset is deliberately explicit: ordinary tables with INTEGER,
REAL, TEXT or BLOB columns, NOT NULL and one INTEGER PRIMARY KEY. Other schema
features are reported, never silently discarded. Hidden SQLite rowids, indexes,
triggers and SQLite's dynamic type semantics are not portable contracts.
"""
import argparse
from contextlib import closing
import hashlib
import json
import math
import os
from pathlib import Path
import re
import sqlite3
import struct
import tempfile


class MigrationError(ValueError):
    pass


def identifier(name):
    if not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", name):
        raise MigrationError(f"Identifier requires manual mapping: {name!r}")
    return name


def quote(name):
    return '"' + name.replace('"', '""') + '"'


def connect_sqlite(path):
    path = Path(path).resolve(strict=True)
    connection = sqlite3.connect(path.as_uri() + "?mode=ro", uri=True)
    connection.execute("PRAGMA query_only=ON")
    connection.execute("BEGIN")  # Hold one consistent source snapshot through verification.
    return connection


def inspect_source(source):
    tables, issues = [], []
    for kind, name, sql in source.execute(
        "SELECT type,name,sql FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%' ORDER BY name"
    ):
        if kind != "table":
            issues.append(f"{kind} {name}: not supported by automatic migration")
            continue
        try:
            identifier(name)
            if not sql or not re.match(r"\s*CREATE\s+TABLE\s+", sql, re.I):
                raise MigrationError("virtual or missing table definition")
            # Validate every column clause; reject any unrecognized semantics.
            opening, ending = sql.find("("), sql.rfind(")")
            if opening < 0 or ending < opening or sql[ending + 1:].strip(" ;\t\r\n"):
                raise MigrationError("unsupported table options")
            columns = []
            for clause in sql[opening + 1:ending].split(","):
                match = re.fullmatch(
                    r'\s*(?:"([A-Za-z_][A-Za-z0-9_]*)"|([A-Za-z_][A-Za-z0-9_]*))\s+'
                    r'(INTEGER|REAL|TEXT|BLOB)((?:\s+(?:NOT\s+NULL|PRIMARY\s+KEY))*)\s*',
                    clause, re.I,
                )
                if not match:
                    raise MigrationError("unsupported type, constraint, default or column expression")
                column, dtype, constraints = match[1] or match[2], match[3].upper(), match[4].upper()
                if constraints.count("PRIMARY") > 1 or constraints.count("NOT") > 1:
                    raise MigrationError("duplicate constraints")
                primary = "PRIMARY" in constraints
                if primary and dtype != "INTEGER":
                    raise MigrationError("only INTEGER PRIMARY KEY is portable")
                columns.append({"name": column, "type": dtype, "primary_key": primary,
                                "not_null": "NOT" in constraints})
            if sum(c["primary_key"] for c in columns) > 1:
                raise MigrationError("composite primary keys are unsupported")
            metadata = source.execute(f"PRAGMA table_xinfo({quote(name)})").fetchall()
            if len(metadata) != len(columns) or any(c[6] for c in metadata):
                raise MigrationError("hidden or generated columns are unsupported")
            if [c[1].lower() for c in metadata] != [c["name"].lower() for c in columns]:
                raise MigrationError("column metadata mismatch")
            if source.execute(f"PRAGMA foreign_key_list({quote(name)})").fetchall():
                raise MigrationError("foreign keys are unsupported")
            indexes = source.execute(f"PRAGMA index_list({quote(name)})").fetchall()
            if indexes:
                raise MigrationError("indexes and UNIQUE constraints require manual migration")
            canonical = "CREATE TABLE " + name + " (" + ", ".join(
                c["name"] + " " + c["type"] + (" PRIMARY KEY" if c["primary_key"] else "")
                + (" NOT NULL" if c["not_null"] else "") for c in columns
            ) + ")"
            tables.append({"name": name, "columns": columns, "ddl": canonical})
        except MigrationError as error:
            issues.append(f"table {name}: {error}")
    return {"compatible": not issues, "tables": tables, "issues": issues,
            "scope": "named scalar columns; no hidden rowid or SQLite semantic equivalence"}


def encoded_row(values):
    out = bytearray()
    for value in values:
        if value is None:
            payload, tag = b"", b"n"
        elif type(value) is int:
            payload, tag = struct.pack(">q", value), b"i"
        elif type(value) is float and math.isfinite(value):
            payload, tag = struct.pack(">d", value), b"f"
        elif type(value) is str:
            payload, tag = value.encode("utf-8"), b"s"
        elif type(value) is bytes:
            payload, tag = value, b"b"
        else:
            raise MigrationError("Nonportable or nonfinite source value")
        out.extend(tag + len(payload).to_bytes(8, "big") + payload)
    return out


class Fingerprint:
    """Order-independent multiset of type-aware row hashes, bounded memory."""
    def __init__(self):
        self.count, self.total, self.squares = 0, 0, 0

    def add(self, values):
        digest = int.from_bytes(hashlib.sha256(encoded_row(values)).digest(), "big")
        self.count += 1
        self.total = (self.total + digest) % (1 << 256)
        self.squares = (self.squares + digest * digest) % (1 << 256)

    def result(self):
        return {"rows": self.count, "sha256_sum": f"{self.total:064x}",
                "sha256_square_sum": f"{self.squares:064x}"}


def validate_values(table, row):
    allowed = {"INTEGER": int, "REAL": float, "TEXT": str, "BLOB": bytes}
    for column, value in zip(table["columns"], row):
        if value is not None and type(value) is not allowed[column["type"]]:
            raise MigrationError(f"{table['name']}.{column['name']}: SQLite dynamic value does not match declared type")
    encoded_row(row)


def promote_new(staged, destination, native=False):
    # Hard-link creation is atomic and fails if a destination appeared meanwhile.
    # Staging is on the same filesystem. Never overwrite a user's destination.
    lock_path = destination.with_suffix(".tapir-lock") if native else None
    if lock_path is not None:
        try:
            descriptor = os.open(lock_path, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
        except FileExistsError as error:
            raise MigrationError("Native destination has an existing lock") from error
        try:
            with os.fdopen(descriptor, "w") as lock:
                lock.write(str(os.getpid()))
                lock.flush()
                if destination.with_suffix(".tapir-wal").exists():
                    raise MigrationError("Native destination has an existing WAL sidecar")
                os.link(staged, destination)
        finally:
            lock_path.unlink()
    else:
        os.link(staged, destination)
    if os.name != "nt":
        descriptor = os.open(destination.parent, os.O_RDONLY)
        try:
            os.fsync(descriptor)
        finally:
            os.close(descriptor)


def import_sqlite(source_path, destination, dry_run=False, batch_size=256):
    import tapirus
    if not 1 <= batch_size <= 10000:
        raise MigrationError("batch_size must be between 1 and 10000")
    destination = Path(destination).resolve()
    if destination.exists():
        raise MigrationError("Destination already exists")
    with closing(connect_sqlite(source_path)) as source:
        report = inspect_source(source)
        # Ask the actual target parser to check identifiers and the full DDL.
        with tapirus.connect(":memory:") as probe:
            for table in report["tables"]:
                try:
                    probe.execute(table["ddl"])
                except tapirus.TapirusError as error:
                    report["issues"].append(f"{table['name']}: target DDL rejected: {error}")
        report["compatible"] = not report["issues"]
        if dry_run or not report["compatible"]:
            return report
        if not destination.parent.is_dir():
            raise MigrationError("Destination parent directory does not exist")
        with tempfile.TemporaryDirectory(prefix=".tapirus-migrate-", dir=destination.parent) as directory:
            staged = Path(directory) / "import.tapir"
            fingerprints = {}
            with tapirus.connect(str(staged)) as target:
                target.execute("BEGIN")
                try:
                    for table in report["tables"]:
                        target.execute(table["ddl"])
                        name, columns = table["name"], [c["name"] for c in table["columns"]]
                        read = source.execute(f"SELECT {','.join(map(quote, columns))} FROM {quote(name)}")
                        insert = f"INSERT INTO {name} ({','.join(columns)}) VALUES ({','.join('?' for _ in columns)})"
                        fingerprint = Fingerprint()
                        while batch := read.fetchmany(batch_size):
                            for row in batch:
                                validate_values(table, row)
                                target.execute(insert, list(row))
                                fingerprint.add(row)
                        fingerprints[name] = fingerprint.result()
                    target.execute("COMMIT")
                except BaseException:
                    target.execute("ROLLBACK")
                    raise
            # Verify committed, reopened data before making the destination visible.
            with tapirus.connect(str(staged)) as target:
                integrity = target.integrity_check()
                if integrity.get("errors"):
                    raise MigrationError("Imported database failed integrity check")
                for table in report["tables"]:
                    actual = Fingerprint()
                    # Native query currently materializes a table; source ingestion streams.
                    for row in target.query(f"SELECT * FROM {table['name']}"):
                        actual.add([row[c["name"].lower()] for c in table["columns"]])
                    if actual.result() != fingerprints[table["name"]]:
                        raise MigrationError(f"Verification mismatch for {table['name']}")
            promote_new(staged, destination, native=True)
        report.update({"destination": str(destination), "verified": fingerprints,
                       "version": tapirus.__version__})
        return report


def export_sqlite(source_path, destination):
    """Export scalar SQL tables; reject non-scalar models and existing targets."""
    import tapirus
    destination = Path(destination).resolve()
    if destination.exists():
        raise MigrationError("Destination already exists")
    with tapirus.connect(str(Path(source_path).resolve(strict=True))) as source:
        schema = source.schema()
        for table in schema:
            identifier(table["name"])
            for column in table["columns"]:
                identifier(column["name"])
                if column["data_type"] not in ("Integer", "Real", "Text", "Blob"):
                    raise MigrationError("Only scalar SQL tables can be exported")
        with tempfile.TemporaryDirectory(prefix=".tapirus-export-", dir=destination.parent) as directory:
            staged = Path(directory) / "export.sqlite"
            fingerprints = {}
            with closing(sqlite3.connect(staged)) as target:
                target.execute("BEGIN")
                for table in schema:
                    columns = table["columns"]
                    ddl = ", ".join(quote(c["name"]) + " " + c["data_type"].upper()
                                    + (" PRIMARY KEY" if c["primary_key"] else "")
                                    + (" NOT NULL" if c["not_null"] else "") for c in columns)
                    target.execute(f"CREATE TABLE {quote(table['name'])} ({ddl})")
                    fingerprint = Fingerprint()
                    for row in source.query(f"SELECT * FROM {table['name']}"):
                        values = [row[c["name"]] for c in columns]
                        target.execute(f"INSERT INTO {quote(table['name'])} VALUES ({','.join('?' for _ in columns)})", values)
                        fingerprint.add(values)
                    fingerprints[table["name"]] = fingerprint.result()
                target.commit()
            with closing(connect_sqlite(staged)) as verified:
                for table in schema:
                    actual = Fingerprint()
                    for row in verified.execute(f"SELECT * FROM {quote(table['name'])}"):
                        actual.add(row)
                    if actual.result() != fingerprints[table["name"]]:
                        raise MigrationError("Export verification mismatch")
            with staged.open("r+b") as file:
                os.fsync(file.fileno())
            promote_new(staged, destination)
    return {"destination": str(destination), "verified": fingerprints,
            "scope": "scalar SQL tables only; vector/document/graph/memory models are not exported"}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("direction", choices=["import", "export"])
    parser.add_argument("source", type=Path)
    parser.add_argument("destination", type=Path)
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args(argv)
    try:
        if args.direction == "export" and args.dry_run:
            parser.error("--dry-run is available for import")
        result = (import_sqlite(args.source, args.destination, args.dry_run)
                  if args.direction == "import" else export_sqlite(args.source, args.destination))
        print(json.dumps(result, indent=2))
        return 0 if result.get("compatible", True) else 2
    except (MigrationError, OSError, sqlite3.Error) as error:
        parser.exit(2, f"Migration failed: {error}\n")


if __name__ == "__main__":
    raise SystemExit(main())
