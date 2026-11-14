use tapirus::{Connection, Error, Value};

fn mem_conn() -> Connection {
    Connection::open_in_memory().expect("open in-memory db")
}

#[test]
fn test_default_check_and_unique_constraints() {
    let conn = mem_conn();
    conn.execute(
        "CREATE TABLE products (
            id INTEGER PRIMARY KEY,
            sku TEXT UNIQUE NOT NULL,
            qty INTEGER DEFAULT 10 CHECK (qty >= 0),
            discount REAL DEFAULT -0.5,
            created_ts TEXT DEFAULT CURRENT_TIMESTAMP,
            org_id INTEGER,
            code TEXT,
            UNIQUE (org_id, code),
            CHECK (org_id > 0)
        );",
    )
    .expect("create table");

    // Defaults applied on omitted columns, NULL passes CHECK per 3-valued SQL logic
    conn.execute("INSERT INTO products (id, sku, org_id, code) VALUES (1, 'SKU-1', 7, 'A');")
        .expect("insert with defaults");

    let rows = conn
        .query("SELECT qty, discount, created_ts FROM products WHERE id = 1;")
        .expect("select row");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].values()[0], Value::Integer(10));
    assert_eq!(rows[0].values()[1], Value::Real(-0.5));
    assert!(matches!(rows[0].values()[2], Value::Text(ref s) if !s.is_empty()));

    // Column CHECK violation
    let err = conn
        .execute(
            "INSERT INTO products (id, sku, qty, org_id, code) VALUES (2, 'SKU-2', -1, 7, 'B');",
        )
        .unwrap_err();
    assert!(matches!(err, Error::ConstraintViolation(_)));

    // Table CHECK violation on UPDATE
    let err = conn
        .execute("UPDATE products SET org_id = 0 WHERE id = 1;")
        .unwrap_err();
    assert!(matches!(err, Error::ConstraintViolation(_)));

    // Inline UNIQUE violation
    let err = conn
        .execute("INSERT INTO products (id, sku, org_id, code) VALUES (2, 'SKU-1', 7, 'B');")
        .unwrap_err();
    assert!(matches!(err, Error::ConstraintViolation(_)));

    // Composite UNIQUE violation
    let err = conn
        .execute("INSERT INTO products (id, sku, org_id, code) VALUES (2, 'SKU-2', 7, 'A');")
        .unwrap_err();
    assert!(matches!(err, Error::ConstraintViolation(_)));

    // Multiple NULLs in UNIQUE / composite UNIQUE allowed per SQL standard
    conn.execute("INSERT INTO products (id, sku, org_id, code) VALUES (2, 'SKU-2', NULL, 'A');")
        .expect("null in composite unique allowed");
    conn.execute("INSERT INTO products (id, sku, org_id, code) VALUES (3, 'SKU-3', NULL, 'A');")
        .expect("second null in composite unique allowed");

    // ALTER TABLE ADD COLUMN with DEFAULT backfills existing rows
    conn.execute("ALTER TABLE products ADD COLUMN status TEXT DEFAULT 'active';")
        .expect("alter add default column");
    let rows = conn
        .query("SELECT status FROM products WHERE id = 1;")
        .expect("query backfilled col");
    assert_eq!(rows[0].values()[0], Value::Text("active".to_string()));
}

#[test]
fn test_foreign_keys_cascade_set_null_and_restrict() {
    let conn = mem_conn();
    conn.execute("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL);")
        .unwrap();
    conn.execute(
        "CREATE TABLE orders (
            id INTEGER PRIMARY KEY,
            user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE ON UPDATE CASCADE,
            total REAL
        );",
    )
    .unwrap();
    conn.execute(
        "CREATE TABLE notes (
            id INTEGER PRIMARY KEY,
            user_id INTEGER REFERENCES users(id) ON DELETE SET NULL ON UPDATE CASCADE,
            body TEXT
        );",
    )
    .unwrap();
    conn.execute(
        "CREATE TABLE audits (
            id INTEGER PRIMARY KEY,
            user_id INTEGER,
            FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE RESTRICT
        );",
    )
    .unwrap();

    conn.execute("INSERT INTO users VALUES (1, 'Alice'), (2, 'Bob');")
        .unwrap();

    // Reject missing parent
    let err = conn
        .execute("INSERT INTO orders VALUES (10, 999, 50.0);")
        .unwrap_err();
    assert!(matches!(err, Error::ConstraintViolation(_)));

    conn.execute("INSERT INTO orders VALUES (10, 1, 50.0), (11, 2, 75.0);")
        .unwrap();
    conn.execute("INSERT INTO notes VALUES (100, 1, 'VIP');")
        .unwrap();
    conn.execute("INSERT INTO audits VALUES (200, 2);").unwrap();

    // ON UPDATE CASCADE updates child row FK column
    conn.execute("UPDATE users SET id = 101 WHERE id = 1;")
        .unwrap();
    let order_rows = conn
        .query("SELECT user_id FROM orders WHERE id = 10;")
        .unwrap();
    assert_eq!(order_rows[0].values()[0], Value::Integer(101));

    // RESTRICT blocks delete of user 2 while referenced in audits
    let err = conn.execute("DELETE FROM users WHERE id = 2;").unwrap_err();
    assert!(matches!(err, Error::ConstraintViolation(_)));

    // NULL FK in notes allows delete later; CASCADE on user 101 deletes order 10 and sets notes 100 to NULL
    conn.execute("DELETE FROM users WHERE id = 101;").unwrap();
    assert!(
        conn.query("SELECT id FROM orders WHERE id = 10;")
            .unwrap()
            .is_empty()
    );
    let note_rows = conn
        .query("SELECT user_id FROM notes WHERE id = 100;")
        .unwrap();
    assert_eq!(note_rows[0].values()[0], Value::Null);

    // PRAGMA foreign_keys = OFF disables FK check
    conn.execute("PRAGMA foreign_keys = OFF;").unwrap();
    conn.execute("INSERT INTO orders VALUES (99, 8888, 1.0);")
        .expect("FK bypassed when PRAGMA foreign_keys = OFF");
    let fk_state = conn.query("PRAGMA foreign_keys;").unwrap();
    assert_eq!(fk_state[0].values()[0], Value::Integer(0));
}

#[test]
fn test_composite_unique_index_and_triggers_and_fts5_and_pragma_table_info() {
    let conn = mem_conn();
    conn.execute("CREATE TABLE accounts (id INTEGER PRIMARY KEY, org INTEGER, email TEXT, balance INTEGER DEFAULT 0);")
        .unwrap();
    conn.execute("CREATE UNIQUE INDEX idx_org_email ON accounts (org, email);")
        .unwrap();

    conn.execute("INSERT INTO accounts VALUES (1, 10, 'a@x.com', 100), (2, 20, 'a@x.com', 200);")
        .unwrap();

    // Duplicate composite tuple fails
    let err = conn
        .execute("INSERT INTO accounts VALUES (3, 10, 'a@x.com', 300);")
        .unwrap_err();
    assert!(matches!(err, Error::ConstraintViolation(_)));

    // Composite index query matches accurately
    let res = conn
        .query("SELECT id, balance FROM accounts WHERE org = 10 AND email = 'a@x.com';")
        .unwrap();
    assert_eq!(res.len(), 1);
    assert_eq!(res[0].values()[0], Value::Integer(1));

    // Triggers: AFTER INSERT audit log + BEFORE UPDATE RAISE(ABORT) + DROP TRIGGER
    conn.execute("CREATE TABLE audit_log (id INTEGER PRIMARY KEY, msg TEXT);")
        .unwrap();
    conn.execute(
        "CREATE TRIGGER trg_acct_ins AFTER INSERT ON accounts FOR EACH ROW BEGIN INSERT INTO audit_log (msg) VALUES (NEW.email); END;",
    )
    .unwrap();
    conn.execute(
        "CREATE TRIGGER trg_no_neg BEFORE UPDATE ON accounts FOR EACH ROW WHEN NEW.balance < 0 BEGIN RAISE(ABORT, 'negative balance'); END;",
    )
    .unwrap();

    conn.execute("INSERT INTO accounts VALUES (4, 30, 'c@x.com', 50);")
        .unwrap();
    let logs = conn.query("SELECT msg FROM audit_log;").unwrap();
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0].values()[0], Value::Text("c@x.com".to_string()));

    let err = conn
        .execute("UPDATE accounts SET balance = -5 WHERE id = 4;")
        .unwrap_err();
    assert!(matches!(err, Error::ConstraintViolation(ref m) if m.contains("negative balance")));

    conn.execute("DROP TRIGGER trg_no_neg;").unwrap();
    conn.execute("UPDATE accounts SET balance = -5 WHERE id = 4;")
        .expect("update succeeds after DROP TRIGGER");

    // FTS5 virtual table + MATCH + bm25(docs) + ORDER BY rank
    conn.execute("CREATE VIRTUAL TABLE docs USING fts5(title, body);")
        .unwrap();
    conn.execute(
        "INSERT INTO docs VALUES ('Rust Intro', 'Systems programming in Rust is fast and safe');",
    )
    .unwrap();
    conn.execute("INSERT INTO docs VALUES ('Cooking', 'Baking sourdough bread takes patience');")
        .unwrap();
    conn.execute(
        "INSERT INTO docs VALUES ('Rust DB', 'TapirusDB is written in Rust with MVCC and FTS5');",
    )
    .unwrap();

    let hits = conn
        .query("SELECT title, bm25(docs) FROM docs WHERE docs MATCH 'Rust' ORDER BY rank;")
        .unwrap();
    assert_eq!(hits.len(), 2);
    assert!(matches!(hits[0].values()[1], Value::Real(s) if s < 0.0));

    // Delete from FTS5 updates inverted index
    conn.execute("DELETE FROM docs WHERE title = 'Rust Intro';")
        .unwrap();
    let hits_after = conn
        .query("SELECT title FROM docs WHERE docs MATCH 'Rust';")
        .unwrap();
    assert_eq!(hits_after.len(), 1);
    assert_eq!(
        hits_after[0].values()[0],
        Value::Text("Rust DB".to_string())
    );

    // PRAGMA table_info(accounts)
    let info = conn.query("PRAGMA table_info(accounts);").unwrap();
    assert_eq!(info.len(), 4);
    assert_eq!(info[0].values()[1], Value::Text("id".to_string()));
    assert_eq!(info[0].values()[5], Value::Integer(1));
}
