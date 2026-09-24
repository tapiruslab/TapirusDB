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
        affected = lib.tapirus_execute(self._handle, sql.encode("utf-8"))
        if affected < 0:
            err_ptr = lib.tapirus_last_error(self._handle)
            msg = "Query execution error"
            if err_ptr:
                msg = ctypes.string_at(err_ptr).decode("utf-8", errors="replace")
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
        res_ptr = lib.tapirus_query(self._handle, sql.encode("utf-8"))
        if not res_ptr:
            err_ptr = lib.tapirus_last_error(self._handle)
            msg = "Query failed"
            if err_ptr:
                msg = ctypes.string_at(err_ptr).decode("utf-8", errors="replace")
                lib.tapirus_free_string(err_ptr)
            raise QueryError(msg)

        try:
            raw_str = ctypes.string_at(res_ptr).decode("utf-8", errors="replace")
            return json.loads(raw_str)
        finally:
            lib.tapirus_free_string(res_ptr)

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
