use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};
use tapirus::pager::Wal;
use tapirus::{Config, Connection, Result, Value};

#[test]
fn recovery_discards_uncommitted_tail_before_read_append_and_checkpoint() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let wal_path = dir.path().join("recovery.wal");
    {
        let mut wal = Wal::open(&wal_path, 4096)?;
        wal.write_frame(1, &vec![11; 4096], true, 1)?;
        wal.write_frame(1, &vec![22; 4096], false, 1)?;
        wal.sync()?;
    }
    // A torn frame must also be removed physically before subsequent appends.
    OpenOptions::new()
        .append(true)
        .open(&wal_path)?
        .write_all(&[5; 137])?;
    let mut wal = Wal::open(&wal_path, 4096)?;
    assert_eq!(wal.frame_count(), 1);
    assert_eq!(wal.read_page(1).unwrap()[0], 11);
    let mut base = Some(
        OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(dir.path().join("base"))?,
    );
    let mut cache = HashMap::new();
    wal.recover(&mut base, &mut cache)?;
    wal.checkpoint(&mut base, &mut cache)?;
    let mut byte = [0];
    base.as_mut().unwrap().seek(SeekFrom::Start(0))?;
    base.as_mut().unwrap().read_exact(&mut byte)?;
    assert_eq!(byte[0], 11);
    wal.write_frame(1, &vec![33; 4096], true, 1)?;
    drop(wal);
    assert_eq!(Wal::open(wal_path, 4096)?.read_page(1).unwrap()[0], 33);
    Ok(())
}

#[test]
fn rollback_restores_overflow_allocator_and_reusable_nested_savepoints() -> Result<()> {
    let db = Connection::open_in_memory()?;
    db.execute("CREATE TABLE payloads (id INTEGER PRIMARY KEY, body TEXT);")?;
    let insert = |id, text: String| {
        db.execute_with_params(
            "INSERT INTO payloads VALUES (?, ?);",
            &[Value::Integer(id), Value::Text(text)],
        )
    };
    insert(1, "X".repeat(10000))?;
    db.begin_transaction()?;
    db.execute("DELETE FROM payloads WHERE id = 1;")?;
    db.rollback()?;
    insert(2, "Y".repeat(10000))?;
    assert_eq!(db.query("SELECT * FROM payloads;")?.len(), 2);
    db.execute("BEGIN;")?;
    db.execute("SAVEPOINT outer_sp;")?;
    db.execute("SAVEPOINT inner_sp;")?;
    db.execute("DELETE FROM payloads WHERE id = 1;")?;
    db.execute("RELEASE inner_sp;")?;
    db.execute("ROLLBACK TO outer_sp;")?;
    assert_eq!(db.query("SELECT * FROM payloads;")?.len(), 2);
    db.execute("DELETE FROM payloads WHERE id = 2;")?;
    db.execute("ROLLBACK TO outer_sp;")?;
    db.execute("RELEASE outer_sp;")?;
    db.execute("COMMIT;")?;
    insert(3, "Z".repeat(10000))?;
    assert_eq!(db.query("SELECT * FROM payloads;")?.len(), 3);
    Ok(())
}

#[test]
fn prepared_and_plain_inserts_share_constraints() -> Result<()> {
    let db = Connection::open_in_memory()?;
    db.execute("CREATE TABLE typed (id INTEGER PRIMARY KEY, body TEXT NOT NULL, amount INTEGER, embedding VECTOR(2));")?;
    let statement = db.prepare("INSERT INTO typed VALUES (?, ?, ?, ?);")?;
    for values in [
        vec![
            Value::Integer(1),
            Value::Null,
            Value::Integer(3),
            Value::Vector(vec![1., 0.]),
        ],
        vec![
            Value::Integer(2),
            Value::Text("ok".into()),
            Value::Text("invalid".into()),
            Value::Vector(vec![1., 0.]),
        ],
        vec![
            Value::Integer(3),
            Value::Text("ok".into()),
            Value::Integer(3),
            Value::Vector(vec![1.]),
        ],
    ] {
        assert!(statement.execute(&values).is_err());
    }
    assert!(db.query("SELECT * FROM typed;")?.is_empty());
    Ok(())
}

#[test]
fn multirow_insert_is_complete_atomic_and_returns_all_rows() -> Result<()> {
    let db = Connection::open_in_memory()?;
    db.execute("CREATE TABLE batch (id INTEGER PRIMARY KEY, body TEXT NOT NULL);")?;
    let rows = db.query("INSERT INTO batch VALUES (1, 'one'), (2, 'two') RETURNING id;")?;
    assert_eq!(rows.len(), 2);
    assert!(
        db.execute("INSERT INTO batch VALUES (3, 'three'), (1, 'duplicate');")
            .is_err()
    );
    assert_eq!(db.query("SELECT * FROM batch;")?.len(), 2);
    db.execute("BEGIN;")?;
    assert!(
        db.execute("INSERT INTO batch VALUES (4, 'four'), (5, NULL);")
            .is_err()
    );
    db.execute("INSERT INTO batch VALUES (6, 'six');")?;
    db.execute("COMMIT;")?;
    assert_eq!(db.query("SELECT * FROM batch;")?.len(), 3);
    assert!(db.query("SELECT * FROM batch; DELETE FROM batch;").is_err());
    assert!(
        db.execute("INSERT INTO batch VALUES (7, 'seven') GARBAGE;")
            .is_err()
    );
    Ok(())
}

#[test]
fn cypher_create_survives_reopen_and_rolls_back() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("graph.tapir");
    {
        let db = Connection::open(&path)?;
        db.execute("CREATE (a:Person {name: 'Alex'});")?;
        db.execute("BEGIN;")?;
        db.execute("CREATE (b:Person {name: 'Sam'});")?;
        db.execute("ROLLBACK;")?;
        assert_eq!(db.query("MATCH (a:Person) RETURN a.name;")?.len(), 1);
    }
    let db = Connection::open(path)?;
    assert_eq!(db.query("MATCH (a:Person) RETURN a.name;")?.len(), 1);
    Ok(())
}

#[test]
#[ignore = "subprocess helper used by vector_snapshot_recovers_after_abrupt_exit"]
fn vector_crash_writer() -> Result<()> {
    let path = std::env::var("TAPIRUS_REGRESSION_DB").expect("child database path");
    let db = Connection::open(path)?;
    db.execute("CREATE TABLE embeddings (id INTEGER PRIMARY KEY, embedding VECTOR(2));")?;
    db.execute("INSERT INTO embeddings VALUES (1, [1.0, 0.0]);")?;
    db.checkpoint()?;
    db.execute("INSERT INTO embeddings VALUES (2, [0.0, 1.0]);")?;
    std::process::exit(0)
}

#[test]
fn vector_snapshot_recovers_after_abrupt_exit() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("vectors.tapir");
    let child = std::process::Command::new(std::env::current_exe()?)
        .args(["--exact", "vector_crash_writer", "--ignored"])
        .env("TAPIRUS_REGRESSION_DB", &path)
        .status()?;
    assert!(child.success());
    let db = Connection::open(path)?;
    assert_eq!(db.query("SELECT * FROM embeddings;")?.len(), 2);
    assert_eq!(db.executor().read().vector_indexes()["embeddings"].len(), 2);
    Ok(())
}

#[test]
fn snapshots_reject_active_transactions_and_replicas_persist_with_sequence_checks() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let source = Connection::open_with_config(
        dir.path().join("primary.tapir"),
        Config {
            page_cache_capacity: 4,
            ..Config::default()
        },
    )?;
    source.execute("CREATE TABLE records (id INTEGER PRIMARY KEY, body TEXT);")?;
    for id in 1..=100 {
        source.execute_with_params(
            "INSERT INTO records VALUES (?, ?);",
            &[Value::Integer(id), Value::Text("record".repeat(50))],
        )?;
    }
    source.begin_transaction()?;
    source.execute("INSERT INTO records VALUES (101, 'private');")?;
    assert!(source.export_replication_chunk().is_err());
    assert!(source.export_bytes().is_err());
    assert!(source.backup(dir.path().join("invalid.tapir")).is_err());
    source.rollback()?;
    let snapshot = source.export_replication_chunk()?;
    assert_eq!(snapshot.frames.len(), snapshot.total_pages as usize);
    let path = dir.path().join("replica.tapir");
    {
        let replica = Connection::open(&path)?;
        replica.apply_replication_chunk(&snapshot)?;
        assert_eq!(replica.query("SELECT * FROM records;")?.len(), 100);
        assert!(replica.apply_replication_chunk(&snapshot).is_err());
    }
    let replica = Connection::open(path)?;
    assert_eq!(replica.query("SELECT * FROM records;")?.len(), 100);
    assert!(replica.apply_replication_chunk(&snapshot).is_err());
    Ok(())
}

#[test]
fn cdc_publishes_only_committed_changes_after_releasing_database_locks() -> Result<()> {
    use std::sync::mpsc;
    use std::time::Duration;

    let db = Connection::open_in_memory()?;
    db.execute("CREATE TABLE cdc_rows (id INTEGER PRIMARY KEY, body TEXT);")?;
    let observer = db.clone();
    let (tx, rx) = mpsc::channel();
    db.subscribe("cdc_rows", move |event| {
        // Reenter the database from the callback. This must not run under pager/executor locks.
        let rows = observer
            .query("SELECT * FROM cdc_rows;")
            .map(|rows| rows.len());
        let _ = tx.send((event.row_id, rows));
    });

    db.execute("BEGIN;")?;
    db.execute("INSERT INTO cdc_rows VALUES (1, 'kept');")?;
    assert!(
        rx.try_recv().is_err(),
        "uncommitted changes must not be published"
    );
    db.execute("SAVEPOINT nested;")?;
    db.execute("INSERT INTO cdc_rows VALUES (2, 'rolled back');")?;
    db.execute("ROLLBACK TO nested;")?;
    db.execute("INSERT INTO cdc_rows VALUES (3, 'kept');")?;
    db.execute("COMMIT;")?;

    let (first_row_id, first_visible_rows) = rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(first_row_id, 1);
    assert_eq!(first_visible_rows?, 2);
    let (second_row_id, second_visible_rows) = rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(second_row_id, 3);
    assert_eq!(second_visible_rows?, 2);
    assert!(
        rx.try_recv().is_err(),
        "rolled-back savepoint changes must not be published"
    );

    db.execute("BEGIN;")?;
    db.execute("INSERT INTO cdc_rows VALUES (4, 'discarded');")?;
    db.execute("ROLLBACK;")?;
    assert!(
        rx.try_recv().is_err(),
        "rolled-back transactions must not be published"
    );
    Ok(())
}
