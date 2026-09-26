"""
TapirusDB Official Python SDK
~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

An embedded quad-model database and AI agent memory engine engineered in 100% Safe Rust.

Basic usage:

    >>> import tapirus
    >>> db = tapirus.connect("agent_memory.tapir")
    >>> db.execute("CREATE TABLE products (id INT PRIMARY KEY, name TEXT);")
    >>> db.execute("INSERT INTO products VALUES (1, 'Autonomous Drone');")
    >>> rows = db.query("SELECT * FROM products;")
    >>> print(rows)
    [{'id': 1, 'name': 'Autonomous Drone'}]
"""

from .connection import Connection, connect
from .exceptions import (
    TapirusError,
    ConnectionError,
    QueryError,
    AuthenticationError,
    ConstraintError,
)

__version__ = "1.0.1"

def version() -> str:
    """Returns the TapirusDB engine version."""
    from .ffi import get_ffi
    ffi = get_ffi()
    if ffi.is_native_available and hasattr(ffi._lib, "tapirus_version"):
        v = ffi._lib.tapirus_version()
        if v:
            return v.decode("utf-8")
    return __version__

__all__ = [
    "connect",
    "Connection",
    "version",
    "TapirusError",
    "ConnectionError",
    "QueryError",
    "AuthenticationError",
    "ConstraintError",
]
