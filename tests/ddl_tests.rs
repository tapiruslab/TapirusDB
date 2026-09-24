//! Data Definition Language (DDL) Tests for TapirusDB
//!
//! Validates:
//! - CREATE TABLE / DROP TABLE (including IF EXISTS idempotency)
//! - ALTER TABLE ADD COLUMN with O(1) lazy null-padding
//! - ALTER TABLE validation and duplicate column errors
//! - Persistent CREATE VIEW, DROP VIEW, and view query execution

use tapirus::{Connection, Result, Value};
use tempfile::NamedTempFile;

#[test]
fn test_drop_table_and_if_exists() -> Result<()> {
    let db = Connection::open_in_memory()?;

    db.execute("CREATE TABLE temp_data (id INTEGER PRIMARY KEY, note TEXT);")?;
    assert!(db.table("temp_data").is_some());
    assert_eq!(db.tables().len(), 1);

    // Drop existing table
    let dropped = db.execute("DROP TABLE temp_data;")?;
    assert_eq!(dropped, 1);
    assert!(db.table("temp_data").is_none());
    assert_eq!(db.tables().len(), 0);

    // DROP TABLE IF EXISTS on non-existent table returns 0 without error
    let drop_missing_if_exists = db.execute("DROP TABLE IF EXISTS temp_data;")?;
    assert_eq!(drop_missing_if_exists, 0);

    // DROP TABLE without IF EXISTS on missing table returns error
    let drop_missing_err = db.execute("DROP TABLE temp_data;");
    assert!(drop_missing_err.is_err());

    Ok(())
}

#[test]
fn test_alter_table_add_column_with_lazy_null_padding() {
    let tmp = NamedTempFile::new().unwrap();
    let db_path = tmp.path().to_str().unwrap();

    // Step 1: Create initial table with 3 columns and insert rows
    {
        let conn = Connection::open(db_path).expect("Open database");
        conn.execute("CREATE TABLE products (id INTEGER PRIMARY KEY, name TEXT, price REAL);")
            .expect("Create table");

        conn.execute("INSERT INTO products (id, name, price) VALUES (1, 'Laptop', 1200.0);")
            .expect("Insert 1");
        conn.execute("INSERT INTO products (id, name, price) VALUES (2, 'Mouse', 25.0);")
            .expect("Insert 2");
        conn.execute("INSERT INTO products (id, name, price) VALUES (3, 'Keyboard', 75.0);")
            .expect("Insert 3");

        let rows = conn.query("SELECT * FROM products;").expect("Select initial");
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].columns().len(), 3);
    }

    // Step 2: Reopen and execute ALTER TABLE ADD COLUMN
    {
        let conn = Connection::open(db_path).expect("Reopen database");

        // Add a new column 'category' of type TEXT
        let altered = conn
            .execute("ALTER TABLE products ADD COLUMN category TEXT;")
            .expect("Alter table add column");
        assert_eq!(altered, 1);

        // Verify that existing rows have the new column with NULL value (lazy null padding)
        let rows = conn
            .query("SELECT id, name, price, category FROM products;")
            .expect("Select after alter");
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].columns().len(), 4);
        assert_eq!(rows[0].get_value("category"), Some(&Value::Null));
        assert_eq!(rows[1].get_value("category"), Some(&Value::Null));
        assert_eq!(rows[2].get_value("category"), Some(&Value::Null));

        // Insert a new row that provides all 4 column values
        conn.execute("INSERT INTO products (id, name, price, category) VALUES (4, 'Monitor', 350.0, 'Hardware');")
            .expect("Insert row with new column");

        let row4 = conn
            .query("SELECT * FROM products WHERE id = 4;")
            .expect("Select row 4");
        assert_eq!(row4.len(), 1);
        assert_eq!(row4[0].get_value("category"), Some(&Value::Text("Hardware".into())));

        // Update an existing row to populate the new column
        conn.execute("UPDATE products SET category = 'Peripherals' WHERE id = 2;")
            .expect("Update row 2");

        let row2 = conn
            .query("SELECT category FROM products WHERE id = 2;")
            .expect("Select updated row 2");
        assert_eq!(row2.len(), 1);
        assert_eq!(row2[0].get_value("category"), Some(&Value::Text("Peripherals".into())));

        // Add yet another column 'stock' of type INTEGER
        conn.execute("ALTER TABLE products ADD stock INTEGER;")
            .expect("Alter table add stock without COLUMN keyword");

        let rows_all = conn.query("SELECT * FROM products;").expect("Select with 5 columns");
        assert_eq!(rows_all.len(), 4);
        assert_eq!(rows_all[0].columns().len(), 5);
        assert_eq!(rows_all[0].get_value("stock"), Some(&Value::Null));
    }

    // Step 3: Reopen again and verify schema persistence across process restart
    {
        let conn = Connection::open(db_path).expect("Reopen database after schema evolution");
        let table = conn.table("products").expect("Get table def");
        assert_eq!(table.columns.len(), 5);
        assert_eq!(table.columns[3].name, "category");
        assert_eq!(table.columns[4].name, "stock");

        let rows = conn.query("SELECT id, name, category FROM products ORDER BY id ASC;")
            .expect("Query ordered");
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[1].get_value("category"), Some(&Value::Text("Peripherals".into())));
        assert_eq!(rows[3].get_value("category"), Some(&Value::Text("Hardware".into())));
    }
}

#[test]
fn test_alter_table_validation_errors() {
    let tmp = NamedTempFile::new().unwrap();
    let conn = Connection::open(tmp.path()).expect("Open database");

    conn.execute("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT);")
        .expect("Create users table");

    // Duplicate column name error
    let err = conn.execute("ALTER TABLE users ADD COLUMN name TEXT;");
    assert!(err.is_err(), "Adding duplicate column should return error");

    // Table not found error
    let err_not_found = conn.execute("ALTER TABLE ghosts ADD COLUMN ectoplasm TEXT;");
    assert!(err_not_found.is_err(), "Altering non-existent table should return error");
}

#[test]
fn test_create_and_query_persistent_views() {
    let tmp = NamedTempFile::new().unwrap();
    let db_path = tmp.path().to_str().unwrap();

    // Step 1: Create base table and insert employees
    {
        let conn = Connection::open(db_path).expect("Open database");
        conn.execute(
            "CREATE TABLE employees (id INTEGER PRIMARY KEY, name TEXT, dept TEXT, salary REAL);"
        ).expect("Create employees table");

        conn.execute("INSERT INTO employees (id, name, dept, salary) VALUES (1, 'Alice', 'Engineering', 95000.0);").unwrap();
        conn.execute("INSERT INTO employees (id, name, dept, salary) VALUES (2, 'Bob', 'Sales', 65000.0);").unwrap();
        conn.execute("INSERT INTO employees (id, name, dept, salary) VALUES (3, 'Charlie', 'Engineering', 110000.0);").unwrap();
        conn.execute("INSERT INTO employees (id, name, dept, salary) VALUES (4, 'Diana', 'Marketing', 70000.0);").unwrap();
        conn.execute("INSERT INTO employees (id, name, dept, salary) VALUES (5, 'Eve', 'Engineering', 85000.0);").unwrap();

        // Step 2: Create a View for Engineers
        conn.execute(
            "CREATE VIEW high_engineers AS SELECT id, name, salary FROM employees WHERE dept = 'Engineering';"
        ).expect("Create VIEW");

        let views = conn.views();
        assert_eq!(views, vec!["high_engineers"]);

        // Step 3: Query the View directly
        let eng_rows = conn.query("SELECT * FROM high_engineers;").expect("Query VIEW");
        assert_eq!(eng_rows.len(), 3);
        for row in &eng_rows {
            assert!(row.get_value("name").is_some());
            assert!(row.get_value("salary").is_some());
        }

        // Step 4: Query the View with outer WHERE and ORDER BY
        let filtered = conn.query(
            "SELECT name, salary FROM high_engineers WHERE salary > 90000.0 ORDER BY salary DESC;"
        ).expect("Query VIEW with filter");
        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0].get_value("name"), Some(&Value::Text("Charlie".into())));
        assert_eq!(filtered[1].get_value("name"), Some(&Value::Text("Alice".into())));

        // Step 5: CREATE VIEW IF NOT EXISTS idempotence
        conn.execute(
            "CREATE VIEW IF NOT EXISTS high_engineers AS SELECT id FROM employees;"
        ).expect("Create view if not exists");
    }

    // Step 6: Reopen database and verify View persistence across process restart
    {
        let conn = Connection::open(db_path).expect("Reopen database");
        assert_eq!(conn.views(), vec!["high_engineers"]);

        let rows = conn.query("SELECT * FROM high_engineers;").expect("Query persistent view");
        assert_eq!(rows.len(), 3);

        // Step 7: Drop View
        conn.execute("DROP VIEW high_engineers;").expect("Drop view");
        assert!(conn.views().is_empty());

        let err = conn.query("SELECT * FROM high_engineers;");
        assert!(err.is_err(), "Querying dropped view should fail");

        // DROP VIEW IF EXISTS
        conn.execute("DROP VIEW IF EXISTS high_engineers;").expect("Drop view if exists");
    }
}
