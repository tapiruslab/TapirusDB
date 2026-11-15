#[allow(unused_imports)]
use super::*;
use crate::sql::parser::parse_sql;

#[test]
fn test_executor_create_insert_select_and_vector() {
    let mut pager = Pager::open_in_memory(4096, 128).expect("Pager open");
    let mut executor = SQLExecutor::new(&mut pager).expect("Init executor");

    // 1. CREATE TABLE
    let create_sql =
        "CREATE TABLE articles (id INTEGER PRIMARY KEY, title TEXT, embedding VECTOR(3));";
    let res = executor
        .execute(&mut pager, parse_sql(create_sql).unwrap())
        .expect("Execute CREATE TABLE");
    assert_eq!(res, 1);

    // 2. INSERT records
    executor
            .execute(
                &mut pager,
                parse_sql("INSERT INTO articles (id, title, embedding) VALUES (1, 'Safe Rust Systems', [1.0, 0.0, 0.0]);").unwrap(),
            )
            .expect("Insert 1");

    executor
            .execute(
                &mut pager,
                parse_sql("INSERT INTO articles (id, title, embedding) VALUES (2, 'AI Database Engines', [0.0, 1.0, 0.0]);").unwrap(),
            )
            .expect("Insert 2");

    // 3. SELECT *
    let rows = executor
        .query(
            &mut pager,
            parse_sql("SELECT id, title FROM articles;").unwrap(),
        )
        .expect("Query all");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].get::<i64>("id").unwrap(), 1);
    assert_eq!(rows[0].get::<String>("title").unwrap(), "Safe Rust Systems");

    // 4. POINT LOOKUP by Primary Key (WHERE id = 2)
    let point_rows = executor
        .query(
            &mut pager,
            parse_sql("SELECT title FROM articles WHERE id = 2;").unwrap(),
        )
        .expect("Query point");
    assert_eq!(point_rows.len(), 1);
    assert_eq!(
        point_rows[0].get::<String>("title").unwrap(),
        "AI Database Engines"
    );

    // 5. VECTOR SIMILARITY SEARCH
    let vector_rows = executor
        .query(
            &mut pager,
            parse_sql(
                "SELECT id, title FROM articles VECTOR NEAR embedding = [0.95, 0.05, 0.0] TOP 1;",
            )
            .unwrap(),
        )
        .expect("Vector search");
    assert_eq!(vector_rows.len(), 1);
    assert_eq!(
        vector_rows[0].get::<String>("title").unwrap(),
        "Safe Rust Systems"
    );
}

#[test]
fn test_executor_order_by_aggregates_and_join() {
    let mut pager = Pager::open_in_memory(4096, 128).expect("Pager open");
    let mut executor = SQLExecutor::new(&mut pager).expect("Init executor");

    executor
        .execute(
            &mut pager,
            parse_sql("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT);").unwrap(),
        )
        .unwrap();
    executor
        .execute(
            &mut pager,
            parse_sql(
                "CREATE TABLE orders (id INTEGER PRIMARY KEY, user_id INTEGER, amount REAL);",
            )
            .unwrap(),
        )
        .unwrap();

    executor
        .execute(
            &mut pager,
            parse_sql("INSERT INTO users (id, name) VALUES (1, 'Alice');").unwrap(),
        )
        .unwrap();
    executor
        .execute(
            &mut pager,
            parse_sql("INSERT INTO users (id, name) VALUES (2, 'Bob');").unwrap(),
        )
        .unwrap();

    executor
        .execute(
            &mut pager,
            parse_sql("INSERT INTO orders (id, user_id, amount) VALUES (10, 1, 150.0);").unwrap(),
        )
        .unwrap();
    executor
        .execute(
            &mut pager,
            parse_sql("INSERT INTO orders (id, user_id, amount) VALUES (20, 1, 50.0);").unwrap(),
        )
        .unwrap();
    executor
        .execute(
            &mut pager,
            parse_sql("INSERT INTO orders (id, user_id, amount) VALUES (30, 2, 200.0);").unwrap(),
        )
        .unwrap();

    // 1. ORDER BY DESC
    let ordered = executor
        .query(
            &mut pager,
            parse_sql("SELECT id, amount FROM orders ORDER BY amount DESC;").unwrap(),
        )
        .unwrap();
    assert_eq!(ordered.len(), 3);
    assert_eq!(ordered[0].get::<f64>("amount").unwrap(), 200.0);
    assert_eq!(ordered[1].get::<f64>("amount").unwrap(), 150.0);
    assert_eq!(ordered[2].get::<f64>("amount").unwrap(), 50.0);

    // 2. Aggregates: COUNT, SUM, AVG, MIN, MAX
    let agg = executor
        .query(
            &mut pager,
            parse_sql(
                "SELECT COUNT(*), SUM(amount), AVG(amount), MIN(amount), MAX(amount) FROM orders;",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(agg.len(), 1);
    assert_eq!(agg[0].get::<i64>("COUNT(*)").unwrap(), 3);
    assert_eq!(agg[0].get::<f64>("SUM(amount)").unwrap(), 400.0);
    assert_eq!(agg[0].get::<f64>("AVG(amount)").unwrap(), 400.0 / 3.0);
    assert_eq!(agg[0].get::<f64>("MIN(amount)").unwrap(), 50.0);
    assert_eq!(agg[0].get::<f64>("MAX(amount)").unwrap(), 200.0);

    // 3. INNER JOIN
    let joined = executor
            .query(
                &mut pager,
                parse_sql("SELECT users.name, orders.amount FROM users INNER JOIN orders ON users.id = orders.user_id ORDER BY orders.amount ASC;").unwrap(),
            )
            .unwrap();
    assert_eq!(joined.len(), 3);
    assert_eq!(joined[0].get::<String>("users.name").unwrap(), "Alice");
    assert_eq!(joined[0].get::<f64>("orders.amount").unwrap(), 50.0);
    assert_eq!(joined[2].get::<String>("users.name").unwrap(), "Bob");
    assert_eq!(joined[2].get::<f64>("orders.amount").unwrap(), 200.0);
}

#[test]
fn test_graph_match_cypher_query() {
    let mut pager = Pager::open_in_memory(4096, 128).expect("Pager open");
    let mut executor = SQLExecutor::new(&mut pager).expect("Init executor");

    // Insert nodes and edge
    executor
            .execute(
                &mut pager,
                parse_sql(r#"GRAPH INSERT NODE 1 LABEL "Person" PROPERTIES "{\"name\":\"Alex\",\"role\":\"founder\"}";"#).unwrap(),
            )
            .unwrap();
    executor
        .execute(
            &mut pager,
            parse_sql(
                r#"GRAPH INSERT NODE 2 LABEL "Project" PROPERTIES "{\"title\":\"TapirusDB\"}";"#,
            )
            .unwrap(),
        )
        .unwrap();
    executor
        .execute(
            &mut pager,
            parse_sql(r#"GRAPH INSERT EDGE 1 2 LABEL "CREATOR_OF";"#).unwrap(),
        )
        .unwrap();

    // 1. Declarative Cypher-style GRAPH MATCH
    let query_sql =
        "GRAPH MATCH (a:Person)-[r:CREATOR_OF]->(b:Project) WHERE a.id = 1 RETURN a.name, b.title;";
    let rows = executor
        .query(&mut pager, parse_sql(query_sql).unwrap())
        .expect("Execute graph match");

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get::<String>("a.name").unwrap(), "Alex");
    assert_eq!(rows[0].get::<String>("b.title").unwrap(), "TapirusDB");

    // 2. Direct MATCH without GRAPH keyword
    let match_sql = "MATCH (a)-[r]->(b) RETURN a.name, b.title;";
    let rows2 = executor
        .query(&mut pager, parse_sql(match_sql).unwrap())
        .expect("Execute direct match");
    assert_eq!(rows2.len(), 1);
    assert_eq!(rows2[0].get::<String>("a.name").unwrap(), "Alex");
}

#[test]
fn test_executor_window_functions() {
    let mut pager = Pager::open_in_memory(4096, 128).expect("Pager open");
    let mut executor = SQLExecutor::new(&mut pager).expect("Init executor");

    executor
        .execute(
            &mut pager,
            parse_sql(
                "CREATE TABLE emp (id INTEGER PRIMARY KEY, name TEXT, dept TEXT, salary INTEGER);",
            )
            .unwrap(),
        )
        .unwrap();

    executor
        .execute(
            &mut pager,
            parse_sql("INSERT INTO emp (id, name, dept, salary) VALUES (1, 'Alice', 'Eng', 9000);")
                .unwrap(),
        )
        .unwrap();
    executor
        .execute(
            &mut pager,
            parse_sql("INSERT INTO emp (id, name, dept, salary) VALUES (2, 'Bob', 'Eng', 8000);")
                .unwrap(),
        )
        .unwrap();
    executor
        .execute(
            &mut pager,
            parse_sql(
                "INSERT INTO emp (id, name, dept, salary) VALUES (3, 'Charlie', 'Eng', 8000);",
            )
            .unwrap(),
        )
        .unwrap();
    executor
        .execute(
            &mut pager,
            parse_sql(
                "INSERT INTO emp (id, name, dept, salary) VALUES (4, 'David', 'Sales', 7000);",
            )
            .unwrap(),
        )
        .unwrap();
    executor
        .execute(
            &mut pager,
            parse_sql("INSERT INTO emp (id, name, dept, salary) VALUES (5, 'Eve', 'Sales', 6000);")
                .unwrap(),
        )
        .unwrap();

    // 1. ROW_NUMBER with PARTITION BY and ORDER BY
    let sql_rn = "SELECT id, name, dept, ROW_NUMBER() OVER (PARTITION BY dept ORDER BY salary DESC) AS rn FROM emp ORDER BY id ASC;";
    let rows_rn = executor
        .query(&mut pager, parse_sql(sql_rn).unwrap())
        .expect("Query ROW_NUMBER");
    assert_eq!(rows_rn.len(), 5);
    // Alice is rank 1 in Eng
    assert_eq!(rows_rn[0].get::<i64>("rn").unwrap(), 1);
    // David is rank 1 in Sales
    assert_eq!(rows_rn[3].get::<i64>("rn").unwrap(), 1);
    // Eve is rank 2 in Sales
    assert_eq!(rows_rn[4].get::<i64>("rn").unwrap(), 2);

    // 2. DENSE_RANK
    let sql_dr = "SELECT id, salary, DENSE_RANK() OVER (ORDER BY salary DESC) AS drank FROM emp ORDER BY salary DESC;";
    let rows_dr = executor
        .query(&mut pager, parse_sql(sql_dr).unwrap())
        .expect("Query DENSE_RANK");
    assert_eq!(rows_dr[0].get::<i64>("drank").unwrap(), 1); // 9000
    assert_eq!(rows_dr[1].get::<i64>("drank").unwrap(), 2); // 8000
    assert_eq!(rows_dr[2].get::<i64>("drank").unwrap(), 2); // 8000 tie
    assert_eq!(rows_dr[3].get::<i64>("drank").unwrap(), 3); // 7000

    // 3. LAG and LEAD
    let sql_lag = "SELECT id, salary, LAG(salary, 1) OVER (ORDER BY id ASC) AS prev_sal, LEAD(salary, 1) OVER (ORDER BY id ASC) AS next_sal FROM emp ORDER BY id ASC;";
    let rows_lag = executor
        .query(&mut pager, parse_sql(sql_lag).unwrap())
        .expect("Query LAG and LEAD");
    assert_eq!(rows_lag[0].get_value("prev_sal"), Some(&Value::Null));
    assert_eq!(rows_lag[0].get::<i64>("next_sal").unwrap(), 8000);
    assert_eq!(rows_lag[1].get::<i64>("prev_sal").unwrap(), 9000);

    // 4. Window SUM with PARTITION BY
    let sql_sum = "SELECT id, dept, SUM(salary) OVER (PARTITION BY dept) AS total_dept FROM emp ORDER BY id ASC;";
    let rows_sum = executor
        .query(&mut pager, parse_sql(sql_sum).unwrap())
        .expect("Query Window SUM");
    assert_eq!(rows_sum[0].get::<i64>("total_dept").unwrap(), 25000); // Eng: 9000+8000+8000
    assert_eq!(rows_sum[3].get::<i64>("total_dept").unwrap(), 13000); // Sales: 7000+6000
}

#[test]
fn test_executor_graph_algorithms() {
    let mut pager = Pager::open_in_memory(4096, 128).expect("Pager open");
    let mut executor = SQLExecutor::new(&mut pager).expect("Init executor");

    executor
        .execute(
            &mut pager,
            parse_sql("GRAPH INSERT NODE 1 LABEL \"Server\" PROPERTIES \"{}\";").unwrap(),
        )
        .unwrap();
    executor
        .execute(
            &mut pager,
            parse_sql("GRAPH INSERT NODE 2 LABEL \"Database\" PROPERTIES \"{}\";").unwrap(),
        )
        .unwrap();
    executor
        .execute(
            &mut pager,
            parse_sql("GRAPH INSERT NODE 3 LABEL \"Cache\" PROPERTIES \"{}\";").unwrap(),
        )
        .unwrap();
    executor
        .execute(
            &mut pager,
            parse_sql("GRAPH INSERT EDGE 1 2 LABEL \"CONNECTS\";").unwrap(),
        )
        .unwrap();
    executor
        .execute(
            &mut pager,
            parse_sql("GRAPH INSERT EDGE 2 3 LABEL \"CONNECTS\";").unwrap(),
        )
        .unwrap();
    executor
        .execute(
            &mut pager,
            parse_sql("GRAPH INSERT EDGE 3 1 LABEL \"CONNECTS\";").unwrap(),
        )
        .unwrap();

    // 1. GRAPH ALGORITHM PAGERANK
    let pr_rows = executor
        .query(&mut pager, parse_sql("GRAPH ALGORITHM PAGERANK;").unwrap())
        .expect("PageRank query");
    assert_eq!(pr_rows.len(), 3);
    assert_eq!(pr_rows[0].columns(), &["node_id", "label", "pagerank"]);

    // 2. GRAPH ALGORITHM CONNECTED_COMPONENTS
    let cc_rows = executor
        .query(
            &mut pager,
            parse_sql("GRAPH ALGORITHM CONNECTED_COMPONENTS;").unwrap(),
        )
        .expect("WCC query");
    assert_eq!(cc_rows.len(), 3);
    assert_eq!(
        cc_rows[0].get::<i64>("component_id").unwrap(),
        cc_rows[1].get::<i64>("component_id").unwrap()
    );

    // 3. GRAPH ALGORITHM BETWEENNESS
    let bc_rows = executor
        .query(
            &mut pager,
            parse_sql("GRAPH ALGORITHM BETWEENNESS;").unwrap(),
        )
        .expect("BC query");
    assert_eq!(bc_rows.len(), 3);

    // 4. GRAPH ALGORITHM LOUVAIN
    let lv_rows = executor
        .query(&mut pager, parse_sql("GRAPH ALGORITHM LOUVAIN;").unwrap())
        .expect("Louvain query");
    assert_eq!(lv_rows.len(), 3);
}

#[test]
fn test_executor_tap_decision_functions() {
    let mut pager = Pager::open_in_memory(4096, 128).expect("Pager open");
    let mut executor = SQLExecutor::new(&mut pager).expect("Init executor");

    executor
        .execute(
            &mut pager,
            parse_sql(
                "CREATE TABLE tickets (id INTEGER PRIMARY KEY, content TEXT, category TEXT);",
            )
            .unwrap(),
        )
        .unwrap();
    executor.execute(&mut pager, parse_sql("INSERT INTO tickets VALUES (1, 'Urgent billing payment failed during checkout', 'billing');").unwrap()).unwrap();
    executor.execute(&mut pager, parse_sql("INSERT INTO tickets VALUES (2, 'Cannot connect to wifi router hardware', 'hardware');").unwrap()).unwrap();

    // 1. SELECT with TAP_CLASSIFY
    let rows = executor.query(&mut pager, parse_sql("SELECT id, TAP_CLASSIFY(content, 'billing, technical, sales') AS predicted FROM tickets;").unwrap()).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].get::<String>("predicted").unwrap(), "billing");

    // 2. SELECT with TAP_VERIFY in projection
    let rows_v = executor.query(&mut pager, parse_sql("SELECT id, TAP_VERIFY(content, 'payment error occurred') AS is_payment_issue FROM tickets;").unwrap()).unwrap();
    assert_eq!(rows_v.len(), 2);
    assert_eq!(rows_v[0].get::<i64>("is_payment_issue").unwrap(), 1);

    // 3. SELECT with TAP_SCORE in projection
    let rows_s = executor
        .query(
            &mut pager,
            parse_sql("SELECT id, TAP_SCORE(content, 'urgent priority') AS urgency FROM tickets;")
                .unwrap(),
        )
        .unwrap();
    assert_eq!(rows_s.len(), 2);
    assert!(rows_s[0].get::<f64>("urgency").unwrap() > 0.4);

    // 4. SELECT with TAP_ROUTE in projection
    let rows_r = executor.query(&mut pager, parse_sql("SELECT id, TAP_ROUTE(content, 'escalate_billing, triage, hardware_team') AS next_step FROM tickets;").unwrap()).unwrap();
    assert_eq!(rows_r.len(), 2);
    assert_eq!(
        rows_r[0].get::<String>("next_step").unwrap(),
        "escalate_billing"
    );

    // 5. WHERE clause filtering with TAP_VERIFY
    let rows_where = executor.query(&mut pager, parse_sql("SELECT id, category FROM tickets WHERE TAP_VERIFY(content, 'wifi router hardware connection failure') = 1;").unwrap()).unwrap();
    assert_eq!(rows_where.len(), 1);
    assert_eq!(rows_where[0].get::<i64>("id").unwrap(), 2);
}

#[test]
fn test_executor_vector_cosine_and_timestamp() {
    let mut pager = Pager::open_in_memory(4096, 128).expect("Pager open");
    let mut executor = SQLExecutor::new(&mut pager).expect("Init executor");

    executor.execute(&mut pager, parse_sql("CREATE TABLE embeddings (doc_id INTEGER PRIMARY KEY, content TEXT, vector VECTOR(3), created_at TIMESTAMP);").unwrap()).unwrap();
    executor.execute(&mut pager, parse_sql("INSERT INTO embeddings VALUES (1, 'Machine Learning', [1.0, 0.0, 0.0], '2026-10-09T08:00:00Z');").unwrap()).unwrap();
    executor.execute(&mut pager, parse_sql("INSERT INTO embeddings VALUES (2, 'Culinary Arts', [0.0, 1.0, 0.0], '2026-10-09T08:01:00Z');").unwrap()).unwrap();

    // 1. SELECT with VECTOR_COSINE in projection
    let rows = executor.query(&mut pager, parse_sql("SELECT doc_id, content, VECTOR_COSINE(vector, [0.9, 0.1, 0.0]) AS score FROM embeddings ORDER BY score DESC;").unwrap()).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].get::<i64>("doc_id").unwrap(), 1);
    let score_1 = rows[0].get::<f64>("score").unwrap();
    let score_2 = rows[1].get::<f64>("score").unwrap();
    assert!(score_1 > 0.9);
    assert!(score_2 < 0.2);

    // 2. SELECT with VECTOR_L2 in projection
    let rows_l2 = executor.query(&mut pager, parse_sql("SELECT doc_id, VECTOR_L2(vector, [1.0, 0.0, 0.0]) AS dist FROM embeddings ORDER BY dist ASC;").unwrap()).unwrap();
    assert_eq!(rows_l2[0].get::<i64>("doc_id").unwrap(), 1);
    assert_eq!(rows_l2[0].get::<f64>("dist").unwrap(), 0.0);
}

#[test]
fn test_index_bucket_binary_codec_and_legacy_json_compat() {
    let mut bucket = IndexBucket::new();
    bucket.add_row_id(Value::Text("alpha".into()), 10);
    bucket.add_row_id(Value::Text("alpha".into()), 25);
    bucket.add_row_id(Value::Integer(-42), 7);

    let encoded = bucket.to_bytes();
    assert_eq!(encoded[0], INDEX_BUCKET_MAGIC);
    let decoded = IndexBucket::from_bytes(&encoded).expect("decode binary bucket");
    assert_eq!(decoded, bucket);

    let legacy_json = serde_json::to_vec(&bucket).expect("encode legacy json");
    let decoded_legacy = IndexBucket::from_bytes(&legacy_json).expect("decode legacy json bucket");
    assert_eq!(decoded_legacy, bucket);

    // Corrupted payload must fail cleanly instead of silently wiping posting lists.
    let mut bad = encoded.clone();
    bad[1] = 0x7F;
    assert!(IndexBucket::from_bytes(&bad).is_err());
}

#[test]
fn test_phase2_pk_range_order_pushdown_topn_and_distinct() {
    let mut pager = Pager::open_in_memory(4096, 128).expect("Pager open");
    let mut executor = SQLExecutor::new(&mut pager).expect("Init executor");

    executor
        .execute(
            &mut pager,
            parse_sql("CREATE TABLE events (id INTEGER PRIMARY KEY, kind TEXT, score INTEGER);")
                .unwrap(),
        )
        .unwrap();
    executor
        .execute(
            &mut pager,
            parse_sql("CREATE INDEX idx_events_kind ON events (kind);").unwrap(),
        )
        .unwrap();

    for id in [-15i64, -3, 1, 2, 5, 10, 12, 18, 25, 30] {
        let kind = if id % 2 == 0 { "even" } else { "odd" };
        let score = (id * 7) % 19;
        let sql = format!("INSERT INTO events VALUES ({id}, '{kind}', {score});");
        executor
            .execute(&mut pager, parse_sql(&sql).unwrap())
            .unwrap();
    }

    // 1. PK Range Scan + ORDER BY id DESC LIMIT/OFFSET across negative and positive keys
    let desc_rows = executor
        .query(
            &mut pager,
            parse_sql("SELECT id FROM events WHERE id >= -3 AND id <= 25 ORDER BY id DESC LIMIT 4 OFFSET 1;")
                .unwrap(),
        )
        .unwrap();
    let desc_ids: Vec<i64> = desc_rows
        .iter()
        .map(|r| r.get::<i64>("id").unwrap())
        .collect();
    assert_eq!(desc_ids, vec![18, 12, 10, 5]);

    // 2. Single-pass aggregate with PK Range pushdown
    let agg_rows = executor
        .query(
            &mut pager,
            parse_sql(
                "SELECT COUNT(*) AS cnt, SUM(id) AS total FROM events WHERE id > 1 AND id < 20;",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(agg_rows[0].get::<i64>("cnt").unwrap(), 5);
    assert_eq!(
        agg_rows[0].get::<i64>("total").unwrap(),
        2 + 5 + 10 + 12 + 18
    );

    // 3. Secondary Index + ORDER BY (non-PK top-N partial sort)
    let topn = executor
        .query(
            &mut pager,
            parse_sql("SELECT id, score FROM events WHERE kind = 'even' ORDER BY score DESC LIMIT 2 OFFSET 1;")
                .unwrap(),
        )
        .unwrap();
    assert_eq!(topn.len(), 2);
    assert!(topn[0].get::<i64>("score").unwrap() >= topn[1].get::<i64>("score").unwrap());

    // 4. O(N) RowDistinctSet across duplicate values with OFFSET + LIMIT
    let distinct_rows = executor
        .query(
            &mut pager,
            parse_sql("SELECT DISTINCT kind FROM events ORDER BY kind ASC LIMIT 1 OFFSET 1;")
                .unwrap(),
        )
        .unwrap();
    assert_eq!(distinct_rows.len(), 1);
    assert_eq!(distinct_rows[0].get::<String>("kind").unwrap(), "odd");

    // 5. EXPLAIN QUERY PLAN reflects PK Range + PK ORDER pushdown without temp B-Tree
    let explain = executor
        .query(
            &mut pager,
            parse_sql(
                "EXPLAIN QUERY PLAN SELECT id FROM events WHERE id >= 1 ORDER BY id DESC LIMIT 5;",
            )
            .unwrap(),
        )
        .unwrap();
    let details: Vec<String> = explain
        .iter()
        .map(|r| r.get::<String>("detail").unwrap())
        .collect();
    assert!(
        details
            .iter()
            .any(|d| d.contains("RANGE SCAN TABLE events USING PRIMARY KEY ORDER (DESC)"))
    );
    assert!(!details.iter().any(|d| d.contains("USE TEMP B-TREE")));

    // 6. Shadowed alias preserves exact pre-projection ORDER BY table-column ordering
    let aliased = executor
        .query(
            &mut pager,
            parse_sql("SELECT 0 - id AS id FROM events WHERE id >= 10 ORDER BY id ASC LIMIT 2;")
                .unwrap(),
        )
        .unwrap();
    let aliased_ids: Vec<i64> = aliased
        .iter()
        .map(|r| r.get::<i64>("id").unwrap())
        .collect();
    assert_eq!(aliased_ids, vec![-10, -12]);
}

#[test]
fn test_phase3_inset_exists_like_dml_pushdown_and_filtered_agg() {
    let mut pager = Pager::open_in_memory(4096, 128).expect("Pager open");
    let mut executor = SQLExecutor::new(&mut pager).expect("Executor new");

    executor
        .execute(
            &mut pager,
            parse_sql(
                "CREATE TABLE products (id INTEGER PRIMARY KEY, sku TEXT UNIQUE, category TEXT, price INTEGER CHECK (price >= 0), notes TEXT);",
            )
            .unwrap(),
        )
        .unwrap();
    executor
        .execute(
            &mut pager,
            parse_sql("CREATE INDEX idx_prod_cat ON products (category);").unwrap(),
        )
        .unwrap();
    executor
        .execute(
            &mut pager,
            parse_sql(
                "CREATE TABLE orders (id INTEGER PRIMARY KEY, prod_id INTEGER REFERENCES products(id) ON DELETE CASCADE, qty INTEGER);",
            )
            .unwrap(),
        )
        .unwrap();
    executor
        .execute(
            &mut pager,
            parse_sql("CREATE INDEX idx_ord_prod ON orders (prod_id);").unwrap(),
        )
        .unwrap();

    for i in 1..=20 {
        let cat = if i % 2 == 0 { "Alpha" } else { "Beta" };
        executor
            .execute(
                &mut pager,
                parse_sql(&format!(
                    "INSERT INTO products VALUES ({i}, 'SKU-{i:03}', '{cat}', {}, 'Item #{i} — Kafe');",
                    i * 10
                ))
                .unwrap(),
            )
            .unwrap();
        executor
            .execute(
                &mut pager,
                parse_sql(&format!(
                    "INSERT INTO orders VALUES ({}, {i}, {});",
                    i + 100,
                    i % 3 + 1
                ))
                .unwrap(),
            )
            .unwrap();
    }

    // 1. O(1) IN / NOT IN with NULL three-valued logic and Int/Real equivalence
    let in_rows = executor
        .query(
            &mut pager,
            parse_sql("SELECT id FROM products WHERE price IN (20.0, 50, 999) ORDER BY id ASC;")
                .unwrap(),
        )
        .unwrap();
    let in_ids: Vec<i64> = in_rows
        .iter()
        .map(|r| r.get::<i64>("id").unwrap())
        .collect();
    assert_eq!(in_ids, vec![2, 5]);

    let not_in_null = executor
        .query(
            &mut pager,
            parse_sql("SELECT id FROM products WHERE id NOT IN (1, NULL);").unwrap(),
        )
        .unwrap();
    assert!(not_in_null.is_empty());

    // 2. IN (SELECT ...) + EXISTS / NOT EXISTS
    let sub_rows = executor
        .query(
            &mut pager,
            parse_sql(
                "SELECT id FROM products WHERE id IN (SELECT prod_id FROM orders WHERE qty = 3) AND EXISTS (SELECT id FROM products WHERE category = 'Alpha') ORDER BY id ASC;",
            )
            .unwrap(),
        )
        .unwrap();
    assert!(!sub_rows.is_empty());
    assert!(
        sub_rows
            .iter()
            .all(|r| r.get::<i64>("id").unwrap() % 3 == 2)
    );

    // 3. Zero-alloc ASCII & Unicode LIKE
    let like_ascii = executor
        .query(
            &mut pager,
            parse_sql("SELECT id FROM products WHERE sku LIKE 'sku-01_' ORDER BY id ASC;").unwrap(),
        )
        .unwrap();
    assert_eq!(like_ascii.len(), 10);

    let like_uni = executor
        .query(
            &mut pager,
            parse_sql("SELECT id FROM products WHERE notes LIKE '%kafe' AND id <= 3;").unwrap(),
        )
        .unwrap();
    assert_eq!(like_uni.len(), 3);

    // 4. Projected Filtered Aggregation + Bounded PK Streaming GROUP BY
    let agg = executor
        .query(
            &mut pager,
            parse_sql(
                "SELECT COUNT(*) AS cnt, SUM(price) AS total FROM products WHERE category = 'Alpha' AND id BETWEEN 2 AND 10;",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(agg[0].get::<i64>("cnt").unwrap(), 5);
    assert_eq!(agg[0].get::<i64>("total").unwrap(), 20 + 40 + 60 + 80 + 100);

    let gb = executor
        .query(
            &mut pager,
            parse_sql(
                "SELECT category, COUNT(*) AS cnt, SUM(price) AS total FROM products WHERE id >= 1 AND id <= 6 AND price >= 20 GROUP BY category ORDER BY category ASC;",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(gb.len(), 2);
    assert_eq!(gb[0].get::<String>("category").unwrap(), "Alpha");
    assert_eq!(gb[0].get::<i64>("cnt").unwrap(), 3);
    assert_eq!(gb[1].get::<String>("category").unwrap(), "Beta");
    assert_eq!(gb[1].get::<i64>("cnt").unwrap(), 2);

    // 5. O(log N) PK & Secondary-Index DML Update / Delete + CASCADE + UNIQUE/CHECK enforcement
    let upd_pk = executor
        .execute(
            &mut pager,
            parse_sql("UPDATE products SET price = 777 WHERE id = 4 AND category = 'Alpha';")
                .unwrap(),
        )
        .unwrap();
    assert_eq!(upd_pk, 1);

    let upd_idx = executor
        .execute(
            &mut pager,
            parse_sql("UPDATE products SET price = 888 WHERE category = 'Beta' AND id = 3;")
                .unwrap(),
        )
        .unwrap();
    assert_eq!(upd_idx, 1);

    assert!(
        executor
            .execute(
                &mut pager,
                parse_sql("UPDATE products SET price = -5 WHERE id = 3;").unwrap(),
            )
            .is_err()
    );
    assert!(
        executor
            .execute(
                &mut pager,
                parse_sql("UPDATE products SET sku = 'SKU-001' WHERE id = 2;").unwrap(),
            )
            .is_err()
    );

    // Unsatisfiable PK range returns 0 cleanly
    let del_empty = executor
        .execute(
            &mut pager,
            parse_sql("DELETE FROM products WHERE id > 10 AND id < 5;").unwrap(),
        )
        .unwrap();
    assert_eq!(del_empty, 0);

    // Delete parent row cascades into indexed child orders via IndexBucket path
    let del_pk = executor
        .execute(
            &mut pager,
            parse_sql("DELETE FROM products WHERE id = 4;").unwrap(),
        )
        .unwrap();
    assert_eq!(del_pk, 1);
    let orphan_orders = executor
        .query(
            &mut pager,
            parse_sql("SELECT id FROM orders WHERE prod_id = 4;").unwrap(),
        )
        .unwrap();
    assert!(orphan_orders.is_empty());
}

#[test]
fn test_phase4_chained_joins_predicate_pushdown_inset_indexes_and_agg_guards() {
    let mut pager = Pager::open_in_memory(4096, 128).expect("Pager open");
    let mut executor = SQLExecutor::new(&mut pager).expect("Executor new");

    for ddl in [
        "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT, tier TEXT);",
        "CREATE TABLE products (id INTEGER PRIMARY KEY, title TEXT, tag TEXT, price INTEGER);",
        "CREATE INDEX idx_products_tag ON products (tag);",
        "CREATE TABLE orders (id INTEGER PRIMARY KEY, user_id INTEGER, product_id INTEGER, qty INTEGER);",
        "CREATE INDEX idx_orders_user ON orders (user_id);",
    ] {
        executor
            .execute(&mut pager, parse_sql(ddl).unwrap())
            .unwrap();
    }

    for row_sql in [
        "INSERT INTO users VALUES (1, 'Alice', 'gold');",
        "INSERT INTO users VALUES (2, 'Bob', 'silver');",
        "INSERT INTO users VALUES (3, 'Charlie', 'gold');",
        "INSERT INTO products VALUES (10, 'Laptop', 'tech', 1200);",
        "INSERT INTO products VALUES (11, 'Mouse', 'tech', 25);",
        "INSERT INTO products VALUES (12, 'Desk', 'office', 350);",
        "INSERT INTO products VALUES (13, 'Pen', 'office', 5);",
        "INSERT INTO orders VALUES (100, 1, 10, 1);",
        "INSERT INTO orders VALUES (101, 1, 12, 2);",
        "INSERT INTO orders VALUES (102, 2, 10, 4);",
        "INSERT INTO orders VALUES (103, 3, 13, 10);",
        "INSERT INTO orders VALUES (104, 3, 11, 3);",
    ] {
        executor
            .execute(&mut pager, parse_sql(row_sql).unwrap())
            .unwrap();
    }

    // 1. PK Point Lookup must NOT bypass aggregates (COUNT/SUM with WHERE pk = X)
    let agg_pk = executor
        .query(
            &mut pager,
            parse_sql("SELECT COUNT(*), SUM(price) FROM products WHERE id = 10;").unwrap(),
        )
        .unwrap();
    assert_eq!(agg_pk.len(), 1);
    assert_eq!(
        agg_pk[0].values(),
        &[Value::Integer(1), Value::Integer(1200)]
    );

    let agg_empty = executor
        .query(
            &mut pager,
            parse_sql("SELECT COUNT(*) FROM products WHERE id > 99 AND id < 10;").unwrap(),
        )
        .unwrap();
    assert_eq!(agg_empty.len(), 1);
    assert_eq!(agg_empty[0].values(), &[Value::Integer(0)]);

    // 2. Compound-AND PK lookup + projected scan + ORDER BY
    let compound_pk = executor
        .query(
            &mut pager,
            parse_sql(
                "SELECT id, title FROM products WHERE id = 12 AND tag = 'office' ORDER BY title;",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(compound_pk.len(), 1);
    assert_eq!(compound_pk[0].get::<String>("title").unwrap(), "Desk");

    // 3. Multi-key PK IN (...) & Secondary Index IN (...) + EXPLAIN plan verification
    let pk_in = executor
        .query(
            &mut pager,
            parse_sql("SELECT id, title FROM products WHERE id IN (13, 10, 99) ORDER BY id DESC;")
                .unwrap(),
        )
        .unwrap();
    let pk_in_ids: Vec<i64> = pk_in.iter().map(|r| r.get::<i64>("id").unwrap()).collect();
    assert_eq!(pk_in_ids, vec![13, 10]);

    let idx_in = executor
        .query(
            &mut pager,
            parse_sql("SELECT id FROM products WHERE tag IN ('tech', 'missing') ORDER BY id ASC;")
                .unwrap(),
        )
        .unwrap();
    let idx_in_ids: Vec<i64> = idx_in.iter().map(|r| r.get::<i64>("id").unwrap()).collect();
    assert_eq!(idx_in_ids, vec![10, 11]);

    let explain_pk_in = executor
        .query(
            &mut pager,
            parse_sql("EXPLAIN SELECT id FROM products WHERE id IN (10, 12);").unwrap(),
        )
        .unwrap();
    let detail = explain_pk_in[0].get::<String>("detail").unwrap();
    assert!(
        detail.contains("PRIMARY KEY") && detail.contains("IN"),
        "Expected PK IN plan, got {detail}"
    );

    // 4. Chained 3-table JOIN with table aliases, right/multi-table WHERE predicates, reversed ON keys, ORDER BY, LIMIT
    let joined = executor
        .query(
            &mut pager,
            parse_sql(
                "SELECT u.name AS buyer, p.title AS item, o.qty AS qty \
                 FROM orders o \
                 INNER JOIN users u ON o.user_id = u.id \
                 INNER JOIN products p ON p.id = o.product_id \
                 WHERE u.tier = 'gold' AND p.price >= 25 \
                 ORDER BY o.qty DESC \
                 LIMIT 2;",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(joined.len(), 2);
    assert_eq!(joined[0].get::<String>("buyer").unwrap(), "Charlie");
    assert_eq!(joined[0].get::<String>("item").unwrap(), "Mouse");
    assert_eq!(joined[0].get::<i64>("qty").unwrap(), 3);
    assert_eq!(joined[1].get::<String>("buyer").unwrap(), "Alice");
    assert_eq!(joined[1].get::<String>("item").unwrap(), "Desk");
    assert_eq!(joined[1].get::<i64>("qty").unwrap(), 2);

    // 5. Right-side predicate on single INNER JOIN must NOT falsely drop left rows on wrong table
    let right_filtered = executor
        .query(
            &mut pager,
            parse_sql(
                "SELECT users.name, orders.id FROM users INNER JOIN orders ON users.id = orders.user_id WHERE orders.qty = 4;",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(right_filtered.len(), 1);
    assert_eq!(right_filtered[0].get::<String>("name").unwrap(), "Bob");

    // 6. LEFT JOIN with right-table WHERE filter must preserve standard SQL post-join semantics
    let left_join_filtered = executor
        .query(
            &mut pager,
            parse_sql(
                "SELECT users.name, orders.qty FROM users LEFT JOIN orders ON users.id = orders.user_id WHERE orders.qty >= 4 ORDER BY users.id ASC;",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(left_join_filtered.len(), 2);
    assert_eq!(left_join_filtered[0].get::<String>("name").unwrap(), "Bob");
    assert_eq!(
        left_join_filtered[1].get::<String>("name").unwrap(),
        "Charlie"
    );
}
