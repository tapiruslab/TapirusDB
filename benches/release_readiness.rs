//! Reproducible small-workload measurements with correctness and reopen checks.
#![forbid(unsafe_code)]

use serde_json::{Value as Json, json};
use sha2::{Digest, Sha256};
use std::{hint::black_box, process::Command, time::Instant};
use tapirus::{Connection, DistanceMetric, HnswIndex, Value, VectorIndexEngine};

fn timings(mut operation: impl FnMut(usize), count: usize) -> Json {
    let mut samples = Vec::with_capacity(count);
    for i in 0..count {
        let start = Instant::now();
        operation(i);
        samples.push(start.elapsed().as_secs_f64() * 1_000_000.0);
    }
    samples.sort_by(f64::total_cmp);
    let percentile = |fraction: f64| samples[((count - 1) as f64 * fraction).ceil() as usize];
    json!({"samples": count, "unit": "microseconds", "p50": percentile(0.5), "p95": percentile(0.95), "p99": percentile(0.99), "min": samples[0], "max": samples[count-1]})
}

fn output(program: &str, arguments: &[&str]) -> String {
    Command::new(program)
        .args(arguments)
        .output()
        .ok()
        .filter(|result| result.status.success())
        .map(|result| String::from_utf8_lossy(&result.stdout).trim().to_string())
        .unwrap_or_else(|| "unavailable".into())
}

fn main() {
    if cfg!(debug_assertions) {
        println!(
            "Release measurements skipped in debug/test. Run cargo bench --bench release_readiness."
        );
        return;
    }
    const ROWS: usize = 1_000;
    let directory = tempfile::tempdir().expect("benchmark directory");
    let tapir_file = directory.path().join("measured.tapir");
    let sqlite_file = directory.path().join("measured.sqlite");
    let tapir = Connection::open(&tapir_file).expect("TapirusDB open");
    let sqlite = rusqlite::Connection::open(&sqlite_file).expect("SQLite open");
    sqlite.pragma_update(None, "journal_mode", "WAL").unwrap();
    sqlite.pragma_update(None, "synchronous", "FULL").unwrap();
    let schema = "CREATE TABLE items (id INTEGER PRIMARY KEY, body TEXT NOT NULL);";
    tapir.execute(schema).unwrap();
    sqlite.execute(schema, []).unwrap();
    let start = Instant::now();
    tapir.execute("BEGIN;").unwrap();
    for i in 0..ROWS {
        tapir
            .execute_with_params(
                "INSERT INTO items VALUES (?, ?);",
                &[Value::Integer(i as i64), Value::Text(format!("source {i}"))],
            )
            .unwrap();
    }
    tapir.execute("COMMIT;").unwrap();
    let tapir_batch_ms = start.elapsed().as_secs_f64() * 1000.0;
    let start = Instant::now();
    sqlite.execute("BEGIN;", []).unwrap();
    for i in 0..ROWS {
        sqlite
            .execute(
                "INSERT INTO items VALUES (?, ?);",
                rusqlite::params![i as i64, format!("source {i}")],
            )
            .unwrap();
    }
    sqlite.execute("COMMIT;", []).unwrap();
    let sqlite_batch_ms = start.elapsed().as_secs_f64() * 1000.0;
    // Both APIs receive the same SQL/values on each operation. Parsing is included.
    for _ in 0..20 {
        black_box(tapir.query("SELECT * FROM items WHERE id = 0;").unwrap());
        black_box(
            sqlite
                .query_row("SELECT body FROM items WHERE id = 0;", [], |row| {
                    row.get::<_, String>(0)
                })
                .unwrap(),
        );
    }
    let tapir_read = timings(
        |i| {
            let rows = tapir
                .query_with_params(
                    "SELECT body FROM items WHERE id = ?;",
                    &[Value::Integer((i % ROWS) as i64)],
                )
                .unwrap();
            assert_eq!(
                rows[0].get::<String>("body").unwrap(),
                format!("source {}", i % ROWS)
            );
            black_box(rows);
        },
        1_000,
    );
    let sqlite_read = timings(
        |i| {
            let value: String = sqlite
                .query_row(
                    "SELECT body FROM items WHERE id = ?;",
                    [(i % ROWS) as i64],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(value, format!("source {}", i % ROWS));
            black_box(value);
        },
        1_000,
    );
    let tapir_write = timings(
        |i| {
            tapir
                .execute_with_params(
                    "INSERT INTO items VALUES (?, ?);",
                    &[
                        Value::Integer((ROWS + i) as i64),
                        Value::Text("durable source".into()),
                    ],
                )
                .unwrap();
        },
        100,
    );
    let sqlite_write = timings(
        |i| {
            sqlite
                .execute(
                    "INSERT INTO items VALUES (?, ?);",
                    rusqlite::params![(ROWS + i) as i64, "durable source"],
                )
                .unwrap();
        },
        100,
    );
    tapir.close().unwrap();
    drop(tapir);
    sqlite.close().unwrap();
    let reopened = Connection::open(&tapir_file).unwrap();
    assert_eq!(
        reopened
            .query("SELECT COUNT(*) AS count FROM items;")
            .unwrap()[0]
            .get::<i64>("count")
            .unwrap(),
        1_100
    );
    let sqlite_reopened = rusqlite::Connection::open(&sqlite_file).unwrap();
    assert_eq!(
        sqlite_reopened
            .query_row("SELECT COUNT(*) FROM items;", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        1_100
    );

    // Seeded, independent vector components; flat exact cosine is the ground truth.
    let mut state = 0x1a2b3c4du32;
    let mut random = || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        (state as f64 / u32::MAX as f64 * 2.0 - 1.0) as f32
    };
    let vectors: Vec<Vec<f32>> = (0..1_000)
        .map(|_| (0..32).map(|_| random()).collect())
        .collect();
    let mut index = HnswIndex::with_seed(32, DistanceMetric::Cosine, 20_261_016);
    for (i, vector) in vectors.iter().enumerate() {
        index.insert_vector(i as u64 + 1, vector).unwrap();
    }
    let queries: Vec<Vec<f32>> = (0..100)
        .map(|i| {
            vectors[i * 7]
                .iter()
                .map(|value| value + random() * 0.02)
                .collect()
        })
        .collect();
    let expected: Vec<Vec<u64>> = queries
        .iter()
        .map(|query| {
            let mut exact: Vec<_> = vectors
                .iter()
                .enumerate()
                .map(|(i, vector)| {
                    (
                        i as u64 + 1,
                        1.0 - tapirus::memory::cosine_similarity(query, vector),
                    )
                })
                .collect();
            exact.sort_by(|a, b| a.1.total_cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
            exact.into_iter().take(10).map(|(id, _)| id).collect()
        })
        .collect();
    let mut hits = 0;
    let vector_latency = timings(
        |i| {
            let result = index
                .search_knn(black_box(&queries[i]), 10, DistanceMetric::Cosine)
                .unwrap();
            assert_eq!(result.len(), 10);
            hits += result
                .iter()
                .filter(|(id, _)| expected[i].contains(id))
                .count();
            black_box(result);
        },
        queries.len(),
    );
    let binary = std::env::current_exe().unwrap();
    let report = json!({
        "scope": "single-host small-workload measurements; query parsing and correctness assertions included; warm point reads; vector index in memory; no embedding-model cost",
        "source_head": output("git", &["rev-parse", "HEAD"]),
        "source_dirty": !output("git", &["status", "--porcelain"]).is_empty(),
        "binary_sha256": format!("{:x}", Sha256::digest(std::fs::read(binary).unwrap())),
        "cargo_lock_sha256": format!("{:x}", Sha256::digest(std::fs::read("Cargo.lock").unwrap())),
        "rustc": output("rustc", &["--version"]), "os": std::env::consts::OS, "arch": std::env::consts::ARCH,
        "cpu": std::env::var("PROCESSOR_IDENTIFIER").unwrap_or_else(|_| "record host CPU separately".into()),
        "profile": "release: opt-level 3, fat LTO, one codegen unit; default Cargo features",
        "storage": "temporary files on host filesystem; TapirusDB WAL commit and SQLite WAL synchronous=FULL",
        "sqlite_version": rusqlite::version(), "rows": ROWS,
        "tapirus": {"batch_1000_with_commit_ms": tapir_batch_ms, "point_read": tapir_read, "autocommit_insert": tapir_write},
        "sqlite": {"batch_1000_with_commit_ms": sqlite_batch_ms, "point_read": sqlite_read, "autocommit_insert": sqlite_write},
        "hnsw": {"vectors": 1000, "dimensions": 32, "queries": queries.len(), "k": 10, "seed": 20_261_016, "recall_at_10": hits as f64 / (queries.len() * 10) as f64, "latency": vector_latency},
        "correctness": {"point_read_values_checked": true, "both_databases_reopened_with_1100_rows": true}
    });
    std::fs::write(
        "target/release-readiness-results.json",
        serde_json::to_string_pretty(&report).unwrap(),
    )
    .unwrap();
    println!("{}", serde_json::to_string_pretty(&report).unwrap());
    let recall = hits as f64 / (queries.len() * 10) as f64;
    assert!(
        recall >= 0.90,
        "HNSW recall@10 {recall} is below the fixture acceptance threshold"
    );
}
