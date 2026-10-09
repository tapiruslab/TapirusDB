use tapirus::{Connection, Result, Value};

#[test]
fn test_dynamic_scalar_udf_registry_builtins() -> Result<()> {
    let conn = Connection::open_in_memory()?;

    conn.execute("CREATE TABLE devs (id INT PRIMARY KEY, name TEXT, delta INT, score REAL, phone TEXT);")?;

    conn.execute("INSERT INTO devs (id, name, delta, score, phone) VALUES (1, 'alice', -15, 9.876, NULL);")?;
    conn.execute("INSERT INTO devs (id, name, delta, score, phone) VALUES (2, 'bob', 42, 3.1415, '555-0199');")?;

    // Built-in string functions
    let rows = conn.query("SELECT UPPER(name) AS u_name, LOWER('MIXED') AS l_str, LENGTH(name) AS n_len FROM devs WHERE id = 1;")?;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get_value("u_name"), Some(&Value::Text("ALICE".to_string())));
    assert_eq!(rows[0].get_value("l_str"), Some(&Value::Text("mixed".to_string())));
    assert_eq!(rows[0].get_value("n_len"), Some(&Value::Integer(5)));

    // Built-in math functions: ABS, ROUND
    let rows = conn.query("SELECT ABS(delta) AS abs_d, ROUND(score, 2) AS r_sc FROM devs WHERE id = 1;")?;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get_value("abs_d"), Some(&Value::Integer(15)));
    assert_eq!(rows[0].get_value("r_sc"), Some(&Value::Real(9.88)));

    // Built-in COALESCE and CONCAT
    let rows = conn.query("SELECT COALESCE(phone, 'N/A') AS contact, CONCAT(name, ' ', 'rocks') AS phrase FROM devs WHERE id = 1;")?;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get_value("contact"), Some(&Value::Text("N/A".to_string())));
    assert_eq!(rows[0].get_value("phrase"), Some(&Value::Text("alice rocks".to_string())));

    // Built-in UDF inside WHERE clause
    let rows = conn.query("SELECT name FROM devs WHERE LENGTH(name) > 3;")?;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get_value("name"), Some(&Value::Text("alice".to_string())));

    let rows = conn.query("SELECT name FROM devs WHERE ABS(delta) > 20;")?;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get_value("name"), Some(&Value::Text("bob".to_string())));

    Ok(())
}

#[test]
fn test_dynamic_scalar_udf_custom_closure() -> Result<()> {
    let conn = Connection::open_in_memory()?;

    // Register a custom scalar UDF
    conn.register_scalar_function("DOUBLE_VAL", |args| {
        if let Some(first) = args.first() {
            match first {
                Value::Integer(i) => Ok(Value::Integer(i * 2)),
                Value::Real(r) => Ok(Value::Real(r * 2.0)),
                _ => Ok(Value::Null),
            }
        } else {
            Ok(Value::Null)
        }
    });

    conn.execute("CREATE TABLE nums (id INT PRIMARY KEY, val INT);")?;
    conn.execute("INSERT INTO nums (id, val) VALUES (1, 21);")?;
    conn.execute("INSERT INTO nums (id, val) VALUES (2, 50);")?;

    let rows = conn.query("SELECT id, DOUBLE_VAL(val) AS doubled FROM nums ORDER BY id ASC;")?;
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].get_value("doubled"), Some(&Value::Integer(42)));
    assert_eq!(rows[1].get_value("doubled"), Some(&Value::Integer(100)));

    Ok(())
}

#[test]
fn test_advanced_sql_exists_subqueries() -> Result<()> {
    let conn = Connection::open_in_memory()?;

    conn.execute("CREATE TABLE depts (id INT PRIMARY KEY, name TEXT);")?;
    conn.execute("CREATE TABLE emps (id INT PRIMARY KEY, dept_id INT, name TEXT);")?;

    conn.execute("INSERT INTO depts (id, name) VALUES (1, 'Engineering');")?;
    conn.execute("INSERT INTO depts (id, name) VALUES (2, 'Marketing');")?;

    conn.execute("INSERT INTO emps (id, dept_id, name) VALUES (10, 1, 'Grace');")?;

    // EXISTS when subquery returns rows -> true
    let rows = conn.query("SELECT name FROM depts WHERE EXISTS (SELECT * FROM emps WHERE dept_id = 1);")?;
    assert_eq!(rows.len(), 2);

    // NOT EXISTS when subquery returns rows -> false
    let rows = conn.query("SELECT name FROM depts WHERE NOT EXISTS (SELECT * FROM emps WHERE dept_id = 1);")?;
    assert_eq!(rows.len(), 0);

    // NOT EXISTS when subquery returns no rows -> true
    let rows = conn.query("SELECT name FROM depts WHERE NOT EXISTS (SELECT * FROM emps WHERE dept_id = 999);")?;
    assert_eq!(rows.len(), 2);

    Ok(())
}

#[test]
fn test_advanced_sql_derived_tables() -> Result<()> {
    let conn = Connection::open_in_memory()?;

    conn.execute("CREATE TABLE products (id INT PRIMARY KEY, name TEXT, price INT);")?;
    conn.execute("INSERT INTO products (id, name, price) VALUES (1, 'Keyboard', 150);")?;
    conn.execute("INSERT INTO products (id, name, price) VALUES (2, 'Mouse', 40);")?;
    conn.execute("INSERT INTO products (id, name, price) VALUES (3, 'Monitor', 300);")?;

    // Derived table in FROM clause
    let rows = conn.query(
        "SELECT d.name, d.price FROM (SELECT * FROM products WHERE price >= 100) AS d WHERE d.price < 250;",
    )?;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get_value("name"), Some(&Value::Text("Keyboard".to_string())));
    assert_eq!(rows[0].get_value("price"), Some(&Value::Integer(150)));

    Ok(())
}

#[test]
fn test_alter_table_mutations() -> Result<()> {
    let conn = Connection::open_in_memory()?;

    conn.execute("CREATE TABLE staff (id INT PRIMARY KEY, name TEXT, salary INT, notes TEXT);")?;
    conn.execute("INSERT INTO staff (id, name, salary, notes) VALUES (1, 'Alan', 8000, 'Senior');")?;
    conn.execute("INSERT INTO staff (id, name, salary, notes) VALUES (2, 'Ada', 9500, 'Principal');")?;

    // 1. RENAME TABLE
    conn.execute("ALTER TABLE staff RENAME TO engineers;")?;
    let rows = conn.query("SELECT name, salary FROM engineers ORDER BY id ASC;")?;
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].get_value("name"), Some(&Value::Text("Alan".to_string())));
    assert_eq!(rows[1].get_value("name"), Some(&Value::Text("Ada".to_string())));

    // Verify old table name no longer exists
    assert!(conn.query("SELECT * FROM staff;").is_err());

    // 2. RENAME COLUMN
    conn.execute("ALTER TABLE engineers RENAME COLUMN notes TO remarks;")?;
    let rows = conn.query("SELECT id, remarks FROM engineers WHERE id = 2;")?;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get_value("remarks"), Some(&Value::Text("Principal".to_string())));

    // 3. DROP COLUMN
    conn.execute("ALTER TABLE engineers DROP COLUMN salary;")?;
    let rows = conn.query("SELECT * FROM engineers WHERE id = 1;")?;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get_value("name"), Some(&Value::Text("Alan".to_string())));
    assert_eq!(rows[0].get_value("remarks"), Some(&Value::Text("Senior".to_string())));
    // Dropped column should no longer be present
    assert_eq!(rows[0].get_value("salary"), None);

    // Verify Primary Key protection on DROP COLUMN
    let drop_pk_err = conn.execute("ALTER TABLE engineers DROP COLUMN id;");
    assert!(drop_pk_err.is_err());

    Ok(())
}

#[test]
fn test_transaction_savepoints() -> Result<()> {
    let conn = Connection::open_in_memory()?;

    conn.execute("CREATE TABLE audit_log (id INT PRIMARY KEY, action TEXT);")?;

    conn.execute("BEGIN TRANSACTION;")?;
    conn.execute("INSERT INTO audit_log (id, action) VALUES (1, 'BOOT');")?;

    conn.execute("SAVEPOINT sp_first;")?;
    conn.execute("INSERT INTO audit_log (id, action) VALUES (2, 'STEP_A');")?;

    conn.execute("SAVEPOINT sp_second;")?;
    conn.execute("INSERT INTO audit_log (id, action) VALUES (3, 'STEP_B');")?;

    // Rollback to sp_second (undoes STEP_B)
    conn.execute("ROLLBACK TO SAVEPOINT sp_second;")?;

    // Release sp_first
    conn.execute("RELEASE SAVEPOINT sp_first;")?;

    conn.execute("COMMIT;")?;

    let rows = conn.query("SELECT id, action FROM audit_log ORDER BY id ASC;")?;
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].get_value("action"), Some(&Value::Text("BOOT".to_string())));
    assert_eq!(rows[1].get_value("action"), Some(&Value::Text("STEP_A".to_string())));

    Ok(())
}

#[test]
fn test_graph_relational_projection() -> Result<()> {
    let conn = Connection::open_in_memory()?;

    conn.execute("CREATE TABLE people (id INT PRIMARY KEY, name TEXT, role TEXT);")?;
    conn.execute("INSERT INTO people (id, name, role) VALUES (101, 'Linus', 'Maintainer');")?;
    conn.execute("INSERT INTO people (id, name, role) VALUES (102, 'Ken', 'Architect');")?;

    conn.execute("CREATE TABLE follows (from_id INT, to_id INT, rel TEXT, weight REAL);")?;
    conn.execute("INSERT INTO follows (from_id, to_id, rel, weight) VALUES (101, 102, 'COLLABORATES', 2.5);")?;

    // Project people as graph nodes
    let node_count = conn.project_table_as_nodes("people", "id", Some("name"), None)?;
    assert_eq!(node_count, 2);

    // Project follows as graph edges
    let edge_count = conn.project_table_as_edges("follows", "from_id", "to_id", Some("rel"), Some("weight"), None)?;
    assert_eq!(edge_count, 1);

    // Query projected graph using Graph Engine
    let path = conn.graph_find_path(101, 102, 3);
    assert!(path.is_some());
    let edges = path.unwrap();
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].label, "COLLABORATES");
    assert_eq!(edges[0].weight, 2.5);

    Ok(())
}

#[test]
fn test_continuous_streaming_wal_replication() -> Result<()> {
    // 1. Primary node writes data
    let primary = Connection::open_in_memory()?;
    primary.execute("CREATE TABLE cluster_state (id INT PRIMARY KEY, node_name TEXT, term INT);")?;
    primary.execute("INSERT INTO cluster_state (id, node_name, term) VALUES (1, 'node-alpha', 10);")?;
    primary.execute("INSERT INTO cluster_state (id, node_name, term) VALUES (2, 'node-beta', 10);")?;

    // 2. Export replication chunk with hardware CRC32
    let chunk = primary.export_replication_chunk()?;
    assert!(chunk.frame_count > 0);
    assert!(chunk.verify().is_ok());

    // Serialize and deserialize chunk to test network transmission
    let wire_bytes = chunk.to_bytes()?;
    let restored_chunk = tapirus::WalReplicationChunk::from_bytes(&wire_bytes)?;
    assert_eq!(restored_chunk.sequence_number, chunk.sequence_number);

    // 3. Standby replica applies chunk
    let standby = Connection::open_in_memory()?;
    let applied = standby.apply_replication_chunk(&restored_chunk)?;
    assert!(applied > 0);

    // 4. Verify standby replica can query the exact same data immediately
    let rows = standby.query("SELECT id, node_name, term FROM cluster_state ORDER BY id ASC;")?;
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].get_value("node_name"), Some(&Value::Text("node-alpha".to_string())));
    assert_eq!(rows[0].get_value("term"), Some(&Value::Integer(10)));
    assert_eq!(rows[1].get_value("node_name"), Some(&Value::Text("node-beta".to_string())));

    Ok(())
}
