"""
TapirusDB Low-Level C-FFI Loader
"""

import ctypes
import os
import sys
import json
from typing import Optional, List, Dict, Any
from .exceptions import ConnectionError, QueryError, AuthenticationError

def _find_library() -> Optional[str]:
    """Search for the compiled TapirusDB shared library."""
    # 1. Direct environment variable override
    env_path = os.environ.get("TAPIRUS_LIB_PATH")
    if env_path and os.path.isfile(env_path):
        return os.path.abspath(env_path)

    lib_names = []
    if sys.platform.startswith("linux"):
        lib_names = ["libtapirus.so", "libtapirus_ffi.so"]
    elif sys.platform == "darwin":
        lib_names = ["libtapirus.dylib", "libtapirus.so"]
    elif sys.platform == "win32":
        lib_names = ["tapirus.dll", "tapirus_ffi.dll", "libtapirus.dll"]
    else:
        lib_names = ["libtapirus.so"]

    search_dirs = [
        os.path.dirname(__file__),
    ]

    custom_dir = os.environ.get("TAPIRUS_LIB_DIR")
    if custom_dir:
        search_dirs.append(os.path.abspath(custom_dir))

    search_dirs.extend([
        os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "..", "..", "target", "release")),
        os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "..", "..", "target", "debug")),
        os.path.expanduser("~/.tapirus/lib"),
        "/usr/local/lib",
        "/usr/lib",
    ])

    for d in search_dirs:
        for name in lib_names:
            p = os.path.join(d, name)
            if os.path.isfile(p):
                return p

    return None

class FFIWrapper:
    def __init__(self):
        self._lib_path = _find_library()
        self._lib = None

        if self._lib_path:
            try:
                self._lib = ctypes.CDLL(self._lib_path)
                self._bind_functions()
            except Exception as e:
                self._lib = None

    @property
    def is_native_available(self) -> bool:
        return self._lib is not None

    def _bind_functions(self):
        lib = self._lib
        # tapirus_open
        lib.tapirus_open.argtypes = [ctypes.c_char_p]
        lib.tapirus_open.restype = ctypes.c_void_p

        # tapirus_open_encrypted
        lib.tapirus_open_encrypted.argtypes = [ctypes.c_char_p, ctypes.c_char_p]
        lib.tapirus_open_encrypted.restype = ctypes.c_void_p

        # tapirus_open_in_memory
        lib.tapirus_open_in_memory.argtypes = []
        lib.tapirus_open_in_memory.restype = ctypes.c_void_p

        # tapirus_close
        lib.tapirus_close.argtypes = [ctypes.c_void_p]
        lib.tapirus_close.restype = None

        # tapirus_execute
        lib.tapirus_execute.argtypes = [ctypes.c_void_p, ctypes.c_char_p]
        lib.tapirus_execute.restype = ctypes.c_int32

        # tapirus_query
        lib.tapirus_query.argtypes = [ctypes.c_void_p, ctypes.c_char_p]
        lib.tapirus_query.restype = ctypes.c_void_p

        # tapirus_free_string
        lib.tapirus_free_string.argtypes = [ctypes.c_void_p]
        lib.tapirus_free_string.restype = None

        # tapirus_last_error
        lib.tapirus_last_error.argtypes = [ctypes.c_void_p]
        lib.tapirus_last_error.restype = ctypes.c_void_p

        # tapirus_version
        lib.tapirus_version.argtypes = []
        lib.tapirus_version.restype = ctypes.c_char_p

_ffi_instance: Optional[FFIWrapper] = None

def get_ffi() -> FFIWrapper:
    global _ffi_instance
    if _ffi_instance is None:
        _ffi_instance = FFIWrapper()
    return _ffi_instance
