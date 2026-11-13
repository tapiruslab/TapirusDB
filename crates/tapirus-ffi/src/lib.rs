//! # TapirusDB C FFI Layer (`tapirus-ffi`)
//!
//! Provides a standardized C-compatible Application Binary Interface (ABI)
//! allowing external languages (Python, C, C++, Swift, Kotlin, Go, Zig, Node.js)
//! to embed TapirusDB as a shared (`.so`, `.dll`, `.dylib`) or static library.

#![allow(non_snake_case)]

use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::ptr;
use tapirus::Connection;
use tapirus::traits::Value;

/// Opaque handle to a TapirusDB Connection
pub struct TapirusConn {
    inner: Connection,
}

/// Open a TapirusDB single-file database.
///
/// # Safety
/// `path` must be a valid null-terminated UTF-8 C string.
/// Returns a pointer to `TapirusConn` on success, or NULL on failure.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tapirus_open(path: *const c_char) -> *mut TapirusConn {
    if path.is_null() {
        return ptr::null_mut();
    }

    let res = catch_unwind(AssertUnwindSafe(|| {
        let c_str = unsafe { CStr::from_ptr(path) };
        let path_str = match c_str.to_str() {
            Ok(s) => s,
            Err(_) => return ptr::null_mut(),
        };

        match Connection::open(Path::new(path_str)) {
            Ok(inner) => Box::into_raw(Box::new(TapirusConn { inner })),
            Err(_) => ptr::null_mut(),
        }
    }));

    res.unwrap_or(ptr::null_mut())
}

/// Open an encrypted TapirusDB single-file database using ChaCha20-Poly1305 AEAD.
///
/// # Safety
/// `path` and `passphrase` must be valid null-terminated UTF-8 C strings.
/// Returns a pointer to `TapirusConn` on success, or NULL on failure.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tapirus_open_encrypted(
    path: *const c_char,
    passphrase: *const c_char,
) -> *mut TapirusConn {
    if path.is_null() || passphrase.is_null() {
        return ptr::null_mut();
    }

    let res = catch_unwind(AssertUnwindSafe(|| {
        let p_str = match unsafe { CStr::from_ptr(path) }.to_str() {
            Ok(s) => s,
            Err(_) => return ptr::null_mut(),
        };
        let pass_str = match unsafe { CStr::from_ptr(passphrase) }.to_str() {
            Ok(s) => s,
            Err(_) => return ptr::null_mut(),
        };

        match Connection::open_encrypted(Path::new(p_str), pass_str) {
            Ok(inner) => Box::into_raw(Box::new(TapirusConn { inner })),
            Err(_) => ptr::null_mut(),
        }
    }));

    res.unwrap_or(ptr::null_mut())
}

/// Open a transient in-memory TapirusDB database.
///
/// # Safety
/// Returns a pointer to `TapirusConn` on success, or NULL on failure.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tapirus_open_in_memory() -> *mut TapirusConn {
    let res = catch_unwind(AssertUnwindSafe(|| match Connection::open_in_memory() {
        Ok(inner) => Box::into_raw(Box::new(TapirusConn { inner })),
        Err(_) => ptr::null_mut(),
    }));

    res.unwrap_or(ptr::null_mut())
}

/// Close and deallocate an open TapirusDB Connection.
///
/// # Safety
/// `conn` must be a valid pointer returned from `tapirus_open` or `tapirus_open_in_memory`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tapirus_close(conn: *mut TapirusConn) {
    if !conn.is_null() {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            drop(unsafe { Box::from_raw(conn) });
        }));
    }
}

/// Execute a non-query SQL command (CREATE TABLE, INSERT, UPDATE, DELETE).
///
/// Returns number of affected rows (>= 0) on success, or -1 on error.
/// If an error occurs and `err_msg_out` is non-null, `*err_msg_out` is set to an error message
/// that must be freed using `tapirus_free_string`.
///
/// # Safety
/// `conn` and `sql` must be valid non-null pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tapirus_execute(
    conn: *mut TapirusConn,
    sql: *const c_char,
    err_msg_out: *mut *mut c_char,
) -> i32 {
    if conn.is_null() || sql.is_null() {
        return -1;
    }

    let res = catch_unwind(AssertUnwindSafe(|| {
        let conn_ref = unsafe { &*conn };
        let c_str = unsafe { CStr::from_ptr(sql) };
        let sql_str = match c_str.to_str() {
            Ok(s) => s,
            Err(e) => {
                unsafe { set_err(err_msg_out, &format!("Invalid UTF-8 in SQL string: {e}")) };
                return -1;
            }
        };

        match conn_ref.inner.execute(sql_str) {
            Ok(affected) => affected as i32,
            Err(e) => {
                unsafe { set_err(err_msg_out, &e.to_string()) };
                -1
            }
        }
    }));

    res.unwrap_or(-1)
}

/// Execute a SQL query and return rows formatted as a JSON array string.
///
/// On success: returns 0 and sets `*json_out` to a heap-allocated JSON string
/// that MUST be deallocated with `tapirus_free_string`.
/// On error: returns -1 and optionally sets `*err_msg_out`.
///
/// # Safety
/// `conn`, `sql`, and `json_out` must be valid non-null pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tapirus_query_json(
    conn: *mut TapirusConn,
    sql: *const c_char,
    json_out: *mut *mut c_char,
    err_msg_out: *mut *mut c_char,
) -> i32 {
    if conn.is_null() || sql.is_null() || json_out.is_null() {
        return -1;
    }

    let res = catch_unwind(AssertUnwindSafe(|| {
        let conn_ref = unsafe { &*conn };
        let c_str = unsafe { CStr::from_ptr(sql) };
        let sql_str = match c_str.to_str() {
            Ok(s) => s,
            Err(e) => {
                unsafe { set_err(err_msg_out, &format!("Invalid UTF-8 in SQL string: {e}")) };
                return -1;
            }
        };

        match conn_ref.inner.query_json(sql_str) {
            Ok(json_str) => {
                if let Ok(c_json) = CString::new(json_str) {
                    unsafe { *json_out = c_json.into_raw() };
                    0
                } else {
                    unsafe { set_err(err_msg_out, "JSON string contained null byte") };
                    -1
                }
            }
            Err(e) => {
                unsafe { set_err(err_msg_out, &e.to_string()) };
                -1
            }
        }
    }));

    res.unwrap_or(-1)
}

/// Manually trigger a Write-Ahead Log (WAL) checkpoint.
///
/// Flushes all committed frames to the main `.tapir` file and resets the log.
/// Returns number of flushed pages on success, or -1 on error.
///
/// # Safety
/// `conn` must be a valid pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tapirus_checkpoint(conn: *mut TapirusConn) -> i64 {
    if conn.is_null() {
        return -1;
    }

    let res = catch_unwind(AssertUnwindSafe(|| {
        let conn_ref = unsafe { &*conn };
        match conn_ref.inner.checkpoint() {
            Ok(flushed) => flushed as i64,
            Err(_) => -1,
        }
    }));

    res.unwrap_or(-1)
}

/// Free a heap-allocated string returned by `tapirus_query_json` or error messages.
///
/// # Safety
/// `s` must be a valid pointer returned by an FFI function, or NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tapirus_free_string(s: *mut c_char) {
    if !s.is_null() {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            drop(unsafe { CString::from_raw(s) });
        }));
    }
}

/// Return the TapirusDB library version as a static null-terminated C string.
#[unsafe(no_mangle)]
pub extern "C" fn tapirus_version() -> *const c_char {
    concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr() as *const c_char
}

unsafe fn set_err(err_msg_out: *mut *mut c_char, msg: &str) {
    if err_msg_out.is_null() {
        return;
    }
    if let Ok(c_msg) = CString::new(msg) {
        unsafe { *err_msg_out = c_msg.into_raw() };
    }
}

unsafe fn cstr_to_str<'a>(ptr: *const c_char, label: &str) -> Result<&'a str, String> {
    if ptr.is_null() {
        return Err(format!("{label} pointer must not be null"));
    }
    unsafe { CStr::from_ptr(ptr) }
        .to_str()
        .map_err(|e| format!("Invalid UTF-8 in {label}: {e}"))
}

unsafe fn write_cstring_out(
    out: *mut *mut c_char,
    text: String,
    err_msg_out: *mut *mut c_char,
) -> i32 {
    if let Ok(c_str) = CString::new(text) {
        unsafe { *out = c_str.into_raw() };
        0
    } else {
        unsafe { set_err(err_msg_out, "Output string contained null byte") };
        -1
    }
}

unsafe fn parse_json_params(params_json: *const c_char) -> Result<Vec<Value>, String> {
    if params_json.is_null() {
        return Ok(Vec::new());
    }
    let raw = unsafe { cstr_to_str(params_json, "params_json") }?.trim();
    if raw.is_empty() {
        return Ok(Vec::new());
    }
    let parsed: Vec<serde_json::Value> =
        serde_json::from_str(raw).map_err(|e| format!("Invalid JSON parameter array: {e}"))?;
    parsed
        .iter()
        .map(|val| Value::from_json_parameter(val).map_err(|e| e.to_string()))
        .collect()
}

/// Execute a parameterized non-query SQL statement safely (`params_json` is a JSON array or NULL).
///
/// Returns affected rows (`>= 0`) on success, or `-1` on failure.
///
/// # Safety
/// `conn` and `sql` must be valid pointers; `params_json` may be NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tapirus_execute_params(
    conn: *mut TapirusConn,
    sql: *const c_char,
    params_json: *const c_char,
    err_msg_out: *mut *mut c_char,
) -> i32 {
    if conn.is_null() || sql.is_null() {
        return -1;
    }

    let res = catch_unwind(AssertUnwindSafe(|| {
        let conn_ref = unsafe { &*conn };
        let sql_str = match unsafe { cstr_to_str(sql, "SQL") } {
            Ok(s) => s,
            Err(msg) => {
                unsafe { set_err(err_msg_out, &msg) };
                return -1;
            }
        };
        let params = match unsafe { parse_json_params(params_json) } {
            Ok(p) => p,
            Err(msg) => {
                unsafe { set_err(err_msg_out, &msg) };
                return -1;
            }
        };

        match conn_ref.inner.execute_with_params(sql_str, &params) {
            Ok(affected) => affected as i32,
            Err(e) => {
                unsafe { set_err(err_msg_out, &e.to_string()) };
                -1
            }
        }
    }));

    res.unwrap_or(-1)
}

/// Execute a parameterized SQL query returning rows formatted as a JSON array string.
///
/// `params_json` may be NULL or a JSON array of bound values. Returns `0` on success, `-1` on error.
/// Caller must free `*json_out` with `tapirus_free_string`.
///
/// # Safety
/// `conn`, `sql`, and `json_out` must be valid pointers; `params_json` may be NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tapirus_query_params_json(
    conn: *mut TapirusConn,
    sql: *const c_char,
    params_json: *const c_char,
    json_out: *mut *mut c_char,
    err_msg_out: *mut *mut c_char,
) -> i32 {
    if conn.is_null() || sql.is_null() || json_out.is_null() {
        return -1;
    }

    let res = catch_unwind(AssertUnwindSafe(|| {
        let conn_ref = unsafe { &*conn };
        let sql_str = match unsafe { cstr_to_str(sql, "SQL") } {
            Ok(s) => s,
            Err(msg) => {
                unsafe { set_err(err_msg_out, &msg) };
                return -1;
            }
        };
        let params = match unsafe { parse_json_params(params_json) } {
            Ok(p) => p,
            Err(msg) => {
                unsafe { set_err(err_msg_out, &msg) };
                return -1;
            }
        };

        match conn_ref.inner.query_with_params(sql_str, &params) {
            Ok(rows) => match serde_json::to_string(&rows) {
                Ok(json_str) => unsafe { write_cstring_out(json_out, json_str, err_msg_out) },
                Err(e) => {
                    unsafe { set_err(err_msg_out, &format!("Failed to serialize rows: {e}")) };
                    -1
                }
            },
            Err(e) => {
                unsafe { set_err(err_msg_out, &e.to_string()) };
                -1
            }
        }
    }));

    res.unwrap_or(-1)
}

/// Execute a JSON-encoded AI Memory gateway operation (`store`, `upsert`, `search`,
/// `search_detailed`, `forget`, `stats`, `prepare`, `configure`, `delete`, etc.).
///
/// Caller must free `*json_out` with `tapirus_free_string`. Returns `0` on success, `-1` on error.
///
/// # Safety
/// `conn`, `operation`, and `json_out` must be valid pointers; `payload_json` may be NULL (`"{}"`).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tapirus_memory_request_json(
    conn: *mut TapirusConn,
    operation: *const c_char,
    payload_json: *const c_char,
    json_out: *mut *mut c_char,
    err_msg_out: *mut *mut c_char,
) -> i32 {
    if conn.is_null() || operation.is_null() || json_out.is_null() {
        return -1;
    }

    let res = catch_unwind(AssertUnwindSafe(|| {
        let conn_ref = unsafe { &*conn };
        let op = match unsafe { cstr_to_str(operation, "operation") } {
            Ok(s) => s,
            Err(msg) => {
                unsafe { set_err(err_msg_out, &msg) };
                return -1;
            }
        };
        let payload = if payload_json.is_null() {
            "{}"
        } else {
            match unsafe { cstr_to_str(payload_json, "payload_json") } {
                Ok(s) => s,
                Err(msg) => {
                    unsafe { set_err(err_msg_out, &msg) };
                    return -1;
                }
            }
        };

        match conn_ref.inner.memory_request_json(op, payload) {
            Ok(out_json) => unsafe { write_cstring_out(json_out, out_json, err_msg_out) },
            Err(e) => {
                unsafe { set_err(err_msg_out, &e.to_string()) };
                -1
            }
        }
    }));

    res.unwrap_or(-1)
}

/// Store an AI Memory document described by a JSON `MemoryDocument` object.
///
/// Writes the allocated `memory_id` into `*id_out` if non-null. Returns `0` on success, `-1` on error.
///
/// # Safety
/// `conn` and `doc_json` must be valid pointers; `id_out` may be NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tapirus_store_memory_json(
    conn: *mut TapirusConn,
    doc_json: *const c_char,
    id_out: *mut u64,
    err_msg_out: *mut *mut c_char,
) -> i32 {
    if conn.is_null() || doc_json.is_null() {
        return -1;
    }

    let res = catch_unwind(AssertUnwindSafe(|| {
        let conn_ref = unsafe { &*conn };
        let doc_str = match unsafe { cstr_to_str(doc_json, "doc_json") } {
            Ok(s) => s,
            Err(msg) => {
                unsafe { set_err(err_msg_out, &msg) };
                return -1;
            }
        };
        match conn_ref.inner.memory_request_json("store", doc_str) {
            Ok(id_json) => match id_json.trim().parse::<u64>() {
                Ok(id) => {
                    if !id_out.is_null() {
                        unsafe { *id_out = id };
                    }
                    0
                }
                Err(e) => {
                    unsafe {
                        set_err(
                            err_msg_out,
                            &format!("Unexpected store memory response: {e}"),
                        );
                    }
                    -1
                }
            },
            Err(e) => {
                unsafe { set_err(err_msg_out, &e.to_string()) };
                -1
            }
        }
    }));

    res.unwrap_or(-1)
}

/// Search AI memories using a JSON `MemorySearch` query object and return JSON recall results.
///
/// Caller must free `*json_out` with `tapirus_free_string`. Returns `0` on success, `-1` on error.
///
/// # Safety
/// `conn`, `search_json`, and `json_out` must be valid pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tapirus_search_memories_json(
    conn: *mut TapirusConn,
    search_json: *const c_char,
    json_out: *mut *mut c_char,
    err_msg_out: *mut *mut c_char,
) -> i32 {
    if conn.is_null() || search_json.is_null() || json_out.is_null() {
        return -1;
    }
    unsafe {
        tapirus_memory_request_json(conn, c"search".as_ptr(), search_json, json_out, err_msg_out)
    }
}

/// Delete and forget an AI memory entry by `memory_id`.
///
/// Returns `1` if the entry existed and was deleted, `0` if not found, or `-1` on error.
///
/// # Safety
/// `conn` must be a valid pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tapirus_forget_memory(
    conn: *mut TapirusConn,
    memory_id: u64,
    err_msg_out: *mut *mut c_char,
) -> i32 {
    if conn.is_null() {
        return -1;
    }

    let res = catch_unwind(AssertUnwindSafe(|| {
        let conn_ref = unsafe { &*conn };
        match conn_ref.inner.memory_forget_checked(memory_id) {
            Ok(true) => 1,
            Ok(false) => 0,
            Err(e) => {
                unsafe { set_err(err_msg_out, &e.to_string()) };
                -1
            }
        }
    }));

    res.unwrap_or(-1)
}

// =========================================================================
// SQLite C ABI Drop-in Compatibility Layer
// =========================================================================

/// SQLite Result Codes
pub const SQLITE_OK: i32 = 0;
pub const SQLITE_ERROR: i32 = 1;
pub const SQLITE_INTERNAL: i32 = 2;
pub const SQLITE_PERM: i32 = 3;
pub const SQLITE_ABORT: i32 = 4;
pub const SQLITE_BUSY: i32 = 5;
pub const SQLITE_LOCKED: i32 = 6;
pub const SQLITE_NOMEM: i32 = 7;
pub const SQLITE_RANGE: i32 = 25;
pub const SQLITE_ROW: i32 = 100;
pub const SQLITE_DONE: i32 = 101;

/// SQLite Column Datatypes
pub const SQLITE_INTEGER: i32 = 1;
pub const SQLITE_FLOAT: i32 = 2;
pub const SQLITE_TEXT: i32 = 3;
pub const SQLITE_BLOB: i32 = 4;
pub const SQLITE_NULL: i32 = 5;

/// Opaque SQLite Database Handle representation
#[repr(C)]
#[allow(non_camel_case_types)]
pub struct sqlite3 {
    pub(crate) conn: Connection,
    pub(crate) last_error: Option<CString>,
    pub(crate) last_changes: i32,
}

/// Opaque SQLite Statement Handle representation
#[repr(C)]
#[allow(non_camel_case_types)]
pub struct sqlite3_stmt {
    pub(crate) db: *mut sqlite3,
    pub(crate) sql: String,
    pub(crate) is_query: bool,
    pub(crate) param_count: usize,
    pub(crate) bindings: Vec<Value>,
    pub(crate) executed: bool,
    pub(crate) rows: Vec<tapirus::traits::Row>,
    pub(crate) columns: Vec<CString>,
    pub(crate) current_row_idx: usize,
    pub(crate) current_text_cache: Option<CString>,
    pub(crate) current_blob_cache: Option<Vec<u8>>,
}

fn count_sql_params(sql: &str) -> usize {
    let mut count = 0usize;
    let mut chars = sql.chars().peekable();
    let mut in_single = false;
    let mut in_double = false;
    while let Some(c) = chars.next() {
        match c {
            '\'' if !in_double => {
                if in_single && chars.peek() == Some(&'\'') {
                    let _ = chars.next();
                } else {
                    in_single = !in_single;
                }
            }
            '"' if !in_single => {
                if in_double && chars.peek() == Some(&'"') {
                    let _ = chars.next();
                } else {
                    in_double = !in_double;
                }
            }
            '?' if !in_single && !in_double => count += 1,
            _ => {}
        }
    }
    count
}

fn is_query_sql(sql: &str) -> bool {
    let trimmed = sql.trim_start().to_uppercase();
    trimmed.starts_with("SELECT")
        || trimmed.starts_with("WITH")
        || trimmed.starts_with("EXPLAIN")
        || trimmed.starts_with("GRAPH")
}

unsafe fn execute_sqlite_stmt(stmt: &mut sqlite3_stmt) -> i32 {
    if stmt.db.is_null() {
        return SQLITE_ERROR;
    }
    let db_ref = unsafe { &mut *stmt.db };
    if stmt.is_query {
        let result = if stmt.bindings.is_empty() {
            db_ref.conn.query(&stmt.sql)
        } else {
            db_ref.conn.query_with_params(&stmt.sql, &stmt.bindings)
        };
        match result {
            Ok(rows) => {
                let mut cols = Vec::new();
                if let Some(first) = rows.first() {
                    for c in first.columns() {
                        cols.push(CString::new(c.as_str()).unwrap_or_default());
                    }
                }
                stmt.rows = rows;
                stmt.columns = cols;
                stmt.executed = true;
                stmt.current_row_idx = 0;
                db_ref.last_error = None;
                SQLITE_OK
            }
            Err(e) => {
                db_ref.last_error = Some(CString::new(e.to_string()).unwrap_or_default());
                SQLITE_ERROR
            }
        }
    } else {
        let result = if stmt.bindings.is_empty() {
            db_ref.conn.execute(&stmt.sql)
        } else {
            db_ref.conn.execute_with_params(&stmt.sql, &stmt.bindings)
        };
        match result {
            Ok(affected) => {
                db_ref.last_changes = affected as i32;
                stmt.rows.clear();
                stmt.columns.clear();
                stmt.executed = true;
                stmt.current_row_idx = 0;
                db_ref.last_error = None;
                SQLITE_OK
            }
            Err(e) => {
                db_ref.last_error = Some(CString::new(e.to_string()).unwrap_or_default());
                SQLITE_ERROR
            }
        }
    }
}

/// Open an SQLite compatible connection.
///
/// # Safety
/// Standard C ABI contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_open(filename: *const c_char, ppDb: *mut *mut sqlite3) -> i32 {
    unsafe { sqlite3_open_v2(filename, ppDb, 0, ptr::null()) }
}

/// Open an SQLite compatible connection with flags.
///
/// # Safety
/// Standard C ABI contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_open_v2(
    filename: *const c_char,
    ppDb: *mut *mut sqlite3,
    _flags: i32,
    _zVfs: *const c_char,
) -> i32 {
    if ppDb.is_null() {
        return SQLITE_ERROR;
    }
    unsafe { *ppDb = ptr::null_mut() };

    let res = catch_unwind(AssertUnwindSafe(|| {
        if filename.is_null() {
            return SQLITE_ERROR;
        }
        let c_str = unsafe { CStr::from_ptr(filename) };
        let path_str = match c_str.to_str() {
            Ok(s) => s,
            Err(_) => return SQLITE_ERROR,
        };

        let conn_res = if path_str == ":memory:" || path_str.is_empty() {
            Connection::open_in_memory()
        } else {
            Connection::open(Path::new(path_str))
        };

        match conn_res {
            Ok(conn) => {
                let db_box = Box::new(sqlite3 {
                    conn,
                    last_error: None,
                    last_changes: 0,
                });
                unsafe { *ppDb = Box::into_raw(db_box) };
                SQLITE_OK
            }
            Err(_) => SQLITE_ERROR,
        }
    }));

    res.unwrap_or(SQLITE_ERROR)
}

/// Close an SQLite database connection.
///
/// # Safety
/// Standard C ABI contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_close(db: *mut sqlite3) -> i32 {
    if !db.is_null() {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            drop(unsafe { Box::from_raw(db) });
        }));
    }
    SQLITE_OK
}

/// Close an SQLite database connection (v2).
///
/// # Safety
/// Standard C ABI contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_close_v2(db: *mut sqlite3) -> i32 {
    unsafe { sqlite3_close(db) }
}

/// Prepare a SQL query statement.
///
/// # Safety
/// Standard C ABI contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_prepare_v2(
    db: *mut sqlite3,
    zSql: *const c_char,
    nByte: i32,
    ppStmt: *mut *mut sqlite3_stmt,
    pzTail: *mut *const c_char,
) -> i32 {
    if db.is_null() || zSql.is_null() || ppStmt.is_null() {
        return SQLITE_ERROR;
    }
    unsafe { *ppStmt = ptr::null_mut() };
    if !pzTail.is_null() {
        unsafe { *pzTail = ptr::null() };
    }

    let res = catch_unwind(AssertUnwindSafe(|| {
        let db_ref = unsafe { &mut *db };
        let sql_str = if nByte > 0 {
            let slice = unsafe { std::slice::from_raw_parts(zSql as *const u8, nByte as usize) };
            match std::str::from_utf8(slice) {
                Ok(s) => s,
                Err(_) => {
                    db_ref.last_error = Some(CString::new("Invalid UTF-8 in SQL").unwrap());
                    return SQLITE_ERROR;
                }
            }
        } else {
            match unsafe { CStr::from_ptr(zSql) }.to_str() {
                Ok(s) => s,
                Err(_) => {
                    db_ref.last_error = Some(CString::new("Invalid UTF-8 in SQL").unwrap());
                    return SQLITE_ERROR;
                }
            }
        };

        let param_count = count_sql_params(sql_str);
        let is_query = is_query_sql(sql_str);
        let mut stmt = Box::new(sqlite3_stmt {
            db,
            sql: sql_str.to_string(),
            is_query,
            param_count,
            bindings: vec![Value::Null; param_count],
            executed: false,
            rows: Vec::new(),
            columns: Vec::new(),
            current_row_idx: 0,
            current_text_cache: None,
            current_blob_cache: None,
        });

        // Unparameterized statements still execute at prepare time so `sqlite3_column_count`
        // works immediately before the first `sqlite3_step`, preserving existing behavior.
        if param_count == 0 {
            let rc = unsafe { execute_sqlite_stmt(&mut stmt) };
            if rc != SQLITE_OK {
                return rc;
            }
        }

        unsafe { *ppStmt = Box::into_raw(stmt) };
        SQLITE_OK
    }));

    res.unwrap_or(SQLITE_ERROR)
}

unsafe fn set_sqlite_binding(pStmt: *mut sqlite3_stmt, index: i32, val: Value) -> i32 {
    if pStmt.is_null() {
        return SQLITE_ERROR;
    }
    let stmt = unsafe { &mut *pStmt };
    if index < 1 || (index as usize) > stmt.bindings.len() {
        return SQLITE_RANGE;
    }
    stmt.bindings[(index - 1) as usize] = val;
    SQLITE_OK
}

/// Return the number of `?` SQL parameters in a prepared statement.
///
/// # Safety
/// Standard C ABI contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_bind_parameter_count(pStmt: *mut sqlite3_stmt) -> i32 {
    if pStmt.is_null() {
        return 0;
    }
    unsafe { &*pStmt }.param_count as i32
}

/// Reset all parameter bindings on a prepared statement back to `NULL`.
///
/// # Safety
/// Standard C ABI contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_clear_bindings(pStmt: *mut sqlite3_stmt) -> i32 {
    if pStmt.is_null() {
        return SQLITE_ERROR;
    }
    let stmt = unsafe { &mut *pStmt };
    stmt.bindings.fill(Value::Null);
    SQLITE_OK
}

/// Bind `NULL` to 1-based parameter `index`.
///
/// # Safety
/// Standard C ABI contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_bind_null(pStmt: *mut sqlite3_stmt, index: i32) -> i32 {
    unsafe { set_sqlite_binding(pStmt, index, Value::Null) }
}

/// Bind a 32-bit signed integer to 1-based parameter `index`.
///
/// # Safety
/// Standard C ABI contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_bind_int(pStmt: *mut sqlite3_stmt, index: i32, value: i32) -> i32 {
    unsafe { set_sqlite_binding(pStmt, index, Value::Integer(i64::from(value))) }
}

/// Bind a 64-bit signed integer to 1-based parameter `index`.
///
/// # Safety
/// Standard C ABI contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_bind_int64(
    pStmt: *mut sqlite3_stmt,
    index: i32,
    value: i64,
) -> i32 {
    unsafe { set_sqlite_binding(pStmt, index, Value::Integer(value)) }
}

/// Bind a 64-bit IEEE-754 float to 1-based parameter `index`.
///
/// # Safety
/// Standard C ABI contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_bind_double(
    pStmt: *mut sqlite3_stmt,
    index: i32,
    value: f64,
) -> i32 {
    unsafe { set_sqlite_binding(pStmt, index, Value::Real(value)) }
}

/// Bind a UTF-8 text string to 1-based parameter `index`.
///
/// If `n < 0`, reads up to the NUL terminator; otherwise reads `n` bytes. Copies the buffer
/// immediately (`SQLITE_STATIC` and `SQLITE_TRANSIENT` are both safe).
///
/// # Safety
/// Standard C ABI contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_bind_text(
    pStmt: *mut sqlite3_stmt,
    index: i32,
    value: *const c_char,
    n: i32,
    _destructor: *const c_void,
) -> i32 {
    if value.is_null() {
        return unsafe { sqlite3_bind_null(pStmt, index) };
    }
    let res = catch_unwind(AssertUnwindSafe(|| {
        let text = if n < 0 {
            match unsafe { CStr::from_ptr(value) }.to_str() {
                Ok(s) => s.to_owned(),
                Err(_) => return SQLITE_ERROR,
            }
        } else {
            let slice = unsafe { std::slice::from_raw_parts(value as *const u8, n as usize) };
            match std::str::from_utf8(slice) {
                Ok(s) => s.to_owned(),
                Err(_) => return SQLITE_ERROR,
            }
        };
        unsafe { set_sqlite_binding(pStmt, index, Value::Text(text)) }
    }));
    res.unwrap_or(SQLITE_ERROR)
}

/// Bind a binary BLOB to 1-based parameter `index`.
///
/// Copies `n` bytes immediately (`SQLITE_STATIC` and `SQLITE_TRANSIENT` are both safe).
///
/// # Safety
/// Standard C ABI contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_bind_blob(
    pStmt: *mut sqlite3_stmt,
    index: i32,
    value: *const c_void,
    n: i32,
    _destructor: *const c_void,
) -> i32 {
    if value.is_null() {
        return unsafe { sqlite3_bind_null(pStmt, index) };
    }
    if n < 0 {
        return SQLITE_ERROR;
    }
    let res = catch_unwind(AssertUnwindSafe(|| {
        let slice = unsafe { std::slice::from_raw_parts(value as *const u8, n as usize) };
        unsafe { set_sqlite_binding(pStmt, index, Value::Blob(slice.to_vec())) }
    }));
    res.unwrap_or(SQLITE_ERROR)
}

/// Advance a statement cursor to the next row.
///
/// # Safety
/// Standard C ABI contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_step(pStmt: *mut sqlite3_stmt) -> i32 {
    if pStmt.is_null() {
        return SQLITE_ERROR;
    }

    let res = catch_unwind(AssertUnwindSafe(|| {
        let stmt = unsafe { &mut *pStmt };
        if !stmt.executed {
            let rc = unsafe { execute_sqlite_stmt(stmt) };
            if rc != SQLITE_OK {
                return rc;
            }
        }
        if stmt.current_row_idx < stmt.rows.len() {
            stmt.current_row_idx += 1;
            SQLITE_ROW
        } else {
            SQLITE_DONE
        }
    }));

    res.unwrap_or(SQLITE_ERROR)
}

/// Return number of columns in the result set.
///
/// # Safety
/// Standard C ABI contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_column_count(pStmt: *mut sqlite3_stmt) -> i32 {
    if pStmt.is_null() {
        return 0;
    }
    let stmt = unsafe { &*pStmt };
    stmt.columns.len() as i32
}

/// Return column name at index N.
///
/// # Safety
/// Standard C ABI contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_column_name(pStmt: *mut sqlite3_stmt, N: i32) -> *const c_char {
    if pStmt.is_null() || N < 0 {
        return ptr::null();
    }
    let stmt = unsafe { &*pStmt };
    if let Some(c) = stmt.columns.get(N as usize) {
        c.as_ptr()
    } else {
        ptr::null()
    }
}

/// Return column data type at index N.
///
/// # Safety
/// Standard C ABI contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_column_type(pStmt: *mut sqlite3_stmt, N: i32) -> i32 {
    if pStmt.is_null() || N < 0 {
        return SQLITE_NULL;
    }
    let stmt = unsafe { &*pStmt };
    if stmt.current_row_idx == 0 || stmt.current_row_idx > stmt.rows.len() {
        return SQLITE_NULL;
    }
    let row = &stmt.rows[stmt.current_row_idx - 1];
    if let Some(val) = row.values().get(N as usize) {
        match val {
            tapirus::traits::Value::Integer(_) => SQLITE_INTEGER,
            tapirus::traits::Value::Real(_) => SQLITE_FLOAT,
            tapirus::traits::Value::Text(_) => SQLITE_TEXT,
            tapirus::traits::Value::Blob(_) => SQLITE_BLOB,
            tapirus::traits::Value::Null => SQLITE_NULL,
            tapirus::traits::Value::Vector(_) => SQLITE_TEXT,
        }
    } else {
        SQLITE_NULL
    }
}

/// Return string contents of column at index N.
///
/// # Safety
/// Standard C ABI contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_column_text(pStmt: *mut sqlite3_stmt, N: i32) -> *const c_char {
    if pStmt.is_null() || N < 0 {
        return ptr::null();
    }
    let stmt = unsafe { &mut *pStmt };
    if stmt.current_row_idx == 0 || stmt.current_row_idx > stmt.rows.len() {
        return ptr::null();
    }
    let row = &stmt.rows[stmt.current_row_idx - 1];
    if let Some(val) = row.values().get(N as usize) {
        let s = match val {
            tapirus::traits::Value::Text(t) => t.clone(),
            tapirus::traits::Value::Integer(i) => i.to_string(),
            tapirus::traits::Value::Real(f) => f.to_string(),
            tapirus::traits::Value::Null => String::new(),
            other => other.to_string(),
        };
        stmt.current_text_cache = CString::new(s).ok();
        stmt.current_text_cache
            .as_ref()
            .map_or(ptr::null(), |c| c.as_ptr())
    } else {
        ptr::null()
    }
}

/// Return 32-bit integer value of column at index N.
///
/// # Safety
/// Standard C ABI contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_column_int(pStmt: *mut sqlite3_stmt, N: i32) -> i32 {
    unsafe { sqlite3_column_int64(pStmt, N) as i32 }
}

/// Return 64-bit integer value of column at index N.
///
/// # Safety
/// Standard C ABI contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_column_int64(pStmt: *mut sqlite3_stmt, N: i32) -> i64 {
    if pStmt.is_null() || N < 0 {
        return 0;
    }
    let stmt = unsafe { &*pStmt };
    if stmt.current_row_idx == 0 || stmt.current_row_idx > stmt.rows.len() {
        return 0;
    }
    let row = &stmt.rows[stmt.current_row_idx - 1];
    if let Some(val) = row.values().get(N as usize) {
        match val {
            tapirus::traits::Value::Integer(i) => *i,
            tapirus::traits::Value::Real(f) => *f as i64,
            _ => 0,
        }
    } else {
        0
    }
}

/// Return 64-bit float value of column at index N.
///
/// # Safety
/// Standard C ABI contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_column_double(pStmt: *mut sqlite3_stmt, N: i32) -> f64 {
    if pStmt.is_null() || N < 0 {
        return 0.0;
    }
    let stmt = unsafe { &*pStmt };
    if stmt.current_row_idx == 0 || stmt.current_row_idx > stmt.rows.len() {
        return 0.0;
    }
    let row = &stmt.rows[stmt.current_row_idx - 1];
    if let Some(val) = row.values().get(N as usize) {
        match val {
            tapirus::traits::Value::Real(f) => *f,
            tapirus::traits::Value::Integer(i) => *i as f64,
            _ => 0.0,
        }
    } else {
        0.0
    }
}

/// Return byte length of column at index N.
///
/// # Safety
/// Standard C ABI contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_column_bytes(pStmt: *mut sqlite3_stmt, N: i32) -> i32 {
    if pStmt.is_null() || N < 0 {
        return 0;
    }
    let stmt = unsafe { &*pStmt };
    if stmt.current_row_idx == 0 || stmt.current_row_idx > stmt.rows.len() {
        return 0;
    }
    let row = &stmt.rows[stmt.current_row_idx - 1];
    if let Some(val) = row.values().get(N as usize) {
        match val {
            Value::Blob(b) => b.len() as i32,
            Value::Text(t) => t.len() as i32,
            Value::Null => 0,
            other => other.to_string().len() as i32,
        }
    } else {
        0
    }
}

/// Destroy a prepared statement object.
///
/// # Safety
/// Standard C ABI contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_finalize(pStmt: *mut sqlite3_stmt) -> i32 {
    if !pStmt.is_null() {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            drop(unsafe { Box::from_raw(pStmt) });
        }));
    }
    SQLITE_OK
}

/// Return binary BLOB bytes of column at index N.
///
/// # Safety
/// Standard C ABI contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_column_blob(pStmt: *mut sqlite3_stmt, N: i32) -> *const c_void {
    if pStmt.is_null() || N < 0 {
        return ptr::null();
    }
    let stmt = unsafe { &mut *pStmt };
    if stmt.current_row_idx == 0 || stmt.current_row_idx > stmt.rows.len() {
        return ptr::null();
    }
    let row = &stmt.rows[stmt.current_row_idx - 1];
    if let Some(val) = row.values().get(N as usize) {
        let bytes = match val {
            tapirus::traits::Value::Blob(b) => b.clone(),
            tapirus::traits::Value::Text(t) => t.as_bytes().to_vec(),
            tapirus::traits::Value::Null => return ptr::null(),
            other => other.to_string().into_bytes(),
        };
        stmt.current_blob_cache = Some(bytes);
        stmt.current_blob_cache
            .as_ref()
            .map_or(ptr::null(), |b| b.as_ptr() as *const c_void)
    } else {
        ptr::null()
    }
}

/// Reset a prepared statement back to its initial state.
///
/// # Safety
/// Standard C ABI contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_reset(pStmt: *mut sqlite3_stmt) -> i32 {
    if !pStmt.is_null() {
        let stmt = unsafe { &mut *pStmt };
        stmt.current_row_idx = 0;
        stmt.current_text_cache = None;
        stmt.current_blob_cache = None;
        if stmt.param_count > 0 {
            stmt.executed = false;
            stmt.rows.clear();
            stmt.columns.clear();
        }
    }
    SQLITE_OK
}

/// Return English-language text describing the most recent error.
///
/// # Safety
/// Standard C ABI contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_errmsg(db: *mut sqlite3) -> *const c_char {
    if db.is_null() {
        return c"out of memory".as_ptr();
    }
    let db_ref = unsafe { &*db };
    if let Some(ref err) = db_ref.last_error {
        err.as_ptr()
    } else {
        c"not an error".as_ptr()
    }
}

/// Return numeric error code for the most recent failed API call.
///
/// # Safety
/// Standard C ABI contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_errcode(db: *mut sqlite3) -> i32 {
    if db.is_null() {
        SQLITE_NOMEM
    } else {
        let db_ref = unsafe { &*db };
        if db_ref.last_error.is_some() {
            SQLITE_ERROR
        } else {
            SQLITE_OK
        }
    }
}

/// Return number of rows modified, inserted or deleted by the most recently completed statement.
///
/// # Safety
/// Standard C ABI contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_changes(db: *mut sqlite3) -> i32 {
    if db.is_null() {
        0
    } else {
        let db_ref = unsafe { &*db };
        db_ref.last_changes
    }
}

/// Return SQLite library version.
#[unsafe(no_mangle)]
pub extern "C" fn sqlite3_libversion() -> *const c_char {
    c"3.45.0-tapirusdb".as_ptr()
}

/// Return SQLite library version number.
#[unsafe(no_mangle)]
pub extern "C" fn sqlite3_libversion_number() -> i32 {
    3045000
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn test_sqlite_c_abi_roundtrip() {
        unsafe {
            let mut db: *mut sqlite3 = ptr::null_mut();
            let mem = CString::new(":memory:").unwrap();
            let rc = sqlite3_open_v2(mem.as_ptr(), &mut db, 0, ptr::null());
            assert_eq!(rc, SQLITE_OK);
            assert!(!db.is_null());

            // 1. Create table
            let create_sql = CString::new("CREATE TABLE kv (k INT, v TEXT);").unwrap();
            let mut stmt: *mut sqlite3_stmt = ptr::null_mut();
            let rc = sqlite3_prepare_v2(db, create_sql.as_ptr(), -1, &mut stmt, ptr::null_mut());
            assert_eq!(rc, SQLITE_OK);
            assert_eq!(sqlite3_step(stmt), SQLITE_DONE);
            assert_eq!(sqlite3_finalize(stmt), SQLITE_OK);

            // 2. Insert row
            let insert_sql = CString::new("INSERT INTO kv VALUES (42, 'Tapirus DB');").unwrap();
            let mut stmt2: *mut sqlite3_stmt = ptr::null_mut();
            let rc = sqlite3_prepare_v2(db, insert_sql.as_ptr(), -1, &mut stmt2, ptr::null_mut());
            assert_eq!(rc, SQLITE_OK);
            assert_eq!(sqlite3_step(stmt2), SQLITE_DONE);
            assert_eq!(sqlite3_changes(db), 1);
            assert_eq!(sqlite3_finalize(stmt2), SQLITE_OK);

            // 3. Query row
            let select_sql = CString::new("SELECT k, v FROM kv WHERE k = 42;").unwrap();
            let mut stmt3: *mut sqlite3_stmt = ptr::null_mut();
            let rc = sqlite3_prepare_v2(db, select_sql.as_ptr(), -1, &mut stmt3, ptr::null_mut());
            assert_eq!(rc, SQLITE_OK);
            assert_eq!(sqlite3_column_count(stmt3), 2);
            assert_eq!(sqlite3_step(stmt3), SQLITE_ROW);

            assert_eq!(sqlite3_column_int(stmt3, 0), 42);
            let text_ptr = sqlite3_column_text(stmt3, 1);
            assert!(!text_ptr.is_null());
            let text_val = CStr::from_ptr(text_ptr).to_str().unwrap();
            assert_eq!(text_val, "Tapirus DB");

            assert_eq!(sqlite3_step(stmt3), SQLITE_DONE);
            assert_eq!(sqlite3_finalize(stmt3), SQLITE_OK);

            // 4. Close database
            assert_eq!(sqlite3_close(db), SQLITE_OK);
        }
    }

    #[test]
    fn test_tapirus_ffi_params_and_ai_memory_roundtrip() {
        unsafe {
            let conn = tapirus_open_in_memory();
            assert!(!conn.is_null());
            let mut err: *mut c_char = ptr::null_mut();

            let create =
                CString::new("CREATE TABLE items (id INTEGER PRIMARY KEY, name TEXT);").unwrap();
            assert!(tapirus_execute(conn, create.as_ptr(), &mut err) >= 0);

            let insert = CString::new("INSERT INTO items (id, name) VALUES (?, ?);").unwrap();
            let params = CString::new(r#"[1, "O'Reilly; DROP TABLE items;--"]"#).unwrap();
            assert_eq!(
                tapirus_execute_params(conn, insert.as_ptr(), params.as_ptr(), &mut err),
                1
            );

            let select = CString::new("SELECT id, name FROM items WHERE id = ?;").unwrap();
            let qparams = CString::new("[1]").unwrap();
            let mut json_out: *mut c_char = ptr::null_mut();
            assert_eq!(
                tapirus_query_params_json(
                    conn,
                    select.as_ptr(),
                    qparams.as_ptr(),
                    &mut json_out,
                    &mut err
                ),
                0
            );
            let json_str = CStr::from_ptr(json_out).to_str().unwrap().to_owned();
            assert!(json_str.contains("O'Reilly; DROP TABLE items;--"));
            tapirus_free_string(json_out);

            // AI Memory FFI roundtrip
            let doc = CString::new(
                r#"{"content":"TapirusDB vector memory","tags":["ffi","ai"],"embedding":[1.0,0.0]}"#,
            )
            .unwrap();
            let mut mem_id: u64 = 0;
            assert_eq!(
                tapirus_store_memory_json(conn, doc.as_ptr(), &mut mem_id, &mut err),
                0
            );
            assert!(mem_id > 0);

            let search = CString::new(r#"{"embedding":[1.0,0.0],"limit":5}"#).unwrap();
            let mut search_out: *mut c_char = ptr::null_mut();
            assert_eq!(
                tapirus_search_memories_json(conn, search.as_ptr(), &mut search_out, &mut err),
                0
            );
            let hits = CStr::from_ptr(search_out).to_str().unwrap().to_owned();
            assert!(hits.contains("TapirusDB vector memory"));
            tapirus_free_string(search_out);

            let op = CString::new("stats").unwrap();
            let mut stats_out: *mut c_char = ptr::null_mut();
            assert_eq!(
                tapirus_memory_request_json(
                    conn,
                    op.as_ptr(),
                    ptr::null(),
                    &mut stats_out,
                    &mut err
                ),
                0
            );
            let stats_str = CStr::from_ptr(stats_out).to_str().unwrap().to_owned();
            assert!(stats_str.contains("\"documents\":1"));
            tapirus_free_string(stats_out);

            assert_eq!(tapirus_forget_memory(conn, mem_id, &mut err), 1);
            assert_eq!(tapirus_forget_memory(conn, mem_id, &mut err), 0);

            tapirus_close(conn);
        }
    }

    #[test]
    fn test_sqlite_bind_and_reset_cycle() {
        unsafe {
            let mut db: *mut sqlite3 = ptr::null_mut();
            let mem = CString::new(":memory:").unwrap();
            assert_eq!(
                sqlite3_open_v2(mem.as_ptr(), &mut db, 0, ptr::null()),
                SQLITE_OK
            );

            let create_sql = CString::new(
                "CREATE TABLE rec (id INTEGER PRIMARY KEY, label TEXT, score REAL, raw BLOB);",
            )
            .unwrap();
            let mut create_stmt: *mut sqlite3_stmt = ptr::null_mut();
            assert_eq!(
                sqlite3_prepare_v2(
                    db,
                    create_sql.as_ptr(),
                    -1,
                    &mut create_stmt,
                    ptr::null_mut()
                ),
                SQLITE_OK
            );
            assert_eq!(sqlite3_bind_parameter_count(create_stmt), 0);
            assert_eq!(sqlite3_step(create_stmt), SQLITE_DONE);
            sqlite3_finalize(create_stmt);

            let ins_sql = CString::new("INSERT INTO rec VALUES (?, ?, ?, ?);").unwrap();
            let mut ins_stmt: *mut sqlite3_stmt = ptr::null_mut();
            assert_eq!(
                sqlite3_prepare_v2(db, ins_sql.as_ptr(), -1, &mut ins_stmt, ptr::null_mut()),
                SQLITE_OK
            );
            assert_eq!(sqlite3_bind_parameter_count(ins_stmt), 4);
            assert_eq!(sqlite3_bind_int(ins_stmt, 5, 1), SQLITE_RANGE);

            assert_eq!(sqlite3_bind_int64(ins_stmt, 1, 10), SQLITE_OK);
            let txt = CString::new("alpha").unwrap();
            assert_eq!(
                sqlite3_bind_text(ins_stmt, 2, txt.as_ptr(), -1, (-1isize) as *const c_void),
                SQLITE_OK
            );
            assert_eq!(sqlite3_bind_double(ins_stmt, 3, 2.5), SQLITE_OK);
            let blob = [0xDEu8, 0xAD, 0xBE, 0xEF];
            assert_eq!(
                sqlite3_bind_blob(
                    ins_stmt,
                    4,
                    blob.as_ptr() as *const c_void,
                    blob.len() as i32,
                    ptr::null()
                ),
                SQLITE_OK
            );
            assert_eq!(sqlite3_step(ins_stmt), SQLITE_DONE);
            assert_eq!(sqlite3_changes(db), 1);

            // Reset & re-bind with NULL
            assert_eq!(sqlite3_reset(ins_stmt), SQLITE_OK);
            assert_eq!(sqlite3_clear_bindings(ins_stmt), SQLITE_OK);
            assert_eq!(sqlite3_bind_int(ins_stmt, 1, 11), SQLITE_OK);
            assert_eq!(sqlite3_bind_null(ins_stmt, 2), SQLITE_OK);
            assert_eq!(sqlite3_bind_double(ins_stmt, 3, 9.0), SQLITE_OK);
            assert_eq!(sqlite3_bind_null(ins_stmt, 4), SQLITE_OK);
            assert_eq!(sqlite3_step(ins_stmt), SQLITE_DONE);
            sqlite3_finalize(ins_stmt);

            let sel_sql = CString::new("SELECT label, score, raw FROM rec WHERE id = ?;").unwrap();
            let mut sel_stmt: *mut sqlite3_stmt = ptr::null_mut();
            assert_eq!(
                sqlite3_prepare_v2(db, sel_sql.as_ptr(), -1, &mut sel_stmt, ptr::null_mut()),
                SQLITE_OK
            );
            assert_eq!(sqlite3_bind_int(sel_stmt, 1, 10), SQLITE_OK);
            assert_eq!(sqlite3_step(sel_stmt), SQLITE_ROW);
            assert_eq!(sqlite3_column_count(sel_stmt), 3);
            assert_eq!(
                CStr::from_ptr(sqlite3_column_text(sel_stmt, 0))
                    .to_str()
                    .unwrap(),
                "alpha"
            );
            assert!((sqlite3_column_double(sel_stmt, 1) - 2.5).abs() < 1e-9);
            assert_eq!(sqlite3_column_bytes(sel_stmt, 2), 4);
            let blob_ptr = sqlite3_column_blob(sel_stmt, 2) as *const u8;
            assert!(!blob_ptr.is_null());
            assert_eq!(std::slice::from_raw_parts(blob_ptr, 4), &blob);
            assert_eq!(sqlite3_step(sel_stmt), SQLITE_DONE);
            sqlite3_finalize(sel_stmt);

            sqlite3_close(db);
        }
    }
}
