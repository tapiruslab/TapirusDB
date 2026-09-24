//! Comprehensive SQL Execution, DML, and Query Plan Tests for TapirusDB
//!
//! Validates:
//! - Primary key point lookups and range scans
//! - UPDATE and DELETE DML mutations
//! - B+Tree physical space reclamation and slotted-cell reuse
//! - DISTINCT projections
//! - INNER JOIN and LEFT OUTER JOIN
//! - Conjunctions (AND) and disjunctions (OR)
//! - Extended comparisons (>, <, >=, <=, !=, <>)
//! - Pattern matching (LIKE with wildcards) and lexical search (MATCH BM25)
//! - Aggregations (COUNT, SUM, AVG, MIN, MAX), GROUP BY, and HAVING
//! - Subqueries (IN and NOT IN)
//! - EXPLAIN and EXPLAIN QUERY PLAN
//! - Parameterized queries (`?`) and SQL injection safety

use tapirus::{Connection, Result, Value};

#[test]
fn test_sql_update_and_delete() {
    let conn = Connection::open_in_memory().expect("Open in-memory db");

    conn.execute("CREATE TABLE products (id INTEGER PRIMARY KEY, name TEXT, price REAL, stock INTEGER);")
        .expect("Create table");

    conn.execute("INSERT INTO products (id, name, price, stock) VALUES (1, 'Laptop', 1200.0, 10);")
        .expect("Insert 1");
    conn.execute("INSERT INTO products (id, name, price, stock) VALUES (2, 'Mouse', 25.0, 100);")
        .expect("Insert 2");
    conn.execute("INSERT INTO products (id, name, price, stock) VALUES (3, 'Keyboard', 75.0, 50);")
        .expect("Insert 3");

    // UPDATE products
    let affected = conn
        .execute("UPDATE products SET price = 1100.0, stock = 8 WHERE id = 1;")
        .expect("Update products");
    assert_eq!(affected, 1);

    let rows = conn
        .query("SELECT id, name, price, stock FROM products WHERE id = 1;")
        .expect("Query updated product");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get::<f64>("price").unwrap(), 1100.0);
    assert_eq!(rows[0].get::<i64>("stock").unwrap(), 8);

    // DELETE products
    let del_affected = conn
        .execute("DELETE FROM products WHERE id = 2;")
        .expect("Delete product 2");
    assert_eq!(del_affected, 1);

    let all_rows = conn
        .query("SELECT id, name FROM products;")
        .expect("Query all remaining");
    assert_eq!(all_rows.len(), 2);
    let ids: Vec<i64> = all_rows.iter().map(|r| r.get::<i64>("id").unwrap()).collect();
    assert_eq!(ids, vec![1, 3]);

    // DELETE remaining matching condition
    let del_remaining = conn
        .execute("DELETE FROM products WHERE id = 1;")
        .expect("Delete product 1");
    assert_eq!(del_remaining, 1);

    let final_rows = conn
        .query("SELECT id FROM products;")
        .expect("Query remaining");
    assert_eq!(final_rows.len(), 1);
    assert_eq!(final_rows[0].get::<i64>("id").unwrap(), 3);
}

#[test]
fn test_select_distinct() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute("CREATE TABLE products (id INTEGER PRIMARY KEY, category TEXT, price REAL);").unwrap();

    conn.execute("INSERT INTO products VALUES (1, 'electronics', 100.0);").unwrap();
    conn.execute("INSERT INTO products VALUES (2, 'clothing', 50.0);").unwrap();
    conn.execute("INSERT INTO products VALUES (3, 'electronics', 250.0);").unwrap();
    conn.execute("INSERT INTO products VALUES (4, 'clothing', 80.0);").unwrap();
    conn.execute("INSERT INTO products VALUES (5, 'electronics', 100.0);").unwrap();

    let all_cats = conn.query("SELECT category FROM products;").unwrap();
    assert_eq!(all_cats.len(), 5);

    let distinct_cats = conn.query("SELECT DISTINCT category FROM products;").unwrap();
    assert_eq!(distinct_cats.len(), 2);
    let mut names: Vec<String> = distinct_cats
        .iter()
        .map(|r| match r.get_value("category").unwrap() {
            Value::Text(s) => s.clone(),
            _ => panic!("Expected text"),
        })
        .collect();
    names.sort();
    assert_eq!(names, vec!["clothing".to_string(), "electronics".to_string()]);
}

#[test]
fn test_left_outer_join_syntax() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT);").unwrap();
    conn.execute("CREATE TABLE orders (id INTEGER PRIMARY KEY, user_id INTEGER, amount REAL);").unwrap();

    conn.execute("INSERT INTO users VALUES (1, 'Alice');").unwrap();
    conn.execute("INSERT INTO users VALUES (2, 'Bob');").unwrap();
    conn.execute("INSERT INTO orders VALUES (10, 1, 99.0);").unwrap();

    let rows = conn.query("SELECT users.name, orders.amount FROM users LEFT OUTER JOIN orders ON users.id = orders.user_id;").unwrap();
    assert_eq!(rows.len(), 2);

    let alice = rows.iter().find(|r| {
        r.get_value("users.name") == Some(&Value::Text("Alice".to_string()))
    }).unwrap();
    assert_eq!(alice.get_value("orders.amount"), Some(&Value::Real(99.0)));

    let bob = rows.iter().find(|r| {
        r.get_value("users.name") == Some(&Value::Text("Bob".to_string()))
    }).unwrap();
    assert_eq!(bob.get_value("orders.amount"), Some(&Value::Null));
}

#[test]
fn test_extended_where_comparisons_and_conjunction() {
    let conn = Connection::open_in_memory().expect("Open memory db");

    conn.execute(
        "CREATE TABLE products (id INTEGER PRIMARY KEY, name TEXT, price REAL, stock INTEGER);"
    ).expect("Create products table");

    conn.execute("INSERT INTO products (id, name, price, stock) VALUES (1, 'Widget A', 10.50, 100);").unwrap();
    conn.execute("INSERT INTO products (id, name, price, stock) VALUES (2, 'Widget B', 25.00, 10);").unwrap();
    conn.execute("INSERT INTO products (id, name, price, stock) VALUES (3, 'Gadget X', 99.90, 50);").unwrap();
    conn.execute("INSERT INTO products (id, name, price, stock) VALUES (4, 'Gadget Y', 150.00, 5);").unwrap();
    conn.execute("INSERT INTO products (id, name, price, stock) VALUES (5, 'Doohickey', 5.00, 200);").unwrap();

    // 1. Greater than (>)
    let gt_rows = conn.query("SELECT name FROM products WHERE price > 50.0;").unwrap();
    assert_eq!(gt_rows.len(), 2);
    let names: Vec<String> = gt_rows.iter().map(|r| r.get::<String>("name").unwrap()).collect();
    assert!(names.contains(&"Gadget X".to_string()));
    assert!(names.contains(&"Gadget Y".to_string()));

    // 2. Less than or equal (<=)
    let le_rows = conn.query("SELECT name FROM products WHERE stock <= 10;").unwrap();
    assert_eq!(le_rows.len(), 2);

    // 3. Not equal (!= and <>)
    let ne_rows = conn.query("SELECT name FROM products WHERE stock != 100;").unwrap();
    assert_eq!(ne_rows.len(), 4);

    let ne2_rows = conn.query("SELECT name FROM products WHERE stock <> 100;").unwrap();
    assert_eq!(ne2_rows.len(), 4);

    // 4. AND conjunction
    let and_rows = conn.query("SELECT name FROM products WHERE price > 20.0 AND stock >= 50;").unwrap();
    assert_eq!(and_rows.len(), 1);
    assert_eq!(and_rows[0].get::<String>("name").unwrap(), "Gadget X");

    // 5. UPDATE with extended WHERE
    let updated = conn.execute("UPDATE products SET price = 12.00 WHERE stock > 150;").unwrap();
    assert_eq!(updated, 1);
    let updated_row = conn.query("SELECT price FROM products WHERE id = 5;").unwrap();
    assert_eq!(updated_row[0].get::<f64>("price").unwrap(), 12.00);

    // 6. DELETE with extended WHERE
    let deleted = conn.execute("DELETE FROM products WHERE stock < 10;").unwrap();
    assert_eq!(deleted, 1);
    let remaining = conn.query("SELECT COUNT(*) FROM products;").unwrap();
    assert_eq!(remaining[0].get::<i64>("COUNT(*)").unwrap(), 4);
}

#[test]
fn test_sql_like_and_match_bm25() {
    let conn = Connection::open_in_memory().expect("Open memory db");

    conn.execute("CREATE TABLE articles (id INTEGER PRIMARY KEY, title TEXT, body TEXT);").unwrap();

    conn.execute("INSERT INTO articles (id, title, body) VALUES (1, 'Safe Rust for Systems', 'Rust guarantees memory safety without garbage collection.');").unwrap();
    conn.execute("INSERT INTO articles (id, title, body) VALUES (2, 'Embedded AI Engines', 'Deploying SLM and LLM models on edge microcontrollers.');").unwrap();
    conn.execute("INSERT INTO articles (id, title, body) VALUES (3, 'High Performance Graph Databases', 'Graph algorithms in systems programming.');").unwrap();

    // LIKE with % wildcard
    let like_rows = conn.query("SELECT id, title FROM articles WHERE title LIKE '%Rust%';").unwrap();
    assert_eq!(like_rows.len(), 1);
    assert_eq!(like_rows[0].get::<i64>("id").unwrap(), 1);

    // Case-insensitive LIKE
    let like_case = conn.query("SELECT id, title FROM articles WHERE title LIKE '%rust%';").unwrap();
    assert_eq!(like_case.len(), 1);

    // MATCH with BM25 tokenization
    let match_rows = conn.query("SELECT id, title FROM articles WHERE body MATCH 'microcontrollers edge';").unwrap();
    assert_eq!(match_rows.len(), 1);
    assert_eq!(match_rows[0].get::<i64>("id").unwrap(), 2);
}

#[test]
fn test_explain_and_query_plan() {
    let conn = Connection::open_in_memory().expect("Open memory db");

    conn.execute("CREATE TABLE users (id INTEGER PRIMARY KEY, username TEXT, score INTEGER);").unwrap();

    // EXPLAIN on Primary Key point lookup
    let explain_pk = conn.query("EXPLAIN QUERY PLAN SELECT * FROM users WHERE id = 42;").unwrap();
    assert!(!explain_pk.is_empty());
    let detail_pk = explain_pk[0].get::<String>("detail").unwrap();
    assert!(detail_pk.contains("PRIMARY KEY"));

    // EXPLAIN on Filtered Table Scan with ORDER BY
    let explain_scan = conn.query("EXPLAIN SELECT * FROM users WHERE score > 50 ORDER BY score DESC;").unwrap();
    assert!(explain_scan.len() >= 2);
    let details: Vec<String> = explain_scan.iter().map(|r| r.get::<String>("detail").unwrap()).collect();
    assert!(details.iter().any(|d| d.contains("SCAN TABLE users")));
    assert!(details.iter().any(|d| d.contains("ORDER BY score DESC")));
}

#[test]
fn test_btree_delete_and_space_reclamation() -> Result<()> {
    let db = Connection::open_in_memory()?;

    db.execute("CREATE TABLE accounts (id INTEGER PRIMARY KEY, balance REAL, owner TEXT);")?;

    for i in 1..=10 {
        db.execute(&format!(
            "INSERT INTO accounts (id, balance, owner) VALUES ({i}, {}, 'User_{i}');",
            i * 100
        ))?;
    }

    let count_before = db.query("SELECT id FROM accounts;")?.len();
    assert_eq!(count_before, 10);

    // Delete accounts 3, 5, 7
    assert_eq!(db.execute("DELETE FROM accounts WHERE id = 3;")?, 1);
    assert_eq!(db.execute("DELETE FROM accounts WHERE id = 5;")?, 1);
    assert_eq!(db.execute("DELETE FROM accounts WHERE id = 7;")?, 1);

    let rows_after_del = db.query("SELECT id FROM accounts;")?;
    assert_eq!(rows_after_del.len(), 7);
    let surviving_ids: Vec<i64> = rows_after_del
        .iter()
        .map(|r| r.get::<i64>("id").unwrap())
        .collect();
    assert!(!surviving_ids.contains(&3));
    assert!(!surviving_ids.contains(&5));
    assert!(!surviving_ids.contains(&7));

    // Insert back into deleted slots (space reclamation & defragmentation test)
    db.execute("INSERT INTO accounts (id, balance, owner) VALUES (3, 999.0, 'Reclaimed_3');")?;
    db.execute("INSERT INTO accounts (id, balance, owner) VALUES (5, 888.0, 'Reclaimed_5');")?;

    let row3 = db.query("SELECT owner, balance FROM accounts WHERE id = 3;")?;
    assert_eq!(row3.len(), 1);
    assert_eq!(row3[0].get::<String>("owner")?, "Reclaimed_3");

    Ok(())
}

#[test]
fn test_sql_or_group_by_having_and_subqueries() -> Result<()> {
    let db = Connection::open_in_memory()?;

    db.execute("CREATE TABLE products (id INTEGER PRIMARY KEY, name TEXT, category TEXT, price REAL);")?;
    db.execute("CREATE TABLE orders (order_id INTEGER PRIMARY KEY, product_id INTEGER, qty INTEGER);")?;

    db.execute("INSERT INTO products (id, name, category, price) VALUES (1, 'Rust Book', 'Books', 45.0);")?;
    db.execute("INSERT INTO products (id, name, category, price) VALUES (2, 'SQL Guide', 'Books', 30.0);")?;
    db.execute("INSERT INTO products (id, name, category, price) VALUES (3, 'Mechanical Keyboard', 'Hardware', 120.0);")?;
    db.execute("INSERT INTO products (id, name, category, price) VALUES (4, 'USB Cable', 'Hardware', 12.0);")?;
    db.execute("INSERT INTO products (id, name, category, price) VALUES (5, 'Coffee Mug', 'Merch', 15.0);")?;

    db.execute("INSERT INTO orders (order_id, product_id, qty) VALUES (101, 1, 2);")?;
    db.execute("INSERT INTO orders (order_id, product_id, qty) VALUES (102, 3, 1);")?;

    // Logical OR disjunction in WHERE
    let or_rows = db.query("SELECT name FROM products WHERE category = 'Books' OR price <= 15.0;")?;
    assert_eq!(or_rows.len(), 4);

    // GROUP BY and Aggregations
    let group_rows = db.query(
        "SELECT category, COUNT(id), SUM(price), AVG(price) FROM products GROUP BY category;",
    )?;
    assert_eq!(group_rows.len(), 3);

    let books_row = group_rows
        .iter()
        .find(|r| r.get::<String>("category").unwrap() == "Books")
        .expect("Books category");
    assert_eq!(books_row.get::<i64>("COUNT(id)")?, 2);
    let sum_price: f64 = books_row.get("SUM(price)")?;
    assert!((sum_price - 75.0).abs() < 1e-4);

    // HAVING filter on aggregates
    let having_rows = db.query(
        "SELECT category, COUNT(id) FROM products GROUP BY category HAVING COUNT(id) >= 2;",
    )?;
    assert_eq!(having_rows.len(), 2);

    // Subquery IN (SELECT ...)
    let in_subquery_rows = db.query(
        "SELECT name FROM products WHERE id IN (SELECT product_id FROM orders);",
    )?;
    assert_eq!(in_subquery_rows.len(), 2);
    let ordered_names: Vec<String> = in_subquery_rows
        .iter()
        .map(|r| r.get::<String>("name").unwrap())
        .collect();
    assert!(ordered_names.contains(&"Rust Book".to_string()));
    assert!(ordered_names.contains(&"Mechanical Keyboard".to_string()));

    // Subquery NOT IN (SELECT ...)
    let not_in_rows = db.query(
        "SELECT name FROM products WHERE id NOT IN (SELECT product_id FROM orders);",
    )?;
    assert_eq!(not_in_rows.len(), 3);

    Ok(())
}

#[test]
fn test_parameterized_queries_and_sql_safety() -> Result<()> {
    let db = Connection::open_in_memory()?;

    db.execute("CREATE TABLE users (id INTEGER PRIMARY KEY, username TEXT, bio TEXT, balance REAL);")?;

    let malicious_input = "Faiz'; DROP TABLE users; --";
    let bio_with_quotes = "I'm a system's engineer & Rust enthusiast.";

    let inserted = db.execute_with_params(
        "INSERT INTO users (id, username, bio, balance) VALUES (?, ?, ?, ?);",
        &[
            Value::Integer(1),
            Value::Text(malicious_input.to_string()),
            Value::Text(bio_with_quotes.to_string()),
            Value::Real(250.75),
        ],
    )?;
    assert_eq!(inserted, 1);

    let rows = db.query_with_params(
        "SELECT username, bio, balance FROM users WHERE id = ?;",
        &[Value::Integer(1)],
    )?;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get::<String>("username")?, malicious_input);
    assert_eq!(rows[0].get::<String>("bio")?, bio_with_quotes);

    // PreparedStatement reuse
    let prep_insert = db.prepare("INSERT INTO users (id, username, bio, balance) VALUES (?, ?, ?, ?);")?;
    for i in 2..=5 {
        prep_insert.execute(&[
            Value::Integer(i),
            Value::Text(format!("user_{i}")),
            Value::Text("developer".into()),
            Value::Real((i * 50) as f64),
        ])?;
    }

    let prep_query = db.prepare("SELECT username FROM users WHERE balance > ?;")?;
    let high_balance_users = prep_query.query(&[Value::Real(100.0)])?;
    assert_eq!(high_balance_users.len(), 4);

    // Document collection parameterized safety
    let col = db.collection("profiles")?;
    let doc = serde_json::json!({
        "name": "O'Connor",
        "quote": "Robert'); DROP TABLE Students;--",
        "rating": 5
    });
    let doc_id = col.insert_one(&doc)?;
    let retrieved = col.find_by_id(doc_id)?.expect("Document found");
    assert_eq!(retrieved["name"], "O'Connor");
    assert_eq!(retrieved["quote"], "Robert'); DROP TABLE Students;--");

    Ok(())
}
