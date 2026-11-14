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
pub const SQLITE_READONLY: i32 = 8;
pub const SQLITE_IOERR: i32 = 10;
pub const SQLITE_CORRUPT: i32 = 11;
pub const SQLITE_CANTOPEN: i32 = 14;
pub const SQLITE_CONSTRAINT: i32 = 19;
pub const SQLITE_MISMATCH: i32 = 20;
pub const SQLITE_MISUSE: i32 = 21;
pub const SQLITE_RANGE: i32 = 25;
pub const SQLITE_ROW: i32 = 100;
pub const SQLITE_DONE: i32 = 101;

/// SQLite Open Flags
pub const SQLITE_OPEN_READONLY: i32 = 0x0000_0001;
pub const SQLITE_OPEN_READWRITE: i32 = 0x0000_0002;
pub const SQLITE_OPEN_CREATE: i32 = 0x0000_0004;
pub const SQLITE_OPEN_MEMORY: i32 = 0x0000_0080;

/// SQLite Column Datatypes
pub const SQLITE_INTEGER: i32 = 1;
pub const SQLITE_FLOAT: i32 = 2;
pub const SQLITE_TEXT: i32 = 3;
pub const SQLITE_BLOB: i32 = 4;
pub const SQLITE_NULL: i32 = 5;

/// Track C strings allocated by `sqlite3_exec` so `sqlite3_free` is memory-safe.
fn allocated_exec_strings() -> &'static std::sync::Mutex<std::collections::HashSet<usize>> {
    static SET: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<usize>>> =
        std::sync::OnceLock::new();
    SET.get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()))
}

fn alloc_sqlite_cstring(msg: &str) -> *mut c_char {
    let raw = CString::new(msg)
        .unwrap_or_else(|_| CString::new("sqlite3 error").unwrap())
        .into_raw();
    if let Ok(mut set) = allocated_exec_strings().lock() {
        set.insert(raw as usize);
    }
    raw
}

/// Callback type for `sqlite3_exec`.
#[allow(non_camel_case_types)]
pub type sqlite3_callback =
    Option<unsafe extern "C" fn(*mut c_void, i32, *mut *mut c_char, *mut *mut c_char) -> i32>;

/// Opaque SQLite Database Handle representation
#[repr(C)]
#[allow(non_camel_case_types)]
pub struct sqlite3 {
    pub(crate) conn: Connection,
    pub(crate) last_error: Option<CString>,
    pub(crate) last_errcode: i32,
    pub(crate) last_changes: i32,
    pub(crate) total_changes: i64,
    pub(crate) last_insert_rowid: i64,
    pub(crate) busy_timeout_ms: i32,
}

/// Opaque SQLite Statement Handle representation
#[repr(C)]
#[allow(non_camel_case_types)]
pub struct sqlite3_stmt {
    pub(crate) db: *mut sqlite3,
    pub(crate) raw_sql: CString,
    pub(crate) sql: String,
    pub(crate) is_query: bool,
    pub(crate) is_readonly: bool,
    pub(crate) param_count: usize,
    pub(crate) occurrence_slots: Vec<usize>,
    pub(crate) param_names: Vec<Option<CString>>,
    pub(crate) bindings: Vec<Value>,
    pub(crate) executed: bool,
    pub(crate) stepped: bool,
    pub(crate) finished: bool,
    pub(crate) rows: Vec<tapirus::traits::Row>,
    pub(crate) columns: Vec<CString>,
    pub(crate) current_row_idx: usize,
    pub(crate) current_text_cache: Option<CString>,
    pub(crate) current_blob_cache: Option<Vec<u8>>,
}

/// Return `(stmt_byte_end, tail_byte_offset)` for the first top-level statement in `sql`.
///
/// Handles quoted strings, line/block comments, and `BEGIN ... END` / `CASE ... END` blocks.
fn split_first_statement(sql: &str) -> (usize, usize) {
    let bytes = sql.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    let mut in_single = false;
    let mut in_double = false;
    let mut in_line_comment = false;
    let mut in_block_comment = false;
    let mut block_depth: usize = 0;
    let mut word = String::new();

    while i < len {
        let b = bytes[i];
        if in_line_comment {
            if b == b'\n' {
                in_line_comment = false;
            }
            i += 1;
            continue;
        }
        if in_block_comment {
            if b == b'*' && i + 1 < len && bytes[i + 1] == b'/' {
                in_block_comment = false;
                i += 2;
            } else {
                i += 1;
            }
            continue;
        }
        if in_single {
            if b == b'\'' {
                if i + 1 < len && bytes[i + 1] == b'\'' {
                    i += 2;
                    continue;
                }
                in_single = false;
            }
            i += 1;
            continue;
        }
        if in_double {
            if b == b'"' {
                if i + 1 < len && bytes[i + 1] == b'"' {
                    i += 2;
                    continue;
                }
                in_double = false;
            }
            i += 1;
            continue;
        }

        if b == b'-' && i + 1 < len && bytes[i + 1] == b'-' {
            word.clear();
            in_line_comment = true;
            i += 2;
            continue;
        }
        if b == b'/' && i + 1 < len && bytes[i + 1] == b'*' {
            word.clear();
            in_block_comment = true;
            i += 2;
            continue;
        }
        if b == b'\'' {
            word.clear();
            in_single = true;
            i += 1;
            continue;
        }
        if b == b'"' {
            word.clear();
            in_double = true;
            i += 1;
            continue;
        }

        if b.is_ascii_alphanumeric() || b == b'_' {
            word.push((b as char).to_ascii_uppercase());
            i += 1;
            continue;
        }
        if !word.is_empty() {
            if word == "BEGIN" || word == "CASE" {
                block_depth += 1;
            } else if word == "END" && block_depth > 0 {
                block_depth -= 1;
            }
            word.clear();
        }
        if b == b';' && block_depth == 0 {
            return (i + 1, i + 1);
        }
        i += 1;
    }

    (len, len)
}

struct PlaceholderPlan {
    normalized_sql: String,
    param_count: usize,
    occurrence_slots: Vec<usize>,
    param_names: Vec<Option<CString>>,
}

/// Normalize `?`, `?NNN`, `:name`, `@name`, `$name` placeholders into positional `?` bindings.
fn compile_sql_placeholders(sql: &str) -> PlaceholderPlan {
    let bytes = sql.as_bytes();
    let len = bytes.len();
    let mut normalized = String::with_capacity(len);
    let mut occurrence_slots = Vec::new();
    let mut param_names: Vec<Option<CString>> = Vec::new();
    let mut named_map: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut next_anon_slot: usize = 0;

    let mut i = 0;
    let mut in_single = false;
    let mut in_double = false;
    let mut in_line_comment = false;
    let mut in_block_comment = false;

    while i < len {
        let b = bytes[i];
        if in_line_comment {
            normalized.push(b as char);
            if b == b'\n' {
                in_line_comment = false;
            }
            i += 1;
            continue;
        }
        if in_block_comment {
            normalized.push(b as char);
            if b == b'*' && i + 1 < len && bytes[i + 1] == b'/' {
                normalized.push('/');
                in_block_comment = false;
                i += 2;
            } else {
                i += 1;
            }
            continue;
        }
        if in_single {
            let ch = sql[i..].chars().next().unwrap();
            normalized.push(ch);
            i += ch.len_utf8();
            if ch == '\'' {
                if i < len && bytes[i] == b'\'' {
                    normalized.push('\'');
                    i += 1;
                } else {
                    in_single = false;
                }
            }
            continue;
        }
        if in_double {
            let ch = sql[i..].chars().next().unwrap();
            normalized.push(ch);
            i += ch.len_utf8();
            if ch == '"' {
                if i < len && bytes[i] == b'"' {
                    normalized.push('"');
                    i += 1;
                } else {
                    in_double = false;
                }
            }
            continue;
        }

        if b == b'-' && i + 1 < len && bytes[i + 1] == b'-' {
            in_line_comment = true;
            normalized.push_str("--");
            i += 2;
            continue;
        }
        if b == b'/' && i + 1 < len && bytes[i + 1] == b'*' {
            in_block_comment = true;
            normalized.push_str("/*");
            i += 2;
            continue;
        }
        if b == b'\'' {
            in_single = true;
            normalized.push('\'');
            i += 1;
            continue;
        }
        if b == b'"' {
            in_double = true;
            normalized.push('"');
            i += 1;
            continue;
        }

        if b == b'?' {
            let start = i;
            i += 1;
            let num_start = i;
            while i < len && bytes[i].is_ascii_digit() {
                i += 1;
            }
            let slot = if i > num_start {
                let raw_name = &sql[start..i];
                if let Ok(num) = sql[num_start..i].parse::<usize>() {
                    if num >= 1 {
                        let slot_idx = num - 1;
                        if param_names.len() <= slot_idx {
                            param_names.resize(slot_idx + 1, None);
                        }
                        if param_names[slot_idx].is_none() {
                            param_names[slot_idx] = CString::new(raw_name).ok();
                        }
                        if num > next_anon_slot {
                            next_anon_slot = num;
                        }
                        slot_idx
                    } else {
                        let idx = next_anon_slot;
                        next_anon_slot += 1;
                        if param_names.len() <= idx {
                            param_names.resize(idx + 1, None);
                        }
                        idx
                    }
                } else {
                    let idx = next_anon_slot;
                    next_anon_slot += 1;
                    if param_names.len() <= idx {
                        param_names.resize(idx + 1, None);
                    }
                    idx
                }
            } else {
                while param_names.get(next_anon_slot).is_some_and(Option::is_some) {
                    next_anon_slot += 1;
                }
                let idx = next_anon_slot;
                next_anon_slot += 1;
                if param_names.len() <= idx {
                    param_names.resize(idx + 1, None);
                }
                idx
            };
            occurrence_slots.push(slot);
            normalized.push('?');
            continue;
        }

        if matches!(b, b':' | b'@' | b'$')
            && i + 1 < len
            && (bytes[i + 1].is_ascii_alphanumeric() || bytes[i + 1] == b'_')
        {
            let start = i;
            i += 1;
            while i < len && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            let name = &sql[start..i];
            let slot = if let Some(&existing) = named_map.get(name) {
                existing
            } else {
                while param_names.get(next_anon_slot).is_some_and(Option::is_some) {
                    next_anon_slot += 1;
                }
                let idx = next_anon_slot;
                next_anon_slot += 1;
                if param_names.len() <= idx {
                    param_names.resize(idx + 1, None);
                }
                param_names[idx] = CString::new(name).ok();
                named_map.insert(name.to_string(), idx);
                idx
            };
            occurrence_slots.push(slot);
            normalized.push('?');
            continue;
        }

        let ch = sql[i..].chars().next().unwrap();
        normalized.push(ch);
        i += ch.len_utf8();
    }

    let param_count = param_names.len();
    PlaceholderPlan {
        normalized_sql: normalized,
        param_count,
        occurrence_slots,
        param_names,
    }
}

fn strip_leading_sql_comments(mut sql: &str) -> &str {
    loop {
        sql = sql.trim_start();
        if let Some(rest) = sql.strip_prefix("--") {
            match rest.find('\n') {
                Some(idx) => sql = &rest[idx + 1..],
                None => return "",
            }
        } else if let Some(rest) = sql.strip_prefix("/*") {
            match rest.find("*/") {
                Some(idx) => sql = &rest[idx + 2..],
                None => return "",
            }
        } else {
            return sql;
        }
    }
}

fn is_query_sql(sql: &str) -> bool {
    let trimmed = strip_leading_sql_comments(sql).to_uppercase();
    trimmed.starts_with("SELECT")
        || trimmed.starts_with("WITH")
        || trimmed.starts_with("EXPLAIN")
        || trimmed.starts_with("GRAPH")
        || (trimmed.starts_with("PRAGMA") && !trimmed.contains('='))
}

fn map_error_to_sqlite_code(err: &tapirus::error::Error) -> i32 {
    use tapirus::error::Error;
    match err {
        Error::Busy(_) => SQLITE_BUSY,
        Error::ConstraintViolation(_) => SQLITE_CONSTRAINT,
        Error::TransactionError(msg) if msg.contains("read-only") => SQLITE_READONLY,
        Error::Corrupted(msg) if msg.contains("read-only") => SQLITE_READONLY,
        Error::Corrupted(_) => SQLITE_CORRUPT,
        Error::Io(_) => SQLITE_IOERR,
        _ => SQLITE_ERROR,
    }
}

unsafe fn execute_sqlite_stmt(stmt: &mut sqlite3_stmt) -> i32 {
    if stmt.db.is_null() {
        return SQLITE_MISUSE;
    }
    let db_ref = unsafe { &mut *stmt.db };
    let ordered_params: Vec<Value> = stmt
        .occurrence_slots
        .iter()
        .map(|&slot| stmt.bindings.get(slot).cloned().unwrap_or(Value::Null))
        .collect();

    if stmt.is_query {
        let result = if ordered_params.is_empty() {
            db_ref.conn.query(&stmt.sql)
        } else {
            db_ref.conn.query_with_params(&stmt.sql, &ordered_params)
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
                db_ref.last_errcode = SQLITE_OK;
                SQLITE_OK
            }
            Err(e) => {
                let rc = map_error_to_sqlite_code(&e);
                db_ref.last_error = Some(CString::new(e.to_string()).unwrap_or_default());
                db_ref.last_errcode = rc;
                rc
            }
        }
    } else {
        let head_upper = strip_leading_sql_comments(&stmt.sql).to_uppercase();
        let is_insert = head_upper.starts_with("INSERT") || head_upper.starts_with("REPLACE");
        let is_dml =
            is_insert || head_upper.starts_with("UPDATE") || head_upper.starts_with("DELETE");
        let result = if ordered_params.is_empty() {
            db_ref.conn.execute(&stmt.sql)
        } else {
            db_ref.conn.execute_with_params(&stmt.sql, &ordered_params)
        };
        match result {
            Ok(affected) => {
                if is_dml {
                    db_ref.last_changes = affected as i32;
                    db_ref.total_changes += affected as i64;
                }
                if is_insert && affected > 0 {
                    db_ref.last_insert_rowid = db_ref.conn.last_insert_rowid();
                }
                stmt.rows.clear();
                stmt.columns.clear();
                stmt.executed = true;
                stmt.current_row_idx = 0;
                db_ref.last_error = None;
                db_ref.last_errcode = SQLITE_OK;
                SQLITE_OK
            }
            Err(e) => {
                let rc = map_error_to_sqlite_code(&e);
                db_ref.last_error = Some(CString::new(e.to_string()).unwrap_or_default());
                db_ref.last_errcode = rc;
                rc
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
    unsafe {
        sqlite3_open_v2(
            filename,
            ppDb,
            SQLITE_OPEN_READWRITE | SQLITE_OPEN_CREATE,
            ptr::null(),
        )
    }
}

/// Open an SQLite compatible connection with flags.
///
/// # Safety
/// Standard C ABI contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_open_v2(
    filename: *const c_char,
    ppDb: *mut *mut sqlite3,
    flags: i32,
    _zVfs: *const c_char,
) -> i32 {
    if ppDb.is_null() {
        return SQLITE_MISUSE;
    }
    unsafe { *ppDb = ptr::null_mut() };

    let res = catch_unwind(AssertUnwindSafe(|| {
        if filename.is_null() {
            return SQLITE_MISUSE;
        }
        let c_str = unsafe { CStr::from_ptr(filename) };
        let path_str = match c_str.to_str() {
            Ok(s) => s,
            Err(_) => return SQLITE_ERROR,
        };

        let is_memory =
            path_str == ":memory:" || path_str.is_empty() || (flags & SQLITE_OPEN_MEMORY) != 0;
        let is_ro = (flags & SQLITE_OPEN_READONLY) != 0;

        let conn_res = if is_memory {
            Connection::open_in_memory()
        } else if is_ro {
            Connection::open_read_only(Path::new(path_str))
        } else {
            Connection::open_concurrent_with_config(
                Path::new(path_str),
                tapirus::Config {
                    concurrent_access: true,
                    busy_timeout: std::time::Duration::ZERO,
                    ..Default::default()
                },
            )
        };

        match conn_res {
            Ok(conn) => {
                let db_box = Box::new(sqlite3 {
                    conn,
                    last_error: None,
                    last_errcode: SQLITE_OK,
                    last_changes: 0,
                    total_changes: 0,
                    last_insert_rowid: 0,
                    busy_timeout_ms: 0,
                });
                unsafe { *ppDb = Box::into_raw(db_box) };
                SQLITE_OK
            }
            Err(e) => map_error_to_sqlite_code(&e).max(SQLITE_CANTOPEN),
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

/// Prepare a SQL statement for execution.
///
/// DML/DDL execution is strictly deferred until `sqlite3_step()`, matching SQLite semantics.
/// Supports multi-statement `pzTail` iteration and `?`, `?NNN`, `:name`, `@name`, `$name` binds.
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
    if db.is_null() || ppStmt.is_null() {
        return SQLITE_MISUSE;
    }
    unsafe { *ppStmt = ptr::null_mut() };
    if !pzTail.is_null() {
        unsafe { *pzTail = ptr::null() };
    }
    if zSql.is_null() {
        return SQLITE_OK;
    }

    let res = catch_unwind(AssertUnwindSafe(|| {
        let db_ref = unsafe { &mut *db };
        let full_sql = if nByte >= 0 {
            let slice = unsafe { std::slice::from_raw_parts(zSql as *const u8, nByte as usize) };
            match std::str::from_utf8(slice) {
                Ok(s) => s,
                Err(_) => {
                    db_ref.last_error = Some(CString::new("Invalid UTF-8 in SQL").unwrap());
                    db_ref.last_errcode = SQLITE_ERROR;
                    return SQLITE_ERROR;
                }
            }
        } else {
            match unsafe { CStr::from_ptr(zSql) }.to_str() {
                Ok(s) => s,
                Err(_) => {
                    db_ref.last_error = Some(CString::new("Invalid UTF-8 in SQL").unwrap());
                    db_ref.last_errcode = SQLITE_ERROR;
                    return SQLITE_ERROR;
                }
            }
        };

        let (stmt_end, tail_offset) = split_first_statement(full_sql);
        let stmt_slice = &full_sql[..stmt_end];
        if !pzTail.is_null() {
            unsafe { *pzTail = zSql.add(tail_offset) };
        }

        let active_sql = strip_leading_sql_comments(stmt_slice);
        let active_no_semi = active_sql.trim_end().trim_end_matches(';').trim();
        if active_no_semi.is_empty() {
            db_ref.last_error = None;
            db_ref.last_errcode = SQLITE_OK;
            return SQLITE_OK;
        }

        let plan = compile_sql_placeholders(stmt_slice);
        let is_query = is_query_sql(&plan.normalized_sql);

        // Validate non-query SQL syntax at prepare time without executing or mutating the database.
        if !is_query {
            let upper = strip_leading_sql_comments(&plan.normalized_sql).to_uppercase();
            let is_cypher = upper.starts_with("CREATE (")
                || upper.starts_with("CREATE(")
                || upper.starts_with("MATCH")
                || upper.starts_with("CYPHER");
            if !is_cypher {
                if let Err(e) = tapirus::sql::lexer::tokenize(&plan.normalized_sql) {
                    let rc = map_error_to_sqlite_code(&e);
                    db_ref.last_error = Some(CString::new(e.to_string()).unwrap_or_default());
                    db_ref.last_errcode = rc;
                    return rc;
                }
                if plan.occurrence_slots.is_empty()
                    && let Ok(toks) = tapirus::sql::lexer::tokenize(&plan.normalized_sql)
                    && !toks.is_empty()
                    && let Err(e) = tapirus::sql::parser::parse_tokens(&toks)
                {
                    let rc = map_error_to_sqlite_code(&e);
                    db_ref.last_error = Some(CString::new(e.to_string()).unwrap_or_default());
                    db_ref.last_errcode = rc;
                    return rc;
                }
            }
        }

        let raw_sql = CString::new(stmt_slice).unwrap_or_default();
        let mut stmt = Box::new(sqlite3_stmt {
            db,
            raw_sql,
            sql: plan.normalized_sql,
            is_query,
            is_readonly: is_query,
            param_count: plan.param_count,
            occurrence_slots: plan.occurrence_slots,
            param_names: plan.param_names,
            bindings: vec![Value::Null; plan.param_count],
            executed: false,
            stepped: false,
            finished: false,
            rows: Vec::new(),
            columns: Vec::new(),
            current_row_idx: 0,
            current_text_cache: None,
            current_blob_cache: None,
        });

        // Read-only unparameterized queries pre-populate column metadata so `sqlite3_column_count`
        // works before the first `sqlite3_step`, without ever mutating state at prepare time.
        if is_query && stmt.occurrence_slots.is_empty() {
            let rc = unsafe { execute_sqlite_stmt(&mut stmt) };
            if rc != SQLITE_OK {
                return rc;
            }
        }

        unsafe { *ppStmt = Box::into_raw(stmt) };
        db_ref.last_error = None;
        db_ref.last_errcode = SQLITE_OK;
        SQLITE_OK
    }));

    res.unwrap_or(SQLITE_ERROR)
}

unsafe fn set_sqlite_binding(pStmt: *mut sqlite3_stmt, index: i32, val: Value) -> i32 {
    if pStmt.is_null() {
        return SQLITE_MISUSE;
    }
    let stmt = unsafe { &mut *pStmt };
    if index < 1 || (index as usize) > stmt.bindings.len() {
        return SQLITE_RANGE;
    }
    stmt.bindings[(index - 1) as usize] = val;
    SQLITE_OK
}

/// Return the number of SQL parameters in a prepared statement.
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

/// Return the 1-based index of a named SQL parameter (`:name`, `@name`, `$name`, `?NNN`), or 0.
///
/// # Safety
/// Standard C ABI contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_bind_parameter_index(
    pStmt: *mut sqlite3_stmt,
    zName: *const c_char,
) -> i32 {
    if pStmt.is_null() || zName.is_null() {
        return 0;
    }
    let stmt = unsafe { &*pStmt };
    let Ok(target) = unsafe { CStr::from_ptr(zName) }.to_str() else {
        return 0;
    };
    for (idx, name_opt) in stmt.param_names.iter().enumerate() {
        if let Some(name) = name_opt
            && name.to_str().ok() == Some(target)
        {
            return (idx + 1) as i32;
        }
    }
    0
}

/// Return the name of the `N`-th (1-based) SQL parameter, or NULL if unnamed/out-of-range.
///
/// # Safety
/// Standard C ABI contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_bind_parameter_name(
    pStmt: *mut sqlite3_stmt,
    N: i32,
) -> *const c_char {
    if pStmt.is_null() || N < 1 {
        return ptr::null();
    }
    let stmt = unsafe { &*pStmt };
    stmt.param_names
        .get((N - 1) as usize)
        .and_then(|opt| opt.as_ref())
        .map_or(ptr::null(), |c| c.as_ptr())
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
        stmt.stepped = true;
        if !stmt.executed {
            let rc = unsafe { execute_sqlite_stmt(stmt) };
            if rc != SQLITE_OK {
                stmt.finished = true;
                return rc;
            }
        }
        if stmt.current_row_idx < stmt.rows.len() {
            stmt.current_row_idx += 1;
            stmt.finished = false;
            SQLITE_ROW
        } else {
            stmt.finished = true;
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
        stmt.stepped = false;
        stmt.finished = false;
        stmt.current_text_cache = None;
        stmt.current_blob_cache = None;
        if !stmt.is_query || !stmt.occurrence_slots.is_empty() {
            stmt.executed = false;
            stmt.rows.clear();
            if !stmt.occurrence_slots.is_empty() {
                stmt.columns.clear();
            }
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
            db_ref.last_errcode
        } else {
            SQLITE_OK
        }
    }
}

/// Return extended numeric error code for the most recent failed API call.
///
/// # Safety
/// Standard C ABI contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_extended_errcode(db: *mut sqlite3) -> i32 {
    unsafe { sqlite3_errcode(db) }
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
        unsafe { &*db }.last_changes
    }
}

/// Return total number of rows inserted, modified or deleted since this database connection opened.
///
/// # Safety
/// Standard C ABI contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_total_changes(db: *mut sqlite3) -> i32 {
    if db.is_null() {
        0
    } else {
        unsafe { &*db }.total_changes as i32
    }
}

/// Return the rowid of the most recent successful `INSERT` on the database connection.
///
/// # Safety
/// Standard C ABI contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_last_insert_rowid(db: *mut sqlite3) -> i64 {
    if db.is_null() {
        0
    } else {
        unsafe { &*db }.last_insert_rowid
    }
}

/// Configure a busy handler timeout (in milliseconds) for concurrent write locks.
///
/// # Safety
/// Standard C ABI contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_busy_timeout(db: *mut sqlite3, ms: i32) -> i32 {
    if db.is_null() {
        return SQLITE_MISUSE;
    }
    let db_ref = unsafe { &mut *db };
    db_ref.busy_timeout_ms = ms.max(0);
    db_ref
        .conn
        .set_busy_timeout(std::time::Duration::from_millis(
            db_ref.busy_timeout_ms as u64,
        ));
    SQLITE_OK
}

/// Return `1` if the connection is in autocommit mode, or `0` inside an explicit transaction.
///
/// # Safety
/// Standard C ABI contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_get_autocommit(db: *mut sqlite3) -> i32 {
    if db.is_null() {
        1
    } else {
        i32::from(!unsafe { &*db }.conn.is_in_transaction())
    }
}

/// Return the original SQL text used to prepare `pStmt`.
///
/// # Safety
/// Standard C ABI contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_sql(pStmt: *mut sqlite3_stmt) -> *const c_char {
    if pStmt.is_null() {
        ptr::null()
    } else {
        unsafe { &*pStmt }.raw_sql.as_ptr()
    }
}

/// Return `1` if `pStmt` makes no direct changes to the database file, or `0` otherwise.
///
/// # Safety
/// Standard C ABI contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_stmt_readonly(pStmt: *mut sqlite3_stmt) -> i32 {
    if pStmt.is_null() {
        1
    } else {
        i32::from(unsafe { &*pStmt }.is_readonly)
    }
}

/// Return `1` if `pStmt` has been stepped and has not yet completed/reset, or `0` otherwise.
///
/// # Safety
/// Standard C ABI contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_stmt_busy(pStmt: *mut sqlite3_stmt) -> i32 {
    if pStmt.is_null() {
        0
    } else {
        let stmt = unsafe { &*pStmt };
        i32::from(stmt.stepped && !stmt.finished)
    }
}

/// Return the number of columns in the current row of `pStmt` (or 0 if no row is active).
///
/// # Safety
/// Standard C ABI contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_data_count(pStmt: *mut sqlite3_stmt) -> i32 {
    if pStmt.is_null() {
        return 0;
    }
    let stmt = unsafe { &*pStmt };
    if stmt.current_row_idx == 0 || stmt.current_row_idx > stmt.rows.len() || stmt.finished {
        0
    } else {
        stmt.columns.len() as i32
    }
}

/// Free memory previously allocated by `sqlite3_exec` (`pzErrMsg`).
///
/// # Safety
/// `ptr` must be NULL or a pointer previously returned via `sqlite3_exec`'s error out parameter.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_free(ptr_arg: *mut c_void) {
    if ptr_arg.is_null() {
        return;
    }
    let addr = ptr_arg as usize;
    let owned = allocated_exec_strings()
        .lock()
        .map(|mut set| set.remove(&addr))
        .unwrap_or(false);
    if owned {
        unsafe { drop(CString::from_raw(ptr_arg as *mut c_char)) };
    }
}

/// Execute zero or more `;`-separated SQL statements in one call with an optional row callback.
///
/// # Safety
/// Standard C ABI contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_exec(
    db: *mut sqlite3,
    sql: *const c_char,
    callback: sqlite3_callback,
    arg: *mut c_void,
    errmsg: *mut *mut c_char,
) -> i32 {
    if !errmsg.is_null() {
        unsafe { *errmsg = ptr::null_mut() };
    }
    if db.is_null() {
        return SQLITE_MISUSE;
    }
    if sql.is_null() {
        return SQLITE_OK;
    }

    let res = catch_unwind(AssertUnwindSafe(|| {
        let mut cursor = sql;
        loop {
            let mut stmt: *mut sqlite3_stmt = ptr::null_mut();
            let mut tail: *const c_char = ptr::null();
            let rc = unsafe { sqlite3_prepare_v2(db, cursor, -1, &mut stmt, &mut tail) };
            if rc != SQLITE_OK {
                if !errmsg.is_null() {
                    let msg = unsafe { CStr::from_ptr(sqlite3_errmsg(db)) }
                        .to_string_lossy()
                        .into_owned();
                    unsafe { *errmsg = alloc_sqlite_cstring(&msg) };
                }
                return rc;
            }

            if !stmt.is_null() {
                loop {
                    let step_rc = unsafe { sqlite3_step(stmt) };
                    if step_rc == SQLITE_ROW {
                        if let Some(cb) = callback {
                            let ncol = unsafe { sqlite3_column_count(stmt) };
                            let mut name_ptrs: Vec<*mut c_char> =
                                Vec::with_capacity(ncol.max(0) as usize);
                            let mut val_cstrings: Vec<Option<CString>> =
                                Vec::with_capacity(ncol.max(0) as usize);
                            let mut val_ptrs: Vec<*mut c_char> =
                                Vec::with_capacity(ncol.max(0) as usize);
                            for col_idx in 0..ncol {
                                let cname = unsafe { sqlite3_column_name(stmt, col_idx) };
                                name_ptrs.push(cname as *mut c_char);
                                if unsafe { sqlite3_column_type(stmt, col_idx) } == SQLITE_NULL {
                                    val_cstrings.push(None);
                                    val_ptrs.push(ptr::null_mut());
                                } else {
                                    let txt_ptr = unsafe { sqlite3_column_text(stmt, col_idx) };
                                    if txt_ptr.is_null() {
                                        val_cstrings.push(None);
                                        val_ptrs.push(ptr::null_mut());
                                    } else {
                                        let cloned = unsafe { CStr::from_ptr(txt_ptr) }.to_owned();
                                        val_ptrs.push(cloned.as_ptr() as *mut c_char);
                                        val_cstrings.push(Some(cloned));
                                    }
                                }
                            }
                            let cb_rc = unsafe {
                                cb(arg, ncol, val_ptrs.as_mut_ptr(), name_ptrs.as_mut_ptr())
                            };
                            if cb_rc != 0 {
                                unsafe { sqlite3_finalize(stmt) };
                                if !errmsg.is_null() {
                                    unsafe { *errmsg = alloc_sqlite_cstring("query aborted") };
                                }
                                let db_ref = unsafe { &mut *db };
                                db_ref.last_error = Some(CString::new("query aborted").unwrap());
                                db_ref.last_errcode = SQLITE_ABORT;
                                return SQLITE_ABORT;
                            }
                        }
                    } else if step_rc == SQLITE_DONE {
                        break;
                    } else {
                        unsafe { sqlite3_finalize(stmt) };
                        if !errmsg.is_null() {
                            let msg = unsafe { CStr::from_ptr(sqlite3_errmsg(db)) }
                                .to_string_lossy()
                                .into_owned();
                            unsafe { *errmsg = alloc_sqlite_cstring(&msg) };
                        }
                        return step_rc;
                    }
                }
                unsafe { sqlite3_finalize(stmt) };
            }

            if tail.is_null() || unsafe { *tail } == 0 {
                break;
            }
            cursor = tail;
        }
        SQLITE_OK
    }));

    res.unwrap_or(SQLITE_ERROR)
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

    #[test]
    fn test_sqlite_orm_completeness_exec_deferred_named_binds_and_status() {
        unsafe extern "C" fn collect_callback(
            ctx: *mut c_void,
            ncol: i32,
            vals: *mut *mut c_char,
            cols: *mut *mut c_char,
        ) -> i32 {
            let out = unsafe { &mut *(ctx as *mut Vec<(String, String)>) };
            for i in 0..ncol as usize {
                let col_name = unsafe { CStr::from_ptr(*cols.add(i)) }
                    .to_string_lossy()
                    .into_owned();
                let val_str = if unsafe { *vals.add(i) }.is_null() {
                    "NULL".to_string()
                } else {
                    unsafe { CStr::from_ptr(*vals.add(i)) }
                        .to_string_lossy()
                        .into_owned()
                };
                out.push((col_name, val_str));
            }
            0
        }

        unsafe {
            let mut db: *mut sqlite3 = ptr::null_mut();
            let mem = CString::new(":memory:").unwrap();
            assert_eq!(sqlite3_open(mem.as_ptr(), &mut db), SQLITE_OK);
            assert_eq!(sqlite3_busy_timeout(db, 2000), SQLITE_OK);
            assert_eq!(sqlite3_get_autocommit(db), 1);

            // 1. sqlite3_exec multi-statement + trigger semicolon inside BEGIN..END
            let setup_sql = CString::new(
                "CREATE TABLE users (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL, age INTEGER CHECK (age >= 0));
                 CREATE TABLE audit_log (id INTEGER PRIMARY KEY AUTOINCREMENT, note TEXT);
                 CREATE TRIGGER log_ins AFTER INSERT ON users BEGIN INSERT INTO audit_log (note) VALUES ('added'); END;
                 INSERT INTO users (name, age) VALUES ('Alice', 30);"
            )
            .unwrap();
            let mut err_ptr: *mut c_char = ptr::null_mut();
            let rc = sqlite3_exec(db, setup_sql.as_ptr(), None, ptr::null_mut(), &mut err_ptr);
            assert_eq!(rc, SQLITE_OK);
            assert!(err_ptr.is_null());
            assert_eq!(sqlite3_last_insert_rowid(db), 1);
            assert_eq!(sqlite3_total_changes(db), 1);

            // Verify callback in sqlite3_exec
            let mut captured: Vec<(String, String)> = Vec::new();
            let query_sql = CString::new("SELECT id, name FROM users;").unwrap();
            let rc = sqlite3_exec(
                db,
                query_sql.as_ptr(),
                Some(collect_callback),
                (&mut captured as *mut Vec<(String, String)>) as *mut c_void,
                &mut err_ptr,
            );
            assert_eq!(rc, SQLITE_OK);
            assert_eq!(
                captured,
                vec![("id".into(), "1".into()), ("name".into(), "Alice".into())]
            );

            // 2. Prepare DML does NOT execute until sqlite3_step!
            let deferred_sql =
                CString::new("INSERT INTO users (name, age) VALUES ('Ghost', 99);").unwrap();
            let mut deferred_stmt: *mut sqlite3_stmt = ptr::null_mut();
            assert_eq!(
                sqlite3_prepare_v2(
                    db,
                    deferred_sql.as_ptr(),
                    -1,
                    &mut deferred_stmt,
                    ptr::null_mut()
                ),
                SQLITE_OK
            );
            assert_eq!(sqlite3_stmt_readonly(deferred_stmt), 0);
            assert_eq!(sqlite3_stmt_busy(deferred_stmt), 0);
            assert!(!sqlite3_sql(deferred_stmt).is_null());
            // Finalize without stepping -> must NOT insert Ghost!
            sqlite3_finalize(deferred_stmt);
            let mut count_check: Vec<(String, String)> = Vec::new();
            let cnt_sql = CString::new("SELECT COUNT(*) FROM users;").unwrap();
            assert_eq!(
                sqlite3_exec(
                    db,
                    cnt_sql.as_ptr(),
                    Some(collect_callback),
                    (&mut count_check as *mut Vec<(String, String)>) as *mut c_void,
                    ptr::null_mut()
                ),
                SQLITE_OK
            );
            assert_eq!(count_check[0].1, "1");

            // 3. Named (:name, @name) and numbered (?2) parameters + bind_parameter_index/name
            let named_sql = CString::new(
                "INSERT INTO users (name, age) VALUES (:uname, ?2); SELECT name, age FROM users WHERE name = @target;"
            )
            .unwrap();
            let mut stmt1: *mut sqlite3_stmt = ptr::null_mut();
            let mut tail: *const c_char = ptr::null();
            assert_eq!(
                sqlite3_prepare_v2(db, named_sql.as_ptr(), -1, &mut stmt1, &mut tail),
                SQLITE_OK
            );
            assert!(!tail.is_null());
            assert_eq!(sqlite3_bind_parameter_count(stmt1), 2);
            let p_uname = CString::new(":uname").unwrap();
            let p_two = CString::new("?2").unwrap();
            assert_eq!(sqlite3_bind_parameter_index(stmt1, p_uname.as_ptr()), 1);
            assert_eq!(sqlite3_bind_parameter_index(stmt1, p_two.as_ptr()), 2);
            assert_eq!(
                CStr::from_ptr(sqlite3_bind_parameter_name(stmt1, 1))
                    .to_str()
                    .unwrap(),
                ":uname"
            );
            let bob = CString::new("Bob").unwrap();
            sqlite3_bind_text(stmt1, 1, bob.as_ptr(), -1, ptr::null());
            sqlite3_bind_int(stmt1, 2, 25);
            assert_eq!(sqlite3_step(stmt1), SQLITE_DONE);
            assert_eq!(sqlite3_last_insert_rowid(db), 2);
            sqlite3_finalize(stmt1);

            // Second statement via pzTail
            let mut stmt2: *mut sqlite3_stmt = ptr::null_mut();
            assert_eq!(
                sqlite3_prepare_v2(db, tail, -1, &mut stmt2, &mut tail),
                SQLITE_OK
            );
            assert_eq!(sqlite3_stmt_readonly(stmt2), 1);
            let p_target = CString::new("@target").unwrap();
            assert_eq!(sqlite3_bind_parameter_index(stmt2, p_target.as_ptr()), 1);
            sqlite3_bind_text(stmt2, 1, bob.as_ptr(), -1, ptr::null());
            assert_eq!(sqlite3_data_count(stmt2), 0);
            assert_eq!(sqlite3_step(stmt2), SQLITE_ROW);
            assert_eq!(sqlite3_stmt_busy(stmt2), 1);
            assert_eq!(sqlite3_data_count(stmt2), 2);
            assert_eq!(sqlite3_column_int(stmt2, 1), 25);
            assert_eq!(sqlite3_step(stmt2), SQLITE_DONE);
            assert_eq!(sqlite3_stmt_busy(stmt2), 0);
            sqlite3_finalize(stmt2);

            // 4. Autocommit tracks explicit transaction + Constraint error returns SQLITE_CONSTRAINT + sqlite3_free
            let tx_begin = CString::new("BEGIN;").unwrap();
            assert_eq!(
                sqlite3_exec(
                    db,
                    tx_begin.as_ptr(),
                    None,
                    ptr::null_mut(),
                    ptr::null_mut()
                ),
                SQLITE_OK
            );
            assert_eq!(sqlite3_get_autocommit(db), 0);
            let bad_age =
                CString::new("INSERT INTO users (name, age) VALUES ('Bad', -5);").unwrap();
            let bad_rc = sqlite3_exec(db, bad_age.as_ptr(), None, ptr::null_mut(), &mut err_ptr);
            assert_eq!(bad_rc, SQLITE_CONSTRAINT);
            assert_eq!(sqlite3_errcode(db), SQLITE_CONSTRAINT);
            assert_eq!(sqlite3_extended_errcode(db), SQLITE_CONSTRAINT);
            assert!(!err_ptr.is_null());
            sqlite3_free(err_ptr as *mut c_void);
            let tx_rb = CString::new("ROLLBACK;").unwrap();
            assert_eq!(
                sqlite3_exec(db, tx_rb.as_ptr(), None, ptr::null_mut(), ptr::null_mut()),
                SQLITE_OK
            );
            assert_eq!(sqlite3_get_autocommit(db), 1);

            sqlite3_close(db);
        }
    }
}
