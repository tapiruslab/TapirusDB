use tapirus::{Connection, Value};
use tempfile::NamedTempFile;

/// SplitMix64 deterministic PRNG for 100% reproducible fuzzing
struct FuzzRng {
    state: u64,
}

impl FuzzRng {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^ (z >> 31)
    }

    fn gen_range(&mut self, low: usize, high: usize) -> usize {
        if low >= high {
            return low;
        }
        low + (self.next_u64() as usize % (high - low))
    }

    fn gen_string(&mut self, len: usize) -> String {
        const CHARS: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789_ ";
        let mut s = String::with_capacity(len);
        for _ in 0..len {
            let idx = self.gen_range(0, CHARS.len());
            s.push(CHARS[idx] as char);
        }
        s
    }
}

#[test]
fn test_fuzz_randomized_crud_transactions_and_schema_evolution() {
    let tmp = NamedTempFile::new().unwrap();
    let db_path = tmp.path().to_str().unwrap().to_string();

    let mut rng = FuzzRng::new(0xDEADBEEF_CAFE1337);

    // Initial connection
    let mut conn = Connection::open(&db_path).expect("Open database");

    conn.execute("CREATE TABLE accounts (id INTEGER PRIMARY KEY, name TEXT, balance REAL);")
        .expect("Initial table creation");

    let mut shadow_state: std::collections::HashMap<i64, (String, f64, Option<String>)> =
        std::collections::HashMap::new();

    let mut has_extra_column = false;
    let mut in_transaction = false;
    let mut uncommitted_ops: Vec<(i64, Option<(String, f64, Option<String>)>)> = Vec::new();

    const TOTAL_OPERATIONS: usize = 1_000;

    for op_idx in 0..TOTAL_OPERATIONS {
        let op_type = rng.gen_range(0, 10);
        println!("Op {op_idx}: type={op_type}, in_tx={in_transaction}");

        match op_type {
            // 0, 1, 2, 3: INSERT
            0..=3 => {
                let id = rng.gen_range(1, 500) as i64;
                let name = rng.gen_string(8);
                let balance = (rng.gen_range(100, 100_000) as f64) / 100.0;

                let sql = if has_extra_column {
                    let note = rng.gen_string(6);
                    format!(
                        "INSERT INTO accounts (id, name, balance, note) VALUES ({id}, '{name}', {balance}, '{note}');"
                    )
                } else {
                    format!(
                        "INSERT INTO accounts (id, name, balance) VALUES ({id}, '{name}', {balance});"
                    )
                };

                let res = conn.execute(&sql);
                // Determine whether id currently exists — considering committed state
                // AND the running uncommitted_ops (applied in order).
                // This mirrors exactly what the DB sees for PK collision checks.
                let exists_in_tx = {
                    let mut exists = shadow_state.contains_key(&id);
                    if in_transaction {
                        for (k, v) in &uncommitted_ops {
                            if *k == id {
                                exists = v.is_some(); // insert → exists, delete → not exists
                            }
                        }
                    }
                    exists
                };
                if !exists_in_tx {
                    if res.is_err() {
                        eprintln!("FAILED at op_idx {op_idx}: {sql} => {res:?}");
                    }
                    assert!(res.is_ok(), "Insert of unique ID {id} should succeed: {:?}", res);
                    let note = if has_extra_column { Some("note".into()) } else { None };
                    if in_transaction {
                        uncommitted_ops.push((id, Some((name, balance, note))));
                    } else {
                        shadow_state.insert(id, (name, balance, note));
                    }
                }
            }

            // 4, 5: UPDATE
            4..=5 => {
                // Build the set of IDs visible in the current transaction:
                // committed rows minus any in-tx deletions, plus in-tx insertions.
                let mut visible_ids: std::collections::HashSet<i64> =
                    shadow_state.keys().cloned().collect();
                if in_transaction {
                    for (k, v) in &uncommitted_ops {
                        if v.is_some() { visible_ids.insert(*k); }
                        else           { visible_ids.remove(k); }
                    }
                }
                if !visible_ids.is_empty() {
                    let keys: Vec<i64> = visible_ids.into_iter().collect();
                    let target_id = keys[rng.gen_range(0, keys.len())];
                    let new_balance = (rng.gen_range(100, 500_000) as f64) / 100.0;

                    let update_sql = format!(
                        "UPDATE accounts SET balance = {new_balance} WHERE id = {target_id};"
                    );
                    let res = conn.execute(&update_sql);
                    assert!(res.is_ok(), "Update should succeed");

                    if in_transaction {
                        // Record the update as an uncommitted op so it is only
                        // applied to shadow_state if the transaction commits.
                        // The row might be in shadow_state OR in uncommitted_ops.
                        let name_note = shadow_state.get(&target_id)
                            .map(|(n, _, note)| (n.clone(), note.clone()))
                            .or_else(|| {
                                uncommitted_ops.iter().rev()
                                    .find(|(k, v)| *k == target_id && v.is_some())
                                    .and_then(|(_, v)| v.as_ref())
                                    .map(|(n, _, note)| (n.clone(), note.clone()))
                            })
                            .unwrap_or_default();
                        uncommitted_ops.push((target_id, Some((name_note.0, new_balance, name_note.1))));
                    } else {
                        if let Some(entry) = shadow_state.get_mut(&target_id) {
                            entry.1 = new_balance;
                        }
                    }
                }
            }

            // 6: DELETE (Exercising defragmentation and GDPR zeroing)
            6 => {
                // Same merged visible-state logic as UPDATE:
                let mut visible_ids: std::collections::HashSet<i64> =
                    shadow_state.keys().cloned().collect();
                if in_transaction {
                    for (k, v) in &uncommitted_ops {
                        if v.is_some() { visible_ids.insert(*k); }
                        else           { visible_ids.remove(k); }
                    }
                }
                if !visible_ids.is_empty() {
                    let keys: Vec<i64> = visible_ids.into_iter().collect();
                    let target_id = keys[rng.gen_range(0, keys.len())];

                    let del_sql = format!("DELETE FROM accounts WHERE id = {target_id};");
                    let res = conn.execute(&del_sql);
                    assert!(res.is_ok(), "Delete should succeed");

                    if in_transaction {
                        uncommitted_ops.push((target_id, None));
                    } else {
                        shadow_state.remove(&target_id);
                    }
                }
            }

            // 7: ALTER TABLE ADD COLUMN (Schema Evolution)
            7 => {
                if !has_extra_column && op_idx > 100 {
                    let alter_res = conn.execute("ALTER TABLE accounts ADD COLUMN note TEXT;");
                    assert!(alter_res.is_ok(), "ALTER TABLE ADD COLUMN should succeed");
                    has_extra_column = true;
                }
            }

            // 8: Transaction lifecycle (BEGIN, COMMIT, ROLLBACK)
            8 => {
                if !in_transaction {
                    let _ = conn.begin_transaction();
                    in_transaction = true;
                    uncommitted_ops.clear();
                } else {
                    let should_commit = rng.gen_range(0, 2) == 1;
                    if should_commit {
                        println!("Op {op_idx}: COMMIT");
                        let _ = conn.commit();
                        for (k, v) in uncommitted_ops.drain(..) {
                            if let Some(val) = v {
                                shadow_state.insert(k, val);
                            } else {
                                shadow_state.remove(&k);
                            }
                        }
                    } else {
                        println!("Op {op_idx}: ROLLBACK");
                        let _ = conn.rollback();
                        uncommitted_ops.clear();
                    }
                    in_transaction = false;
                }
            }

            // 9: Periodic Checkpoint, Vacuum, and File Reopen
            _ => {
                if in_transaction {
                    let _ = conn.commit();
                    for (k, v) in uncommitted_ops.drain(..) {
                        if let Some(val) = v {
                            shadow_state.insert(k, val);
                        } else {
                            shadow_state.remove(&k);
                        }
                    }
                    in_transaction = false;
                }

                let _ = conn.checkpoint();

                // Reopen the database to test persistence and crash safety
                drop(conn);
                conn = Connection::open(&db_path).expect("Reopen database during fuzz test");

                // Invariant assertion: Verify row count equals shadow state
                let rows = conn.query("SELECT id FROM accounts;").expect("Verify rows");
                assert_eq!(
                    rows.len(),
                    shadow_state.len(),
                    "Row count mismatch at op_idx {op_idx}"
                );
            }
        }
    }

    // Final reconciliation
    if in_transaction {
        let _ = conn.commit();
        for (k, v) in uncommitted_ops.drain(..) {
            if let Some(val) = v {
                shadow_state.insert(k, val);
            } else {
                shadow_state.remove(&k);
            }
        }
    }

    let final_rows = conn.query("SELECT id FROM accounts;").expect("Final query");
    assert_eq!(
        final_rows.len(),
        shadow_state.len(),
        "Final database row count must exactly match shadow state"
    );
}

#[test]
fn test_fuzz_view_queries_and_droppings() {
    let tmp = NamedTempFile::new().unwrap();
    let conn = Connection::open(tmp.path()).expect("Open database");

    conn.execute("CREATE TABLE sensor_data (id INTEGER PRIMARY KEY, sensor_id TEXT, reading REAL);")
        .expect("Create sensor_data");

    for i in 1..=50 {
        let sid = if i % 2 == 0 { "TEMP_A" } else { "TEMP_B" };
        let reading = 20.0 + (i as f64 * 0.5);
        conn.execute(&format!(
            "INSERT INTO sensor_data (id, sensor_id, reading) VALUES ({i}, '{sid}', {reading});"
        )).unwrap();
    }

    // Create multiple views
    conn.execute("CREATE VIEW view_temp_a AS SELECT id, reading FROM sensor_data WHERE sensor_id = 'TEMP_A';").unwrap();
    conn.execute("CREATE VIEW view_temp_b AS SELECT id, reading FROM sensor_data WHERE sensor_id = 'TEMP_B';").unwrap();

    let rows_a = conn.query("SELECT * FROM view_temp_a;").unwrap();
    assert_eq!(rows_a.len(), 25);

    let rows_b = conn.query("SELECT * FROM view_temp_b;").unwrap();
    assert_eq!(rows_b.len(), 25);

    // Nested filter on view
    let high_a = conn.query("SELECT id, reading FROM view_temp_a WHERE reading > 30.0;").unwrap();
    for r in &high_a {
        if let Some(Value::Real(v)) = r.get_value("reading") {
            assert!(*v > 30.0);
        }
    }

    // Drop one view, verify other remains intact
    conn.execute("DROP VIEW view_temp_a;").unwrap();
    assert_eq!(conn.views(), vec!["view_temp_b"]);

    let rows_b_again = conn.query("SELECT * FROM view_temp_b;").unwrap();
    assert_eq!(rows_b_again.len(), 25);
}
