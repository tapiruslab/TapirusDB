//! Seventh-Wave Forensic Hardening Integration Tests for TapirusDB
//!
//! Validates:
//! 1. `SELECT COUNT(1)` and digit literals counting total rows correctly
//! 2. `NOT NULL` column constraint enforcement during INSERT and UPDATE
//! 3. Standard SQL `IS NULL` and `IS NOT NULL` predicate filtering
//! 4. Standard SQL `BETWEEN val1 AND val2` range evaluation
//! 5. Escape sequence translation in string literals (`\n`, `\t`, `\r`, `\\`, etc.)
//! 6. Comment-only SQL queries returning Ok without throwing syntax errors
//! 7. WAL checkpoint sequential page flush order
//! 8. Case-insensitive column matching in Row and FromValue implementations for bool, u64, and usize

use tapirus::traits::{Row, Value};
use tapirus::{Connection, Error};

#[test]
fn test_count_1_and_digits_aggregate() {
    let conn = Connection::open_in_memory().expect("Open DB");
    conn.execute("CREATE TABLE items (id INTEGER PRIMARY KEY, note TEXT)")
        .expect("Create table");

    conn.execute("INSERT INTO items (id, note) VALUES (1, 'alpha')")
        .expect("Insert 1");
    conn.execute("INSERT INTO items (id, note) VALUES (2, NULL)")
        .expect("Insert 2");
    conn.execute("INSERT INTO items (id, note) VALUES (3, 'gamma')")
        .expect("Insert 3");

    // COUNT(*) should be 3
    let rows_star = conn.query("SELECT COUNT(*) FROM items").expect("COUNT(*)");
    assert_eq!(rows_star[0].get_idx::<i64>(0).unwrap(), 3);

    // COUNT(1) should be 3 (not 0!)
    let rows_one = conn.query("SELECT COUNT(1) FROM items").expect("COUNT(1)");
    assert_eq!(rows_one[0].get_idx::<i64>(0).unwrap(), 3);

    // COUNT(42) should be 3
    let rows_num = conn.query("SELECT COUNT(42) FROM items").expect("COUNT(42)");
    assert_eq!(rows_num[0].get_idx::<i64>(0).unwrap(), 3);

    // COUNT(note) should be 2 because row 2 has NULL note
    let rows_col = conn.query("SELECT COUNT(note) FROM items").expect("COUNT(note)");
    assert_eq!(rows_col[0].get_idx::<i64>(0).unwrap(), 2);
}

#[test]
fn test_not_null_constraint_enforcement_on_insert_and_update() {
    let conn = Connection::open_in_memory().expect("Open DB");
    conn.execute(
        "CREATE TABLE accounts (id INTEGER PRIMARY KEY, username TEXT NOT NULL, bio TEXT)",
    )
    .expect("Create table");

    // 1. INSERT with explicit NULL on NOT NULL column must fail
    let res = conn.execute("INSERT INTO accounts (id, username, bio) VALUES (1, NULL, 'coder')");
    match res {
        Err(Error::ConstraintViolation(msg)) => {
            assert!(
                msg.contains("NOT NULL constraint failed"),
                "Expected NOT NULL violation error, got: {msg}"
            );
        }
        other => panic!("Expected ConstraintViolation for NULL username, got: {other:?}"),
    }

    // 2. INSERT omitting NOT NULL column must fail (defaults to NULL)
    let res = conn.execute("INSERT INTO accounts (id, bio) VALUES (2, 'coder')");
    match res {
        Err(Error::ConstraintViolation(msg)) => {
            assert!(
                msg.contains("NOT NULL constraint failed"),
                "Expected NOT NULL violation error, got: {msg}"
            );
        }
        other => panic!("Expected ConstraintViolation for omitted username, got: {other:?}"),
    }

    // 3. Valid INSERT with valid username must succeed
    conn.execute("INSERT INTO accounts (id, username, bio) VALUES (3, 'alice', 'hacker')")
        .expect("Valid insert");

    // 4. UPDATE setting NOT NULL column to NULL must fail
    let res = conn.execute("UPDATE accounts SET username = NULL WHERE id = 3");
    match res {
        Err(Error::ConstraintViolation(msg)) => {
            assert!(
                msg.contains("NOT NULL constraint failed"),
                "Expected NOT NULL violation error, got: {msg}"
            );
        }
        other => panic!("Expected ConstraintViolation for UPDATE username = NULL, got: {other:?}"),
    }

    // 5. UPDATE setting nullable column (bio) to NULL must succeed
    conn.execute("UPDATE accounts SET bio = NULL WHERE id = 3")
        .expect("Update nullable column");

    let rows = conn
        .query("SELECT username, bio FROM accounts WHERE id = 3")
        .expect("Query");
    assert_eq!(rows[0].get::<String>("username").unwrap(), "alice");
    assert_eq!(rows[0].get::<Option<String>>("bio").unwrap(), None);
}

#[test]
fn test_is_null_and_is_not_null_operators() {
    let conn = Connection::open_in_memory().expect("Open DB");
    conn.execute("CREATE TABLE tasks (id INTEGER PRIMARY KEY, completed_at TEXT)")
        .expect("Create table");

    conn.execute("INSERT INTO tasks (id, completed_at) VALUES (1, '2026-09-01')")
        .expect("Insert 1");
    conn.execute("INSERT INTO tasks (id, completed_at) VALUES (2, NULL)")
        .expect("Insert 2");
    conn.execute("INSERT INTO tasks (id, completed_at) VALUES (3, '2026-09-15')")
        .expect("Insert 3");
    conn.execute("INSERT INTO tasks (id, completed_at) VALUES (4, NULL)")
        .expect("Insert 4");

    // IS NULL query
    let null_rows = conn
        .query("SELECT id FROM tasks WHERE completed_at IS NULL ORDER BY id ASC")
        .expect("Query IS NULL");
    assert_eq!(null_rows.len(), 2);
    assert_eq!(null_rows[0].get::<i64>("id").unwrap(), 2);
    assert_eq!(null_rows[1].get::<i64>("id").unwrap(), 4);

    // IS NOT NULL query
    let not_null_rows = conn
        .query("SELECT id FROM tasks WHERE completed_at IS NOT NULL ORDER BY id ASC")
        .expect("Query IS NOT NULL");
    assert_eq!(not_null_rows.len(), 2);
    assert_eq!(not_null_rows[0].get::<i64>("id").unwrap(), 1);
    assert_eq!(not_null_rows[1].get::<i64>("id").unwrap(), 3);
}

#[test]
fn test_between_operator() {
    let conn = Connection::open_in_memory().expect("Open DB");
    conn.execute("CREATE TABLE scores (player TEXT, points INTEGER)")
        .expect("Create table");

    conn.execute("INSERT INTO scores (player, points) VALUES ('p1', 10)")
        .expect("Insert");
    conn.execute("INSERT INTO scores (player, points) VALUES ('p2', 25)")
        .expect("Insert");
    conn.execute("INSERT INTO scores (player, points) VALUES ('p3', 50)")
        .expect("Insert");
    conn.execute("INSERT INTO scores (player, points) VALUES ('p4', 75)")
        .expect("Insert");
    conn.execute("INSERT INTO scores (player, points) VALUES ('p5', 100)")
        .expect("Insert");

    // Range BETWEEN 25 AND 75 inclusive
    let rows = conn
        .query("SELECT player, points FROM scores WHERE points BETWEEN 25 AND 75 ORDER BY points ASC")
        .expect("Query BETWEEN");
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].get::<String>("player").unwrap(), "p2");
    assert_eq!(rows[1].get::<String>("player").unwrap(), "p3");
    assert_eq!(rows[2].get::<String>("player").unwrap(), "p4");
}

#[test]
fn test_escape_sequences_in_string_literals() {
    let conn = Connection::open_in_memory().expect("Open DB");
    conn.execute("CREATE TABLE docs (id INTEGER PRIMARY KEY, content TEXT)")
        .expect("Create table");

    // Insert literal containing \n, \t, \\, \r
    conn.execute("INSERT INTO docs (id, content) VALUES (1, 'Line1\\nLine2\\tTabbed\\\\Backslash\\rReturn')")
        .expect("Insert with escapes");

    let rows = conn.query("SELECT content FROM docs WHERE id = 1").expect("Query");
    let content: String = rows[0].get("content").expect("Get content");

    assert!(
        content.contains('\n'),
        "Content should contain a real newline byte"
    );
    assert!(
        content.contains('\t'),
        "Content should contain a real tab byte"
    );
    assert!(
        content.contains('\\'),
        "Content should contain a real backslash byte"
    );
    assert!(
        content.contains('\r'),
        "Content should contain a real carriage return byte"
    );
    assert_eq!(content, "Line1\nLine2\tTabbed\\Backslash\rReturn");
}

#[test]
fn test_comment_only_sql_queries_and_prepared_statements() {
    let conn = Connection::open_in_memory().expect("Open DB");

    // Line comment execution
    let affected = conn
        .execute("-- Just a single-line comment\n")
        .expect("Execute line comment");
    assert_eq!(affected, 0);

    // Block comment execution
    let affected_block = conn
        .execute("/* This is a block comment */")
        .expect("Execute block comment");
    assert_eq!(affected_block, 0);

    // Empty query returning no rows
    let rows = conn
        .query("-- Only comments in query\n")
        .expect("Query comment");
    assert!(rows.is_empty());

    // Prepared statement on comment
    let stmt = conn.prepare("-- Prepared comment\n").expect("Prepare");
    assert_eq!(stmt.execute(&[]).expect("Execute prepared"), 0);
    assert!(stmt.query(&[]).expect("Query prepared").is_empty());
}

#[test]
fn test_row_get_case_insensitivity_and_from_value_impls() {
    let conn = Connection::open_in_memory().expect("Open DB");
    conn.execute(
        "CREATE TABLE users (id INTEGER PRIMARY KEY, is_active INTEGER, karma INTEGER, name TEXT)",
    )
    .expect("Create table");

    conn.execute(
        "INSERT INTO users (id, is_active, karma, name) VALUES (101, 1, 9999, 'Administrator')",
    )
    .expect("Insert");

    let rows = conn
        .query("SELECT users.id, users.is_active, users.karma, users.name FROM users")
        .expect("Query");
    let row = &rows[0];

    // 1. FromValue for bool (from integer 1)
    let is_active: bool = row.get("is_active").expect("get bool");
    assert!(is_active);

    // 2. FromValue for u64
    let id_u64: u64 = row.get("id").expect("get u64");
    assert_eq!(id_u64, 101);

    // 3. FromValue for usize
    let karma_usize: usize = row.get("karma").expect("get usize");
    assert_eq!(karma_usize, 9999);

    // 4. Case-insensitive unqualified lookup on qualified column name ("users.id" matched by "ID")
    let id_upper: u64 = row.get("ID").expect("get uppercase ID");
    assert_eq!(id_upper, 101);

    // 5. Case-insensitive qualified lookup ("USERS.NAME" matched by "users.name")
    let name: String = row.get("USERS.NAME").expect("get qualified name");
    assert_eq!(name, "Administrator");

    // 6. Direct col_name_matches testing via manual Row
    let manual_row = Row::new(
        vec!["orders.order_id".to_string(), "total_amount".to_string()],
        vec![Value::Integer(500), Value::Real(99.95)],
    );
    assert_eq!(manual_row.get::<u64>("ORDER_ID").unwrap(), 500);
    assert_eq!(manual_row.get::<u64>("orders.order_id").unwrap(), 500);
    assert_eq!(manual_row.get::<f64>("TOTAL_AMOUNT").unwrap(), 99.95);
    assert_eq!(manual_row.get::<f64>("orders.total_amount").unwrap(), 99.95);
}

#[test]
fn test_wal_checkpoint_sequential_order() {
    let tmp_dir = tempfile::tempdir().expect("Create temp dir");
    let db_path = tmp_dir.path().join("wal_seq.tapir");

    {
        let conn = Connection::open(&db_path).expect("Open disk DB");
        conn.execute("CREATE TABLE records (id INTEGER PRIMARY KEY, data TEXT)")
            .expect("Create table");

        // Insert multiple rows creating multiple B-Tree nodes and pages in WAL
        for i in 1..=40 {
            conn.execute(&format!(
                "INSERT INTO records (id, data) VALUES ({i}, 'Sample test data payload string {i}')"
            ))
            .expect("Insert record");
        }

        // Force WAL checkpoint
        conn.checkpoint().expect("Checkpoint WAL");
    }

    // Reopen and ensure all 40 records are intact and recoverable from base file
    {
        let conn = Connection::open(&db_path).expect("Reopen disk DB");
        let count_rows = conn
            .query("SELECT COUNT(1) FROM records")
            .expect("Count records");
        assert_eq!(count_rows[0].get_idx::<i64>(0).unwrap(), 40);

        let row = conn
            .query("SELECT data FROM records WHERE id = 25")
            .expect("Select id 25");
        assert_eq!(
            row[0].get::<String>("data").unwrap(),
            "Sample test data payload string 25"
        );
    }
}
