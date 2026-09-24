"""
TapirusDB Exception Hierarchy
"""

class TapirusError(Exception):
    """Base exception for all TapirusDB errors."""
    pass

class ConnectionError(TapirusError):
    """Raised when failing to open or connect to a database file."""
    pass

class QueryError(TapirusError):
    """Raised when SQL syntax or query execution fails."""
    pass

class AuthenticationError(TapirusError):
    """Raised when passphrase decryption or salt verification fails."""
    pass

class ConstraintError(TapirusError):
    """Raised when primary key, NOT NULL, or uniqueness constraints are violated."""
    pass
