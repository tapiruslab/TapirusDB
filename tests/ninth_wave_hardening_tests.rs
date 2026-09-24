//! Ninth-Wave Forensic Hardening Integration Tests for TapirusDB
//!
//! Validates absolute honesty and integrity across the database engine:
//! 1. `INSERT` with mismatched vector dimensions is rejected with `Error::DimensionMismatch`.
//! 2. `INSERT` with invalid types (e.g. non-numeric string into `INTEGER`) is rejected with `Error::ConstraintViolation`.
//! 3. `INSERT` with safe coercions (e.g. integer into `REAL` or numeric string into `INTEGER`) succeeds seamlessly.
//! 4. `UPDATE` with mismatched vector dimensions is rejected with `Error::DimensionMismatch`.
//! 5. `UPDATE` with invalid types is rejected with `Error::ConstraintViolation`.

use tapirus::{Connection, Error};

#[test]
fn test_insert_vector_dimension_mismatch_rejected() {
    let conn = Connection::open_in_memory().expect("Open in-memory DB");
    conn.execute("CREATE TABLE embeddings (id INTEGER PRIMARY KEY, vec VECTOR(3))")
        .expect("Create vector table");

    // 1. Correct dimension (3 elements) succeeds
    conn.execute("INSERT INTO embeddings (id, vec) VALUES (1, [1.0, 0.0, 0.0])")
        .expect("Insert 3-dim vector");

    // 2. Mismatched dimension (5 elements into VECTOR(3)) must be rejected with DimensionMismatch
    let res = conn.execute("INSERT INTO embeddings (id, vec) VALUES (2, [1.0, 2.0, 3.0, 4.0, 5.0])");
    match res {
        Err(Error::DimensionMismatch(expected, got)) => {
            assert_eq!(expected, 3);
            assert_eq!(got, 5);
        }
        Ok(_) => panic!("Inserting 5-dim vector into VECTOR(3) must NOT silently succeed!"),
        Err(other) => panic!("Expected Error::DimensionMismatch, got: {other:?}"),
    }

    // 3. Mismatched dimension (2 elements into VECTOR(3)) must also be rejected
    let res_short = conn.execute("INSERT INTO embeddings (id, vec) VALUES (3, [1.0, 2.0])");
    match res_short {
        Err(Error::DimensionMismatch(expected, got)) => {
            assert_eq!(expected, 3);
            assert_eq!(got, 2);
        }
        Ok(_) => panic!("Inserting 2-dim vector into VECTOR(3) must NOT silently succeed!"),
        Err(other) => panic!("Expected Error::DimensionMismatch, got: {other:?}"),
    }
}

#[test]
fn test_insert_type_mismatch_rejected() {
    let conn = Connection::open_in_memory().expect("Open in-memory DB");
    conn.execute("CREATE TABLE accounts (id INTEGER PRIMARY KEY, balance INTEGER, note TEXT)")
        .expect("Create accounts table");

    // 1. Valid insertion succeeds
    conn.execute("INSERT INTO accounts (id, balance, note) VALUES (1, 100, 'Opening balance')")
        .expect("Insert valid row");

    // 2. Non-numeric text into INTEGER column must be rejected with ConstraintViolation
    let res = conn.execute("INSERT INTO accounts (id, balance, note) VALUES (2, 'one-hundred', 'Bad row')");
    match res {
        Err(Error::ConstraintViolation(msg)) => {
            assert!(
                msg.contains("balance") && msg.contains("Cannot convert text"),
                "Expected constraint violation mentioning column and failure, got: {msg}"
            );
        }
        Ok(_) => panic!("Inserting non-numeric string into INTEGER column must NOT silently succeed!"),
        Err(other) => panic!("Expected Error::ConstraintViolation, got: {other:?}"),
    }
}

#[test]
fn test_insert_safe_coercion_succeeds() {
    let conn = Connection::open_in_memory().expect("Open in-memory DB");
    conn.execute("CREATE TABLE products (id INTEGER PRIMARY KEY, price REAL, code TEXT)")
        .expect("Create products table");

    // Integer 50 coerces cleanly to REAL 50.0; Integer 999 coerces to TEXT '999'
    conn.execute("INSERT INTO products (id, price, code) VALUES (1, 50, 999)")
        .expect("Insert with safe coercions");

    let rows = conn.query("SELECT id, price, code FROM products WHERE id = 1").expect("Query");
    assert_eq!(rows.len(), 1);

    let price: f64 = rows[0].get("price").expect("price as f64");
    assert_eq!(price, 50.0);

    let code: String = rows[0].get("code").expect("code as String");
    assert_eq!(code, "999");
}

#[test]
fn test_update_vector_dimension_mismatch_rejected() {
    let conn = Connection::open_in_memory().expect("Open in-memory DB");
    conn.execute("CREATE TABLE doc_vectors (id INTEGER PRIMARY KEY, embedding VECTOR(3))")
        .expect("Create table");

    conn.execute("INSERT INTO doc_vectors (id, embedding) VALUES (1, [0.1, 0.2, 0.3])")
        .expect("Insert");

    // Update with 4 dimensions on a VECTOR(3) column must be rejected
    let res = conn.execute("UPDATE doc_vectors SET embedding = [0.1, 0.2, 0.3, 0.4] WHERE id = 1");
    match res {
        Err(Error::DimensionMismatch(expected, got)) => {
            assert_eq!(expected, 3);
            assert_eq!(got, 4);
        }
        Ok(_) => panic!("Updating VECTOR(3) with 4-dim vector must NOT silently succeed!"),
        Err(other) => panic!("Expected Error::DimensionMismatch, got: {other:?}"),
    }
}

#[test]
fn test_update_type_mismatch_rejected() {
    let conn = Connection::open_in_memory().expect("Open in-memory DB");
    conn.execute("CREATE TABLE items (id INTEGER PRIMARY KEY, quantity INTEGER)")
        .expect("Create table");

    conn.execute("INSERT INTO items (id, quantity) VALUES (1, 10)")
        .expect("Insert");

    // Update with non-numeric text must be rejected
    let res = conn.execute("UPDATE items SET quantity = 'infinite' WHERE id = 1");
    match res {
        Err(Error::ConstraintViolation(msg)) => {
            assert!(
                msg.contains("quantity") && msg.contains("Cannot convert text"),
                "Expected constraint violation mentioning column, got: {msg}"
            );
        }
        Ok(_) => panic!("Updating INTEGER with non-numeric text must NOT silently succeed!"),
        Err(other) => panic!("Expected Error::ConstraintViolation, got: {other:?}"),
    }
}
