use std::sync::Arc;
use std::thread;
use tapirus::{Connection, Result};

#[test]
fn test_concurrent_multi_threaded_readers_and_writers() -> Result<()> {
    let temp_dir = std::env::temp_dir();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let db_path = temp_dir.join(format!("tapirus_concurrency_{nanos}.tapir"));

    // 1. Initialize schema and seed data
    {
        let conn = Connection::open(&db_path)?;
        conn.execute(
            "CREATE TABLE sensor_feed (
                id INTEGER PRIMARY KEY,
                sensor_id TEXT NOT NULL,
                reading REAL,
                vec VECTOR(4)
            );",
        )?;

        // Seed 10 rows
        for i in 1..=10 {
            let sql = format!(
                "INSERT INTO sensor_feed VALUES ({i}, 'sensor_{i}', {:.2}, [{:.1}, {:.1}, 0.0, 0.0]);",
                i as f64 * 1.5,
                i as f32 / 10.0,
                1.0 - (i as f32 / 10.0)
            );
            conn.execute(&sql)?;
        }
        conn.checkpoint()?;
    }

    // 2. Open shared Connection across multiple concurrent threads
    let conn = Arc::new(Connection::open(&db_path)?);
    let mut handles = Vec::new();

    // Spawn 4 concurrent reader threads
    for _reader_id in 0..4 {
        let db = Arc::clone(&conn);
        let handle = thread::spawn(move || {
            for round in 0..10 {
                // Read query with filter
                let rows = db
                    .query("SELECT id, sensor_id FROM sensor_feed WHERE id > 2;")
                    .expect("Concurrent read should succeed");
                assert!(rows.len() >= 8);

                // Vector search query
                let vec_rows = db
                    .query("SELECT id FROM sensor_feed VECTOR NEAR vec = [0.5, 0.5, 0.0, 0.0] TOP 2;")
                    .expect("Concurrent vector search should succeed");
                assert_eq!(vec_rows.len(), 2);

                // Small yield
                if round % 3 == 0 {
                    thread::yield_now();
                }
            }
        });
        handles.push(handle);
    }

    // Spawn 4 concurrent writer threads
    for writer_id in 0..4 {
        let db = Arc::clone(&conn);
        let handle = thread::spawn(move || {
            for round in 0..5 {
                let unique_id = 100 + (writer_id * 100) + round;
                let sql = format!(
                    "INSERT INTO sensor_feed VALUES ({unique_id}, 'sensor_w_{writer_id}', 99.9, [0.1, 0.2, 0.3, 0.4]);"
                );
                let affected = db.execute(&sql).expect("Concurrent write should succeed");
                assert_eq!(affected, 1);

                if round % 2 == 0 {
                    thread::yield_now();
                }
            }
        });
        handles.push(handle);
    }

    // Wait for all 8 concurrent threads to complete cleanly
    for handle in handles {
        handle.join().expect("Thread should finish without panicking");
    }

    // 3. Verify final state and consistency
    let final_rows = conn.query("SELECT id FROM sensor_feed;")?;
    // Initial 10 rows + (4 writers * 5 writes = 20 rows) = 30 rows
    assert_eq!(final_rows.len(), 30, "All concurrent writes must be safely committed");

    let _ = std::fs::remove_file(&db_path);
    let _ = std::fs::remove_file(db_path.with_extension("tapir-wal"));
    Ok(())
}
