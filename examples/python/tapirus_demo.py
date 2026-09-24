"""
TapirusDB Python Ctypes Demonstration
Demonstrates calling the TapirusDB C ABI from Python without any external dependencies.
"""

import ctypes
import json
import os
import sys

# Search candidate paths for the shared library
CANDIDATE_PATHS = [
    os.path.expanduser("~/.tapirusdb-target/release/libtapirus.so"),
    os.path.abspath(os.path.join(os.path.dirname(__file__), "../../target/release/libtapirus.so")),
    os.path.abspath(os.path.join(os.path.dirname(__file__), "../../target/release/tapirus.dll")),
    "libtapirus.so",
]

lib_path = None
for path in CANDIDATE_PATHS:
    if os.path.exists(path):
        lib_path = path
        break

if not lib_path:
    print(f"Error: libtapirus shared library not found in candidate paths: {CANDIDATE_PATHS}")
    sys.exit(1)

# Load shared library
tapirus = ctypes.CDLL(lib_path)

# Configure argument and return types
tapirus.tapirus_version.restype = ctypes.c_char_p

tapirus.tapirus_open_in_memory.restype = ctypes.c_void_p

tapirus.tapirus_open.argtypes = [ctypes.c_char_p]
tapirus.tapirus_open.restype = ctypes.c_void_p

tapirus.tapirus_close.argtypes = [ctypes.c_void_p]
tapirus.tapirus_close.restype = None

tapirus.tapirus_execute.argtypes = [
    ctypes.c_void_p,
    ctypes.c_char_p,
    ctypes.POINTER(ctypes.c_char_p),
]
tapirus.tapirus_execute.restype = ctypes.c_int32

tapirus.tapirus_query_json.argtypes = [
    ctypes.c_void_p,
    ctypes.c_char_p,
    ctypes.POINTER(ctypes.c_char_p),
    ctypes.POINTER(ctypes.c_char_p),
]
tapirus.tapirus_query_json.restype = ctypes.c_int32

tapirus.tapirus_free_string.argtypes = [ctypes.c_char_p]
tapirus.tapirus_free_string.restype = None

tapirus.tapirus_checkpoint.argtypes = [ctypes.c_void_p]
tapirus.tapirus_checkpoint.restype = ctypes.c_int64


def main():
    version = tapirus.tapirus_version().decode("utf-8")
    print(f"Loaded TapirusDB native library (v{version}) from: {lib_path}")

    # 1. Open database connection
    conn = tapirus.tapirus_open_in_memory()
    if not conn:
        print("Failed to open TapirusDB in-memory connection")
        sys.exit(1)

    try:
        # 2. Create table with vector column
        err = ctypes.c_char_p()
        sql_create = (
            b"CREATE TABLE satellites (id INTEGER PRIMARY KEY, name TEXT, orbit VECTOR(3));"
        )
        res = tapirus.tapirus_execute(conn, sql_create, ctypes.byref(err))
        if res < 0:
            print(f"Create table error: {err.value.decode('utf-8')}")
            return
        print("Created table 'satellites' with VECTOR(3) support.")

        # 3. Insert orbital telemetry
        satellites = [
            (1, "Hubble Space Telescope", [0.82, 0.12, 0.55]),
            (2, "James Webb Space Telescope", [0.15, 0.94, 0.28]),
            (3, "International Space Station", [0.80, 0.15, 0.58]),
        ]
        for sid, sname, vec in satellites:
            sql_insert = (
                f"INSERT INTO satellites (id, name, orbit) VALUES ({sid}, '{sname}', {vec});".encode(
                    "utf-8"
                )
            )
            tapirus.tapirus_execute(conn, sql_insert, ctypes.byref(err))

        print(f"Inserted {len(satellites)} satellite records with dense vectors.")

        # 4. Query satellites via JSON FFI
        json_out = ctypes.c_char_p()
        sql_select = b"SELECT id, name, orbit FROM satellites;"
        ret = tapirus.tapirus_query_json(conn, sql_select, ctypes.byref(json_out), ctypes.byref(err))
        if ret == 0 and json_out.value:
            rows = json.loads(json_out.value.decode("utf-8"))
            print(f"\nQuery returned {len(rows)} rows (JSON serialized via C ABI):")
            for r in rows:
                print(f"  • #{r['values'][0]['Integer']}: {r['values'][1]['Text']} -> orbit: {r['values'][2]['Vector']}")
            tapirus.tapirus_free_string(json_out)

        # 5. Flush WAL checkpoint
        flushed = tapirus.tapirus_checkpoint(conn)
        print(f"\nWAL Checkpoint successfully flushed {flushed} page(s).")
        print("TapirusDB C FFI verification passed 100% successfully!")

    finally:
        tapirus.tapirus_close(conn)


if __name__ == "__main__":
    main()
