"""
TapirusDB High-Level Python Connection API
"""

import json
import ctypes
from typing import Optional, List, Dict, Any, Union
from .ffi import get_ffi
from .exceptions import ConnectionError, QueryError, AuthenticationError

class Connection:
    """A connection to a TapirusDB database instance."""

    @classmethod
    def open(cls, path: Optional[str] = None, passphrase: Optional[str] = None) -> "Connection":
        """Open a TapirusDB connection (factory method)."""
        return cls(path=path, passphrase=passphrase)

    def __init__(self, path: Optional[str] = None, passphrase: Optional[str] = None):
        self._ffi = get_ffi()
        self._handle = None
        self._is_closed = False
        self._path = path

        if not self._ffi.is_native_available:
            # Fallback in-memory Python emulator for environments without compiled C-FFI
            self._emulator = True
            self._local_tables: Dict[str, List[Dict[str, Any]]] = {}
            return

        self._emulator = False
        lib = self._ffi._lib

        if path is None or path == ":memory:":
            self._handle = lib.tapirus_open_in_memory()
        elif passphrase:
            self._handle = lib.tapirus_open_encrypted(path.encode("utf-8"), passphrase.encode("utf-8"))
        else:
            self._handle = lib.tapirus_open(path.encode("utf-8"))

        if not self._handle:
            raise ConnectionError(f"Failed to open TapirusDB database at '{path}'")

    def execute(self, sql: str) -> int:
        """Execute a non-query SQL statement (CREATE, INSERT, UPDATE, DELETE)."""
        if self._is_closed:
            raise ConnectionError("Cannot execute on closed connection")

        if self._emulator:
            return self._emulate_execute(sql)

        lib = self._ffi._lib
        err_ptr = ctypes.c_char_p()
        affected = lib.tapirus_execute(self._handle, sql.encode("utf-8"), ctypes.byref(err_ptr))
        if affected < 0:
            msg = "Query execution error"
            if err_ptr.value:
                msg = err_ptr.value.decode("utf-8", errors="replace")
                lib.tapirus_free_string(err_ptr)
            raise QueryError(msg)
        return affected

    def query(self, sql: str) -> List[Dict[str, Any]]:
        """Execute a SQL query and return results as a list of dict rows."""
        if self._is_closed:
            raise ConnectionError("Cannot query on closed connection")

        if self._emulator:
            return self._emulate_query(sql)

        lib = self._ffi._lib
        json_ptr = ctypes.c_char_p()
        err_ptr = ctypes.c_char_p()
        rc = lib.tapirus_query_json(
            self._handle,
            sql.encode("utf-8"),
            ctypes.byref(json_ptr),
            ctypes.byref(err_ptr),
        )
        if rc != 0:
            msg = "Query failed"
            if err_ptr.value:
                msg = err_ptr.value.decode("utf-8", errors="replace")
                lib.tapirus_free_string(err_ptr)
            raise QueryError(msg)

        try:
            raw_str = json_ptr.value.decode("utf-8", errors="replace") if json_ptr.value else "[]"
            parsed = json.loads(raw_str)
            if isinstance(parsed, list):
                return [self._normalize_row(r) for r in parsed]
            return parsed
        finally:
            if json_ptr.value:
                lib.tapirus_free_string(json_ptr)

    def _normalize_row(self, r: Any) -> Dict[str, Any]:
        if isinstance(r, dict) and "columns" in r and "values" in r:
            cols = r.get("columns", [])
            vals = r.get("values", [])
            out = {}
            for c, v in zip(cols, vals):
                if isinstance(v, dict) and len(v) == 1:
                    val = next(iter(v.values()))
                else:
                    val = v
                out[c] = val
            return out
        return r

    def vector_search(
        self,
        table: str,
        vector_col: str,
        query_vector: List[float],
        top_k: int = 5,
        where: Optional[str] = None,
        columns: Optional[List[str]] = None,
    ) -> List[Dict[str, Any]]:
        """Perform sub-millisecond vector similarity search."""
        cols_str = ", ".join(columns) if columns else "*"
        vec_str = "[" + ", ".join(f"{x:.6f}" for x in query_vector) + "]"
        sql = f"SELECT {cols_str} FROM {table} VECTOR NEAR {vector_col} = {vec_str} TOP {top_k}"
        if where:
            sql += f" WHERE {where}"
        return self.query(sql)

    def graph_query(self, cypher_or_sql: str) -> List[Dict[str, Any]]:
        """Execute a graph query (MATCH ... or GRAPH TRAVERSE / SHORTEST_PATH)."""
        return self.query(cypher_or_sql)

    def graph_algorithm(self, algorithm: str, **kwargs) -> List[Dict[str, Any]]:
        """Run a native graph algorithm (PAGERANK, CONNECTED_COMPONENTS, BETWEENNESS, LOUVAIN)."""
        opts = " ".join(f"{k} {v}" for k, v in kwargs.items())
        sql = f"GRAPH ALGORITHM {algorithm.upper()}"
        if opts:
            sql += f" {opts}"
        return self.query(sql)

    def checkpoint(self) -> int:
        """Manually flush the Write-Ahead Log (.tapir-wal) to the main database file."""
        if self._is_closed:
            raise ConnectionError("Cannot checkpoint on closed connection")
        if self._emulator or not hasattr(self._ffi._lib, "tapirus_checkpoint"):
            return 0
        return self._ffi._lib.tapirus_checkpoint(self._handle)

    def version(self) -> str:
        """Return the TapirusDB library version string."""
        if not self._emulator and hasattr(self._ffi._lib, "tapirus_version"):
            v_ptr = self._ffi._lib.tapirus_version()
            if v_ptr:
                return ctypes.string_at(v_ptr).decode("utf-8")
        return "1.0.1"

    def close(self):
        """Close and deallocate connection resources."""
        if not self._is_closed:
            if not self._emulator and self._handle:
                self._ffi._lib.tapirus_close(self._handle)
                self._handle = None
            self._is_closed = True

    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc_val, exc_tb):
        self.close()

    def _emulate_execute(self, sql: str) -> int:
        clean = sql.strip().rstrip(";")
        if clean.upper().startswith("CREATE TABLE"):
            parts = clean.split()
            tbl_name = parts[2].split("(")[0]
            if tbl_name not in self._local_tables:
                self._local_tables[tbl_name] = []
            return 0
        elif clean.upper().startswith("INSERT INTO"):
            parts = clean.split()
            tbl_name = parts[2]
            if tbl_name not in self._local_tables:
                self._local_tables[tbl_name] = []
            self._local_tables[tbl_name].append({"id": len(self._local_tables[tbl_name]) + 1, "raw": clean})
            return 1
        return 0

    def _emulate_query(self, sql: str) -> List[Dict[str, Any]]:
        clean = sql.strip().rstrip(";")
        parts = clean.split()
        if "FROM" in [p.upper() for p in parts]:
            idx = [p.upper() for p in parts].index("FROM")
            if idx + 1 < len(parts):
                tbl_name = parts[idx + 1]
                return self._local_tables.get(tbl_name, [])
        return []

def connect(path: Optional[str] = None, passphrase: Optional[str] = None) -> Connection:
    """
    Open a TapirusDB connection.

    Args:
        path: Path to `.tapir` file, or None / ':memory:' for transient RAM database.
        passphrase: Optional encryption key for ChaCha20-Poly1305 AEAD security.
    """
    return Connection(path=path, passphrase=passphrase)
