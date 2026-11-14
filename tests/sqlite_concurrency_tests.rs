use std::time::Duration;
use tapirus::{Config, Connection, Error, SessionDatabase, Value};
use tempfile::tempdir;

#[test]
fn test_mvcc_snapshot_reader_non_blocking_and_repeatable_read() {
    let db = SessionDatabase::open_in_memory().unwrap();
    let writer = db.session().unwrap();
    writer
        .execute("CREATE TABLE acct (id INTEGER PRIMARY KEY, bal INTEGER);")
        .unwrap();
    writer.execute("INSERT INTO acct VALUES (1, 100);").unwrap();

    // Begin uncommitted write transaction
    writer.begin().unwrap();
    writer
        .execute("UPDATE acct SET bal = 999 WHERE id = 1;")
        .unwrap();
    writer.execute("INSERT INTO acct VALUES (2, 200);").unwrap();

    // Existing strict session() behavior still returns Busy when another session owns a transaction
    let blocked = std::thread::scope(|s| {
        s.spawn(|| match db.session() {
            Ok(other) => other
                .execute("INSERT INTO acct VALUES (3, 300);")
                .unwrap_err(),
            Err(e) => e,
        })
        .join()
        .unwrap()
    });
    assert!(matches!(blocked, Error::Busy(_)));

    // Snapshot reader returns immediately without blocking and sees pre-transaction committed state
    let snap_mid = std::thread::scope(|s| {
        s.spawn(|| {
            let snap = db.snapshot_reader().unwrap();
            let rows = snap.query("SELECT id, bal FROM acct ORDER BY id;").unwrap();
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].values(), &[Value::Integer(1), Value::Integer(100)]);
            snap
        })
        .join()
        .unwrap()
    });

    // Commit writer transaction
    writer.commit().unwrap();

    // Repeatable-read: earlier snapshot still sees its captured epoch
    let old_rows = snap_mid
        .query("SELECT id, bal FROM acct ORDER BY id;")
        .unwrap();
    assert_eq!(old_rows.len(), 1);
    assert_eq!(
        old_rows[0].values(),
        &[Value::Integer(1), Value::Integer(100)]
    );

    // New snapshot advances epoch and sees committed writes
    let snap_after = db.snapshot_reader().unwrap();
    assert!(snap_after.epoch() > snap_mid.epoch());
    let new_rows = snap_after
        .query("SELECT id, bal FROM acct ORDER BY id;")
        .unwrap();
    assert_eq!(new_rows.len(), 2);
    assert_eq!(
        new_rows[0].values(),
        &[Value::Integer(1), Value::Integer(999)]
    );
    assert_eq!(
        new_rows[1].values(),
        &[Value::Integer(2), Value::Integer(200)]
    );
}

#[test]
fn test_read_only_connection_concurrent_with_writer() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("ro_concurrent.tapir");

    let writer = Connection::open(&db_path).unwrap();
    writer
        .execute("CREATE TABLE items (id INTEGER PRIMARY KEY, name TEXT);")
        .unwrap();
    writer
        .execute("INSERT INTO items VALUES (1, 'initial');")
        .unwrap();

    // Read-only connection succeeds even while exclusive writer is open (no .tapir-lock contention)
    let ro = Connection::open_read_only(&db_path).unwrap();
    assert!(ro.is_read_only());
    let rows = ro.query("SELECT id, name FROM items ORDER BY id;").unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].values()[1], Value::Text("initial".into()));

    // Mutations must be rejected cleanly on read-only connection
    assert!(ro.execute("INSERT INTO items VALUES (2, 'nope');").is_err());

    // Uncommitted write in writer is invisible to read-only connection
    writer.begin_transaction().unwrap();
    writer
        .execute("INSERT INTO items VALUES (2, 'committed_later');")
        .unwrap();
    let mid_rows = ro.query("SELECT id FROM items ORDER BY id;").unwrap();
    assert_eq!(mid_rows.len(), 1);

    // After commit, read-only reader sees committed WAL frame automatically
    writer.commit().unwrap();
    let post_rows = ro.query("SELECT id, name FROM items ORDER BY id;").unwrap();
    assert_eq!(post_rows.len(), 2);
    assert_eq!(
        post_rows[1].values(),
        &[Value::Integer(2), Value::Text("committed_later".into())]
    );
}

#[test]
fn test_concurrent_connections_wal_coordination_and_busy_timeout() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("multi_concurrent.tapir");

    let c1 = Connection::open_concurrent(&db_path).unwrap();
    c1.execute("CREATE TABLE log (id INTEGER PRIMARY KEY, worker TEXT);")
        .unwrap();

    // Open second concurrent handle on same DB file (would return Busy under legacy Connection::open)
    let c2 = Connection::open_concurrent(&db_path).unwrap();

    // Start explicit write transaction in c1 -> c1 holds .tapir-lock until commit
    c1.begin_transaction().unwrap();
    c1.execute("INSERT INTO log VALUES (1, 'c1');").unwrap();

    // c2 with zero busy_timeout returns Error::Busy immediately on concurrent write attempt
    c2.set_busy_timeout(Duration::ZERO);
    let busy_err = c2.execute("INSERT INTO log VALUES (2, 'c2');").unwrap_err();
    assert!(matches!(busy_err, Error::Busy(_)));

    // c2 can STILL read committed state concurrently!
    let pre_rows = c2.query("SELECT COUNT(*) FROM log;").unwrap();
    assert_eq!(pre_rows[0].values()[0], Value::Integer(0));

    // Commit c1 in background while c2 waits with busy_timeout
    c2.set_busy_timeout(Duration::from_secs(3));
    let c1_clone = c1;
    let t = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(80));
        c1_clone.commit().unwrap();
    });

    // c2 waits for c1 to release .tapir-lock, refreshes WAL, and inserts safely without overwriting c1!
    c2.execute("INSERT INTO log VALUES (2, 'c2');").unwrap();
    t.join().unwrap();

    let final_rows = c2.query("SELECT id, worker FROM log ORDER BY id;").unwrap();
    assert_eq!(final_rows.len(), 2);
    assert_eq!(
        final_rows[0].values(),
        &[Value::Integer(1), Value::Text("c1".into())]
    );
    assert_eq!(
        final_rows[1].values(),
        &[Value::Integer(2), Value::Text("c2".into())]
    );

    // Parallel multi-handle writers stress test with no lost updates
    let p = db_path.clone();
    let mut handles = Vec::new();
    for w in 0..2 {
        let p_w = p.clone();
        handles.push(std::thread::spawn(move || {
            let conn = Connection::open_concurrent_with_config(
                &p_w,
                Config {
                    concurrent_access: true,
                    busy_timeout: Duration::from_secs(5),
                    ..Default::default()
                },
            )
            .unwrap();
            for i in 0..10 {
                let id = 100 + w * 100 + i;
                conn.execute(&format!("INSERT INTO log VALUES ({id}, 'w{w}');"))
                    .unwrap();
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }

    let verifier = Connection::open_read_only(&db_path).unwrap();
    let count_rows = verifier.query("SELECT COUNT(*) FROM log;").unwrap();
    assert_eq!(count_rows[0].values()[0], Value::Integer(22));
}
