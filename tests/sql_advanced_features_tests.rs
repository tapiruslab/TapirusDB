use tapirus::{Connection, Value};

#[test]
fn test_with_recursive_sequence_and_hierarchy() -> tapirus::Result<()> {
    let db = Connection::open_in_memory()?;

    // 1. Classic tableless recursive sequence generator
    let rows = db.query(
        "WITH RECURSIVE cnt(x) AS (
            SELECT 1
            UNION ALL
            SELECT x + 1 FROM cnt WHERE x < 5
        )
        SELECT x, x * 10 AS scaled FROM cnt ORDER BY x ASC;",
    )?;
    assert_eq!(rows.len(), 5);
    let seq: Vec<i64> = rows
        .iter()
        .map(|r| r.get::<i64>("x"))
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(seq, vec![1, 2, 3, 4, 5]);
    let scaled: Vec<i64> = rows
        .iter()
        .map(|r| r.get::<i64>("scaled"))
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(scaled, vec![10, 20, 30, 40, 50]);

    // 2. Hierarchical org-chart traversal joining real table + recursive CTE + string concat
    db.execute(
        "CREATE TABLE employees (id INTEGER PRIMARY KEY, name TEXT NOT NULL, manager_id INTEGER);",
    )?;
    db.execute("INSERT INTO employees VALUES (1, 'Alice', NULL);")?;
    db.execute("INSERT INTO employees VALUES (2, 'Bob', 1);")?;
    db.execute("INSERT INTO employees VALUES (3, 'Carol', 1);")?;
    db.execute("INSERT INTO employees VALUES (4, 'Dave', 2);")?;

    let tree = db.query(
        "WITH RECURSIVE org(id, name, depth, path) AS (
            SELECT id, name, 0, name FROM employees WHERE manager_id IS NULL
            UNION ALL
            SELECT employees.id, employees.name, org.depth + 1, org.path || ' -> ' || employees.name
            FROM employees
            JOIN org ON employees.manager_id = org.id
        )
        SELECT id, name, depth, path FROM org ORDER BY id ASC;",
    )?;
    assert_eq!(tree.len(), 4);
    assert_eq!(tree[0].get::<i64>("depth")?, 0);
    assert_eq!(tree[0].get::<String>("path")?, "Alice");
    assert_eq!(tree[3].get::<i64>("depth")?, 2);
    assert_eq!(tree[3].get::<String>("path")?, "Alice -> Bob -> Dave");

    // 3. Cycle-safe UNION (distinct fixpoint) stops even with cyclic edges
    db.execute("CREATE TABLE edges (src INTEGER NOT NULL, dst INTEGER NOT NULL);")?;
    db.execute("INSERT INTO edges VALUES (1, 2), (2, 3), (3, 1);")?;
    let reach = db.query(
        "WITH RECURSIVE reach(node) AS (
            SELECT 1
            UNION
            SELECT edges.dst FROM edges JOIN reach ON edges.src = reach.node
        )
        SELECT node FROM reach ORDER BY node ASC;",
    )?;
    let nodes: Vec<i64> = reach
        .iter()
        .map(|r| r.get::<i64>("node"))
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(nodes, vec![1, 2, 3]);

    // 4. Standalone UNION and UNION ALL
    let u_all = db.query("SELECT 1 AS v UNION ALL SELECT 1 AS v UNION ALL SELECT 2 AS v;")?;
    assert_eq!(u_all.len(), 3);
    let u_dist = db.query("SELECT 1 AS v UNION SELECT 1 AS v UNION SELECT 2 AS v;")?;
    assert_eq!(u_dist.len(), 2);

    Ok(())
}

#[test]
fn test_partial_indexes_uniqueness_planning_and_reopen() -> tapirus::Result<()> {
    let dir = tempfile::tempdir()?;
    let db_path = dir.path().join("partial_idx.tapir");

    {
        let db = Connection::open(&db_path)?;
        db.execute(
            "CREATE TABLE accounts (id INTEGER PRIMARY KEY, email TEXT NOT NULL, active INTEGER NOT NULL, deleted INTEGER NOT NULL DEFAULT 0);",
        )?;
        db.execute("INSERT INTO accounts VALUES (1, 'alice@example.com', 1, 0);")?;
        db.execute("INSERT INTO accounts VALUES (2, 'alice@example.com', 0, 1);")?;
        db.execute("INSERT INTO accounts VALUES (3, 'bob@example.com', 0, 1);")?;

        // Unique partial index only enforces uniqueness among non-deleted rows
        db.execute("CREATE UNIQUE INDEX idx_active_email ON accounts (email) WHERE deleted = 0;")?;

        // Adding another deleted row with the same email must succeed
        db.execute("INSERT INTO accounts VALUES (4, 'alice@example.com', 0, 1);")?;

        // Adding another active (deleted = 0) row with duplicate email must fail
        let dup_err = db.execute("INSERT INTO accounts VALUES (5, 'alice@example.com', 1, 0);");
        assert!(
            dup_err.is_err(),
            "duplicate key matching partial unique predicate must fail"
        );

        // Soft-delete row 1 via UPDATE -> now inserting new active alice@example.com succeeds
        db.execute("UPDATE accounts SET deleted = 1 WHERE id = 1;")?;
        db.execute("INSERT INTO accounts VALUES (6, 'alice@example.com', 1, 0);")?;

        // Planner uses PARTIAL INDEX when WHERE implies deleted = 0
        let plan_partial = db.query(
            "EXPLAIN QUERY PLAN SELECT id, email FROM accounts WHERE email = 'alice@example.com' AND deleted = 0;",
        )?;
        let detail = plan_partial[0].get::<String>("detail")?;
        assert!(
            detail.contains("PARTIAL INDEX idx_active_email"),
            "expected PARTIAL INDEX plan, got: {detail}"
        );

        let active_rows = db.query(
            "SELECT id, email FROM accounts WHERE email = 'alice@example.com' AND deleted = 0;",
        )?;
        assert_eq!(active_rows.len(), 1);
        assert_eq!(active_rows[0].get::<i64>("id")?, 6);

        // Query WITHOUT deleted = 0 must NOT miss deleted rows by wrongly using partial index
        let all_alice =
            db.query("SELECT id FROM accounts WHERE email = 'alice@example.com' ORDER BY id ASC;")?;
        assert_eq!(all_alice.len(), 4);

        let plan_full = db.query(
            "EXPLAIN QUERY PLAN SELECT id FROM accounts WHERE email = 'alice@example.com';",
        )?;
        let detail_full = plan_full[0].get::<String>("detail")?;
        assert!(
            !detail_full.contains("idx_active_email"),
            "partial index must not be selected when predicate is omitted: {detail_full}"
        );

        db.close()?;
    }

    // Reopen persisted database and verify partial index enforcement + lookup still work
    let db = Connection::open(&db_path)?;
    let still_dup = db.execute("INSERT INTO accounts VALUES (7, 'alice@example.com', 1, 0);");
    assert!(still_dup.is_err());
    let rows =
        db.query("SELECT id FROM accounts WHERE email = 'alice@example.com' AND deleted = 0;")?;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get::<i64>("id")?, 6);

    Ok(())
}

#[test]
fn test_fast_batch_sql_insert_preserves_constraints_inside_tx() -> tapirus::Result<()> {
    let db = Connection::open_in_memory()?;
    db.execute("CREATE TABLE items (id INTEGER PRIMARY KEY, body TEXT NOT NULL);")?;

    db.execute("BEGIN;")?;
    for i in 0..200 {
        db.execute_with_params(
            "INSERT INTO items VALUES (?, ?);",
            &[Value::Integer(i), Value::Text(format!("row-{i}"))],
        )?;
    }
    // Duplicate PK must fail cleanly without corrupting prior rows
    let dup = db.execute_with_params(
        "INSERT INTO items VALUES (?, ?);",
        &[Value::Integer(42), Value::Text("dup".into())],
    );
    assert!(dup.is_err());

    // NULL into NOT NULL column must fail cleanly without corrupting prior rows
    let null_err = db.execute_with_params(
        "INSERT INTO items VALUES (?, ?);",
        &[Value::Integer(500), Value::Null],
    );
    assert!(null_err.is_err());

    db.execute("COMMIT;")?;

    let count = db.query("SELECT COUNT(*) AS c FROM items;")?;
    assert_eq!(count[0].get::<i64>("c")?, 200);
    let check = db.query("SELECT body FROM items WHERE id = 42;")?;
    assert_eq!(check[0].get::<String>("body")?, "row-42");

    Ok(())
}
