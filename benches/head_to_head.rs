//! # Head-to-Head Benchmark: TapirusDB vs SQLite (rusqlite)
//!
//! Evaluates reproducible, side-by-side performance on the exact same host hardware:
//! 1. Bulk Batch Insert (5,000 rows inside a single ACID WAL transaction)
//! 2. Primary Key Point Lookup (5,000 point queries)
//! 3. Aggregation (COUNT, SUM, AVG across 5,000 rows)
//! 4. Hash Join (1,000 users x 1,000 orders)
//! 5. Multi-Model AI Vector Search (128-dim HNSW KNN)
//!
//! Complies strictly with `#![forbid(unsafe_code)]`.

#![forbid(unsafe_code)]

use std::time::Instant;
use tapirus::Connection as TapirusConn;
use tempfile::NamedTempFile;

fn format_ops(count: usize, duration_secs: f64) -> String {
    let ops = count as f64 / duration_secs;
    if ops >= 1_000_000.0 {
        format!("{:.2}M ops/s", ops / 1_000_000.0)
    } else if ops >= 1_000.0 {
        format!("{:.1}k ops/s", ops / 1_000.0)
    } else {
        format!("{:.0} ops/s", ops)
    }
}

fn main() {
    println!("===============================================================================");
    println!("     TAPIRUSDB vs SQLITE: SCIENTIFIC HEAD-TO-HEAD BENCHMARK SUITE             ");
    println!("===============================================================================");
    println!("Hardware: {}", std::env::consts::ARCH);
    println!("OS:       {}", std::env::consts::OS);
    println!("Rust:     Safe Rust (#![forbid(unsafe_code)]) vs C SQLite 3.x");
    println!("Mode:     Persistent Disk Storage with Write-Ahead Logging (WAL)");
    println!("-------------------------------------------------------------------------------\n");

    const ROW_COUNT: usize = 5000;
    const POINT_QUERIES: usize = 5000;
    const JOIN_ROWS: usize = 1000;

    // --- Temporary database files ---
    let tapir_file = NamedTempFile::new().expect("Create tapir tempfile");
    let sqlite_file = NamedTempFile::new().expect("Create sqlite tempfile");

    let tapir_path = tapir_file.path();
    let sqlite_path = sqlite_file.path();

    // -------------------------------------------------------------------------
    // 1. BULK INSERT (5,000 Rows inside ACID WAL Transaction)
    // -------------------------------------------------------------------------
    println!("Benchmarking 1/5: Bulk Insert ({ROW_COUNT} rows in single transaction)...");

    // TapirusDB
    let tapir_conn = TapirusConn::open(tapir_path).expect("Open TapirusDB");
    tapir_conn
        .execute("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT, score REAL);")
        .expect("Create table");

    let start_tapir_insert = Instant::now();
    tapir_conn.execute("BEGIN TRANSACTION;").expect("Begin");
    for i in 1..=ROW_COUNT {
        let sql = format!("INSERT INTO users VALUES ({i}, 'User_{i}', {});", (i * 7) as f64 * 0.1);
        tapir_conn.execute(&sql).expect("Insert");
    }
    tapir_conn.execute("COMMIT;").expect("Commit");
    let dur_tapir_insert = start_tapir_insert.elapsed();

    // SQLite
    let sqlite_conn = rusqlite::Connection::open(sqlite_path).expect("Open SQLite");
    sqlite_conn.pragma_update(None, "journal_mode", "WAL").expect("WAL mode");
    sqlite_conn.pragma_update(None, "synchronous", "NORMAL").expect("Synchronous NORMAL");
    sqlite_conn
        .execute("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT, score REAL);", [])
        .expect("Create table");

    let start_sqlite_insert = Instant::now();
    sqlite_conn.execute("BEGIN TRANSACTION;", []).expect("Begin");
    {
        let mut stmt = sqlite_conn
            .prepare("INSERT INTO users VALUES (?1, ?2, ?3);")
            .expect("Prepare");
        for i in 1..=ROW_COUNT {
            let name = format!("User_{i}");
            let score = (i * 7) as f64 * 0.1;
            stmt.execute(rusqlite::params![i as i64, name, score]).expect("Insert");
        }
    }
    sqlite_conn.execute("COMMIT;", []).expect("Commit");
    let dur_sqlite_insert = start_sqlite_insert.elapsed();

    // -------------------------------------------------------------------------
    // 2. PRIMARY KEY POINT LOOKUP (5,000 point queries)
    // -------------------------------------------------------------------------
    println!("Benchmarking 2/5: Primary Key Point Lookup ({POINT_QUERIES} queries)...");

    // TapirusDB
    let start_tapir_pk = Instant::now();
    for i in 1..=POINT_QUERIES {
        let id = (i * 17) % ROW_COUNT + 1;
        let sql = format!("SELECT id, name, score FROM users WHERE id = {id};");
        let rows = tapir_conn.query(&sql).expect("Query");
        assert_eq!(rows.len(), 1);
    }
    let dur_tapir_pk = start_tapir_pk.elapsed();

    // SQLite
    let start_sqlite_pk = Instant::now();
    {
        let mut stmt = sqlite_conn
            .prepare("SELECT id, name, score FROM users WHERE id = ?1;")
            .expect("Prepare");
        for i in 1..=POINT_QUERIES {
            let id = ((i * 17) % ROW_COUNT + 1) as i64;
            let mut rows = stmt.query(rusqlite::params![id]).expect("Query");
            let row = rows.next().expect("Row").expect("Some row");
            let _got_id: i64 = row.get(0).unwrap();
        }
    }
    let dur_sqlite_pk = start_sqlite_pk.elapsed();

    // -------------------------------------------------------------------------
    // 3. AGGREGATION SCAN (COUNT, SUM, AVG across 5,000 rows)
    // -------------------------------------------------------------------------
    println!("Benchmarking 3/5: Table Aggregation (COUNT, SUM, AVG across all rows)...");
    const AGG_ITERATIONS: usize = 100;

    let start_tapir_agg = Instant::now();
    for _ in 0..AGG_ITERATIONS {
        let rows = tapir_conn
            .query("SELECT COUNT(id), SUM(score), AVG(score) FROM users;")
            .expect("Agg query");
        assert_eq!(rows.len(), 1);
    }
    let dur_tapir_agg = start_tapir_agg.elapsed();

    let start_sqlite_agg = Instant::now();
    {
        let mut stmt = sqlite_conn
            .prepare("SELECT COUNT(id), SUM(score), AVG(score) FROM users;")
            .expect("Prepare");
        for _ in 0..AGG_ITERATIONS {
            let mut rows = stmt.query([]).expect("Query");
            let row = rows.next().expect("Row").expect("Some row");
            let _cnt: i64 = row.get(0).unwrap();
        }
    }
    let dur_sqlite_agg = start_sqlite_agg.elapsed();

    // -------------------------------------------------------------------------
    // 4. INNER JOIN (1,000 Users x 1,000 Orders)
    // -------------------------------------------------------------------------
    println!("Benchmarking 4/5: Hash Join ({JOIN_ROWS} x {JOIN_ROWS} records)...");

    // Setup orders table
    tapir_conn
        .execute("CREATE TABLE orders (id INTEGER PRIMARY KEY, user_id INTEGER, amount REAL);")
        .expect("Create orders");
    tapir_conn.execute("BEGIN TRANSACTION;").expect("Begin");
    for i in 1..=JOIN_ROWS {
        let sql = format!("INSERT INTO orders VALUES ({i}, {i}, {});", i as f64 * 10.5);
        tapir_conn.execute(&sql).expect("Insert");
    }
    tapir_conn.execute("COMMIT;").expect("Commit");

    sqlite_conn
        .execute("CREATE TABLE orders (id INTEGER PRIMARY KEY, user_id INTEGER, amount REAL);", [])
        .expect("Create orders");
    sqlite_conn.execute("BEGIN TRANSACTION;", []).expect("Begin");
    {
        let mut stmt = sqlite_conn
            .prepare("INSERT INTO orders VALUES (?1, ?2, ?3);")
            .expect("Prepare");
        for i in 1..=JOIN_ROWS {
            stmt.execute(rusqlite::params![i as i64, i as i64, i as f64 * 10.5]).expect("Insert");
        }
    }
    sqlite_conn.execute("COMMIT;", []).expect("Commit");

    const JOIN_ITERATIONS: usize = 20;

    let start_tapir_join = Instant::now();
    for _ in 0..JOIN_ITERATIONS {
        let rows = tapir_conn
            .query("SELECT users.name, orders.amount FROM users INNER JOIN orders ON users.id = orders.user_id;")
            .expect("Join query");
        assert_eq!(rows.len(), JOIN_ROWS);
    }
    let dur_tapir_join = start_tapir_join.elapsed();

    let start_sqlite_join = Instant::now();
    {
        let mut stmt = sqlite_conn
            .prepare("SELECT users.name, orders.amount FROM users INNER JOIN orders ON users.id = orders.user_id;")
            .expect("Prepare");
        for _ in 0..JOIN_ITERATIONS {
            let mut rows = stmt.query([]).expect("Query");
            let mut count = 0;
            while let Some(_) = rows.next().expect("Row") {
                count += 1;
            }
            assert_eq!(count, JOIN_ROWS);
        }
    }
    let dur_sqlite_join = start_sqlite_join.elapsed();

    // -------------------------------------------------------------------------
    // 5. NATIVE MULTI-MODEL: AI Vector Search (128-D HNSW KNN)
    // -------------------------------------------------------------------------
    println!("Benchmarking 5/5: Multi-Model AI Vector Search (128-dim HNSW KNN)...");

    tapir_conn
        .execute("CREATE TABLE docs (id INTEGER PRIMARY KEY, body TEXT, embedding VECTOR(128));")
        .expect("Create docs with vector");

    let query_vec = vec![0.05f32; 128];
    for i in 1..=500 {
        let mut vec = vec![0.0f32; 128];
        vec[i % 128] = 1.0;
        let vec_json = serde_json::to_string(&vec).unwrap();
        let sql = format!("INSERT INTO docs VALUES ({i}, 'Document {i}', '{vec_json}');");
        tapir_conn.execute(&sql).expect("Insert vector");
    }

    let start_tapir_vec = Instant::now();
    const VEC_QUERIES: usize = 50;
    for _ in 0..VEC_QUERIES {
        let q_json = serde_json::to_string(&query_vec).unwrap();
        let sql = format!("SELECT id, body FROM docs VECTOR SEARCH embedding SIMILAR TO '{q_json}' TOP 10;");
        let rows = tapir_conn.query(&sql).expect("Vector search");
        assert_eq!(rows.len(), 10);
    }
    let dur_tapir_vec = start_tapir_vec.elapsed();

    // -------------------------------------------------------------------------
    // SUMMARY RESULTS TABLE
    // -------------------------------------------------------------------------
    println!("\n===============================================================================");
    println!("                        FINAL BENCHMARK COMPARISON TABLE                       ");
    println!("===============================================================================");
    println!(
        "| {:<28} | {:<20} | {:<20} |",
        "Workload Benchmark", "TapirusDB (Safe Rust)", "SQLite 3.x (C WAL)"
    );
    println!("|:-----------------------------|:---------------------|:---------------------|");

    println!(
        "| Bulk Insert ({ROW_COUNT} rows)        | {:<20} | {:<20} |",
        format!("{:.1}ms ({})", dur_tapir_insert.as_secs_f64() * 1000.0, format_ops(ROW_COUNT, dur_tapir_insert.as_secs_f64())),
        format!("{:.1}ms ({})", dur_sqlite_insert.as_secs_f64() * 1000.0, format_ops(ROW_COUNT, dur_sqlite_insert.as_secs_f64()))
    );

    println!(
        "| PK Point Lookup ({POINT_QUERIES} queries) | {:<20} | {:<20} |",
        format!("{:.1}ms ({})", dur_tapir_pk.as_secs_f64() * 1000.0, format_ops(POINT_QUERIES, dur_tapir_pk.as_secs_f64())),
        format!("{:.1}ms ({})", dur_sqlite_pk.as_secs_f64() * 1000.0, format_ops(POINT_QUERIES, dur_sqlite_pk.as_secs_f64()))
    );

    println!(
        "| Aggregate Scan (x{AGG_ITERATIONS} passes)     | {:<20} | {:<20} |",
        format!("{:.1}ms ({})", dur_tapir_agg.as_secs_f64() * 1000.0, format_ops(AGG_ITERATIONS, dur_tapir_agg.as_secs_f64())),
        format!("{:.1}ms ({})", dur_sqlite_agg.as_secs_f64() * 1000.0, format_ops(AGG_ITERATIONS, dur_sqlite_agg.as_secs_f64()))
    );

    println!(
        "| Hash Join ({JOIN_ROWS} x {JOIN_ROWS} x{JOIN_ITERATIONS})    | {:<20} | {:<20} |",
        format!("{:.1}ms ({})", dur_tapir_join.as_secs_f64() * 1000.0, format_ops(JOIN_ITERATIONS, dur_tapir_join.as_secs_f64())),
        format!("{:.1}ms ({})", dur_sqlite_join.as_secs_f64() * 1000.0, format_ops(JOIN_ITERATIONS, dur_sqlite_join.as_secs_f64()))
    );

    println!(
        "| AI Vector Search (128-D KNN) | {:<20} | {:<20} |",
        format!("{:.1}ms ({})", dur_tapir_vec.as_secs_f64() * 1000.0, format_ops(VEC_QUERIES, dur_tapir_vec.as_secs_f64())),
        "N/A (Requires ext/C)"
    );

    println!("===============================================================================");
    println!("Benchmark completed cleanly with zero errors.\n");
}
