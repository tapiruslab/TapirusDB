"""
TapirusDB Python Client
The Safe-Rust Embedded Quad-Model AI Database Engine.
Single-file (.tapir), 100% Safe Rust, Quad-Model (SQL + Document + HNSW Vector + Property Graph).
"""

import ctypes
import json
import os
import sys
from typing import Any, Dict, List, Optional

__version__ = "0.1.2"
__all__ = [
    "Tapirus",
    "TapirusError",
    "TapirusVectorStore",
    "TapirusChatMessageHistory",
    "TapirusLlamaVectorStore",
]


class TapirusError(Exception):
    """Exception raised for errors executing TapirusDB operations."""
    pass


def __getattr__(name: str):
    if name in ("TapirusVectorStore", "TapirusChatMessageHistory"):
        from .langchain import TapirusVectorStore, TapirusChatMessageHistory
        return globals()[name] if name in globals() else (
            TapirusVectorStore if name == "TapirusVectorStore" else TapirusChatMessageHistory
        )
    if name == "TapirusLlamaVectorStore":
        from .llamaindex import TapirusLlamaVectorStore
        return TapirusLlamaVectorStore
    raise AttributeError(f"module 'tapirus' has no attribute '{name}'")


def _find_library() -> str:
    candidates = [
        os.path.expanduser("~/.tapirusdb-target/release/libtapirus.so"),
        os.path.expanduser("~/.tapirusdb-target/release/libtapirus.dylib"),
        os.path.abspath(os.path.join(os.path.dirname(__file__), "../../../target/release/libtapirus.so")),
        os.path.abspath(os.path.join(os.path.dirname(__file__), "../../target/release/libtapirus.so")),
        os.path.abspath(os.path.join(os.path.dirname(__file__), "../../../target/release/libtapirus.dylib")),
        os.path.abspath(os.path.join(os.path.dirname(__file__), "../../target/release/libtapirus.dylib")),
        os.path.abspath(os.path.join(os.path.dirname(__file__), "../../../target/release/tapirus.dll")),
        os.path.abspath(os.path.join(os.path.dirname(__file__), "../../target/release/tapirus.dll")),
        "libtapirus.so",
        "libtapirus.dylib",
        "tapirus.dll",
    ]
    for p in candidates:
        if os.path.exists(p):
            return p
    if sys.platform == "darwin":
        return "libtapirus.dylib"
    elif sys.platform == "win32":
        return "tapirus.dll"
    return "libtapirus.so"


class _TapirusFFI:
    _instance = None

    @classmethod
    def get(cls):
        if cls._instance is None:
            lib_path = _find_library()
            try:
                lib = ctypes.CDLL(lib_path)
            except OSError as e:
                raise TapirusError(
                    f"Failed to load TapirusDB native library from '{lib_path}'. "
                    "Ensure libtapirus.so or tapirus.dll is built and available: "
                    f"{e}"
                )

            lib.tapirus_version.restype = ctypes.c_char_p
            lib.tapirus_open_in_memory.restype = ctypes.c_void_p

            lib.tapirus_open.argtypes = [ctypes.c_char_p]
            lib.tapirus_open.restype = ctypes.c_void_p

            lib.tapirus_open_encrypted.argtypes = [ctypes.c_char_p, ctypes.c_char_p]
            lib.tapirus_open_encrypted.restype = ctypes.c_void_p

            lib.tapirus_close.argtypes = [ctypes.c_void_p]
            lib.tapirus_close.restype = None

            lib.tapirus_execute.argtypes = [
                ctypes.c_void_p,
                ctypes.c_char_p,
                ctypes.POINTER(ctypes.c_char_p),
            ]
            lib.tapirus_execute.restype = ctypes.c_int32

            lib.tapirus_query_json.argtypes = [
                ctypes.c_void_p,
                ctypes.c_char_p,
                ctypes.POINTER(ctypes.c_char_p),
                ctypes.POINTER(ctypes.c_char_p),
            ]
            lib.tapirus_query_json.restype = ctypes.c_int32

            lib.tapirus_free_string.argtypes = [ctypes.c_char_p]
            lib.tapirus_free_string.restype = None

            lib.tapirus_checkpoint.argtypes = [ctypes.c_void_p]
            lib.tapirus_checkpoint.restype = ctypes.c_int64

            cls._instance = lib
        return cls._instance


class Tapirus:
    """Active connection to a TapirusDB embedded database."""

    def __init__(self, path: Optional[str] = None, passphrase: Optional[str] = None):
        self._ffi = _TapirusFFI.get()
        if path is None or path == ":memory:":
            self._handle = self._ffi.tapirus_open_in_memory()
        elif passphrase:
            self._handle = self._ffi.tapirus_open_encrypted(
                path.encode("utf-8"),
                passphrase.encode("utf-8"),
            )
        else:
            self._handle = self._ffi.tapirus_open(path.encode("utf-8"))

        if not self._handle:
            raise TapirusError(f"Failed to open TapirusDB database at '{path}'")

    @classmethod
    def open(cls, path: str, passphrase: Optional[str] = None) -> "Tapirus":
        """Open a database file on disk with optional passphrase encryption."""
        return cls(path=path, passphrase=passphrase)

    @classmethod
    def open_in_memory(cls) -> "Tapirus":
        """Open a transient in-memory database."""
        return cls(path=":memory:")

    @classmethod
    def version(cls) -> str:
        """Return the TapirusDB engine version string."""
        ffi = _TapirusFFI.get()
        return ffi.tapirus_version().decode("utf-8")

    def execute(self, sql: str) -> int:
        """Execute a non-query SQL command (CREATE, INSERT, UPDATE, DELETE, BEGIN, COMMIT, etc.)."""
        if not self._handle:
            raise TapirusError("Database connection is closed")

        err_ptr = ctypes.c_char_p()
        affected = self._ffi.tapirus_execute(
            self._handle,
            sql.encode("utf-8"),
            ctypes.byref(err_ptr),
        )

        if affected < 0:
            err_msg = "Unknown error"
            if err_ptr.value:
                err_msg = err_ptr.value.decode("utf-8", errors="replace")
                self._ffi.tapirus_free_string(err_ptr)
            raise TapirusError(f"Execute failed: {err_msg}")

        return affected

    def query(self, sql: str) -> List[Dict[str, Any]]:
        """Execute a query and return rows as a list of dictionaries."""
        if not self._handle:
            raise TapirusError("Database connection is closed")

        json_ptr = ctypes.c_char_p()
        err_ptr = ctypes.c_char_p()

        res = self._ffi.tapirus_query_json(
            self._handle,
            sql.encode("utf-8"),
            ctypes.byref(json_ptr),
            ctypes.byref(err_ptr),
        )

        if res != 0:
            err_msg = "Unknown error"
            if err_ptr.value:
                err_msg = err_ptr.value.decode("utf-8", errors="replace")
                self._ffi.tapirus_free_string(err_ptr)
            raise TapirusError(f"Query failed: {err_msg}")

        try:
            if json_ptr.value:
                payload = json_ptr.value.decode("utf-8", errors="replace")
                raw_rows = json.loads(payload)
                formatted = []
                for r in raw_rows:
                    cols = r.get("columns", [])
                    vals = r.get("values", [])
                    row_dict = {}
                    for col, v in zip(cols, vals):
                        if isinstance(v, dict):
                            val = next(iter(v.values())) if v else None
                        else:
                            val = v
                        row_dict[col] = val
                    formatted.append(row_dict)
                return formatted
            return []
        finally:
            if json_ptr.value:
                self._ffi.tapirus_free_string(json_ptr)

    def checkpoint(self) -> int:
        """Manually flush Write-Ahead Log frames to durable disk storage."""
        if not self._handle:
            raise TapirusError("Database connection is closed")
        return self._ffi.tapirus_checkpoint(self._handle)

    def close(self):
        """Close connection and flush unwritten buffers."""
        if getattr(self, "_handle", None):
            self._ffi.tapirus_close(self._handle)
            self._handle = None

    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc_val, exc_tb):
        self.close()

    def __del__(self):
        self.close()
