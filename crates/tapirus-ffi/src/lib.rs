//! # TapirusDB C FFI Layer (`tapirus-ffi`)
//!
//! Provides a standardized C-compatible Application Binary Interface (ABI)
//! allowing external languages (Python, C, C++, Swift, Kotlin, Go, Zig, Node.js)
//! to embed TapirusDB as a shared (`.so`, `.dll`, `.dylib`) or static library.

use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;
use std::ptr;
use tapirus::Connection;

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
    if !err_msg_out.is_null() {
        if let Ok(c_msg) = CString::new(msg) {
            unsafe { *err_msg_out = c_msg.into_raw() };
        }
    }
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
    pub(crate) sql: String,
    pub(crate) rows: Vec<tapirus::traits::Row>,
    pub(crate) columns: Vec<CString>,
    pub(crate) current_row_idx: usize,
    pub(crate) current_text_cache: Option<CString>,
}

/// Open an SQLite compatible connection.
///
/// # Safety
/// Standard C ABI contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_open(filename: *const c_char, ppDb: *mut *mut sqlite3) -> i32 {
    sqlite3_open_v2(filename, ppDb, 0, ptr::null())
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
    sqlite3_close(db)
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

        let trimmed = sql_str.trim().to_uppercase();
        if trimmed.starts_with("SELECT")
            || trimmed.starts_with("WITH")
            || trimmed.starts_with("EXPLAIN")
            || trimmed.starts_with("GRAPH")
        {
            match db_ref.conn.query(sql_str) {
                Ok(rows) => {
                    let mut cols = Vec::new();
                    if let Some(first) = rows.first() {
                        for c in first.columns() {
                            cols.push(CString::new(c.as_str()).unwrap_or_default());
                        }
                    }
                    let stmt = Box::new(sqlite3_stmt {
                        sql: sql_str.to_string(),
                        rows,
                        columns: cols,
                        current_row_idx: 0,
                        current_text_cache: None,
                    });
                    unsafe { *ppStmt = Box::into_raw(stmt) };
                    SQLITE_OK
                }
                Err(e) => {
                    db_ref.last_error = Some(CString::new(e.to_string()).unwrap_or_default());
                    SQLITE_ERROR
                }
            }
        } else {
            match db_ref.conn.execute(sql_str) {
                Ok(affected) => {
                    db_ref.last_changes = affected as i32;
                    let stmt = Box::new(sqlite3_stmt {
                        sql: sql_str.to_string(),
                        rows: Vec::new(),
                        columns: Vec::new(),
                        current_row_idx: 0,
                        current_text_cache: None,
                    });
                    unsafe { *ppStmt = Box::into_raw(stmt) };
                    SQLITE_OK
                }
                Err(e) => {
                    db_ref.last_error = Some(CString::new(e.to_string()).unwrap_or_default());
                    SQLITE_ERROR
                }
            }
        }
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
        stmt.current_text_cache.as_ref().map_or(ptr::null(), |c| c.as_ptr())
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
    sqlite3_column_int64(pStmt, N) as i32
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
    let ptr = sqlite3_column_text(pStmt, N);
    if ptr.is_null() {
        0
    } else {
        unsafe { CStr::from_ptr(ptr) }.to_bytes().len() as i32
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

/// Reset a prepared statement back to its initial state.
///
/// # Safety
/// Standard C ABI contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_reset(pStmt: *mut sqlite3_stmt) -> i32 {
    if !pStmt.is_null() {
        let stmt = unsafe { &mut *pStmt };
        stmt.current_row_idx = 0;
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
        return b"out of memory\0".as_ptr() as *const c_char;
    }
    let db_ref = unsafe { &*db };
    if let Some(ref err) = db_ref.last_error {
        err.as_ptr()
    } else {
        b"not an error\0".as_ptr() as *const c_char
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
    b"3.45.0-tapirusdb\0".as_ptr() as *const c_char
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
}
