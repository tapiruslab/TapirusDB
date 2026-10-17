#![cfg(feature = "fault-injection")]
use tapirus::pager::fault::{IoFailure, IoFaultPlan, IoPoint};
use tapirus::{Connection, MemoryDocument};

fn plan(point: IoPoint, after: usize, failure: IoFailure) -> IoFaultPlan {
    IoFaultPlan {
        point,
        after,
        failure,
        persistent: false,
    }
}

#[test]
fn partial_frame_failures_are_truncated_before_the_next_successful_commit() {
    for after in 0..8 {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("partial.tapir");
        let db = Connection::open(&file).unwrap();
        db.execute("CREATE TABLE samples (id INTEGER PRIMARY KEY, payload TEXT)")
            .unwrap();
        db.execute("INSERT INTO samples VALUES (1, 'committed')")
            .unwrap();
        db.checkpoint().unwrap();
        db.set_io_fault(Some(plan(
            IoPoint::WalWrite,
            after,
            IoFailure::PartialWrite { bytes: 13 },
        )))
        .unwrap();
        let text = "uncommitted-".repeat(1800);
        let failed = db
            .execute_with_params(
                "INSERT INTO samples VALUES (?, ?)",
                &[tapirus::Value::Integer(2), tapirus::Value::Text(text)],
            )
            .is_err();
        db.set_io_fault(None).unwrap();
        assert!(failed, "fixture must reach injected write #{after}");
        assert!(!db.storage_health().reopen_required);
        db.execute("INSERT INTO samples VALUES (3, 'after failure')")
            .unwrap();
        db.close().unwrap();
        drop(db);
        let db = Connection::open(&file).unwrap();
        let rows = db.query("SELECT id FROM samples ORDER BY id").unwrap();
        assert_eq!(
            rows.iter()
                .map(|r| r.get::<i64>("id").unwrap())
                .collect::<Vec<_>>(),
            vec![1, 3]
        );
        assert!(db.check_integrity().unwrap().is_ok());
    }
}

#[test]
fn disk_full_flush_and_sync_failures_preserve_atomic_sql_and_memory() {
    for point in [IoPoint::WalWrite, IoPoint::WalFlush, IoPoint::WalSync] {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("failure.tapir");
        let db = Connection::open(&file).unwrap();
        db.execute("CREATE TABLE samples (id INTEGER)").unwrap();
        db.execute("BEGIN").unwrap();
        db.execute("INSERT INTO samples VALUES (1)").unwrap();
        db.store_memory(&MemoryDocument {
            content: "must rollback with SQL".into(),
            ..Default::default()
        })
        .unwrap();
        db.set_io_fault(Some(plan(point, 0, IoFailure::DiskFull)))
            .unwrap();
        assert!(db.execute("COMMIT").is_err());
        db.set_io_fault(None).unwrap();
        if db.is_in_transaction() {
            db.execute("ROLLBACK").unwrap();
        }
        assert!(db.query("SELECT * FROM samples").unwrap().is_empty());
        assert_eq!(db.memory_count(), 0);
        db.close().unwrap();
        drop(db);
        let db = Connection::open(&file).unwrap();
        assert!(db.query("SELECT * FROM samples").unwrap().is_empty());
        assert_eq!(db.memory_count(), 0);
    }
}

#[test]
fn torn_checkpoint_header_is_recovered_from_the_committed_wal() {
    for prefix in [0, 1, 13, 27, 45, 99, 2000] {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("checkpoint.tapir");
        let db = Connection::open(&file).unwrap();
        db.execute("CREATE TABLE samples (id INTEGER)").unwrap();
        db.checkpoint().unwrap();
        db.execute("INSERT INTO samples VALUES (42)").unwrap();
        db.set_io_fault(Some(plan(
            IoPoint::CheckpointWrite,
            0,
            IoFailure::PartialWrite { bytes: prefix },
        )))
        .unwrap();
        assert!(db.checkpoint().is_err());
        assert!(db.storage_health().checkpoint_error.is_some());
        drop(db); // Do not retry checkpoint: leave its actual torn base image.
        let db = Connection::open(&file).unwrap();
        assert_eq!(
            db.query("SELECT id FROM samples").unwrap()[0]
                .get::<i64>("id")
                .unwrap(),
            42
        );
        assert!(db.check_integrity().unwrap().is_ok());
    }
}

#[test]
fn wal_reset_failures_require_reopen_and_preserve_committed_data() {
    for (point, failure) in [
        (IoPoint::WalTruncate, IoFailure::DiskFull),
        (IoPoint::WalReset, IoFailure::PartialWrite { bytes: 13 }),
        (IoPoint::WalSync, IoFailure::DiskFull),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("reset.tapir");
        let db = Connection::open(&file).unwrap();
        db.execute("CREATE TABLE samples (id INTEGER)").unwrap();
        db.execute("INSERT INTO samples VALUES (7)").unwrap();
        db.set_io_fault(Some(plan(point, 0, failure))).unwrap();
        assert!(db.checkpoint().is_err());
        assert!(db.storage_health().reopen_required);
        assert!(db.execute("INSERT INTO samples VALUES (8)").is_err());
        drop(db);
        let db = Connection::open(&file).unwrap();
        assert_eq!(db.query("SELECT id FROM samples").unwrap().len(), 1);
        assert!(db.check_integrity().unwrap().is_ok());
    }
}

#[test]
fn backup_failure_preserves_existing_backup_and_source_with_any_extension() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source.db");
    let backup = directory.path().join("backup.tapir");
    let db = Connection::open(&source).unwrap();
    db.execute("CREATE TABLE samples (id INTEGER)").unwrap();
    db.execute("INSERT INTO samples VALUES (1)").unwrap();
    db.backup(&backup).unwrap();
    let previous = std::fs::read(&backup).unwrap();
    db.execute("INSERT INTO samples VALUES (2)").unwrap();
    for failure in [
        plan(
            IoPoint::BackupWrite,
            0,
            IoFailure::PartialWrite { bytes: 27 },
        ),
        plan(IoPoint::BackupSync, 0, IoFailure::DiskFull),
    ] {
        db.set_io_fault(Some(failure)).unwrap();
        assert!(db.backup(&backup).is_err());
        assert_eq!(std::fs::read(&backup).unwrap(), previous);
        assert_eq!(db.query("SELECT * FROM samples").unwrap().len(), 2);
    }
    db.set_io_fault(None).unwrap();
    assert!(db.backup(&source).is_err());
    db.backup(&backup).unwrap();
    let restored = Connection::open(&backup).unwrap();
    assert_eq!(restored.query("SELECT * FROM samples").unwrap().len(), 2);
}

#[test]
fn restore_refuses_live_targets_and_validates_before_replacement() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source.tapir");
    let target = directory.path().join("target.db");
    let db = Connection::open(&source).unwrap();
    db.execute("CREATE TABLE samples (id INTEGER)").unwrap();
    db.execute("INSERT INTO samples VALUES (1)").unwrap();
    db.close().unwrap();
    drop(db);
    let live = Connection::open(&target).unwrap();
    live.execute("CREATE TABLE old_data (id INTEGER)").unwrap();
    assert!(Connection::restore_backup(&source, &target, Default::default(), true).is_err());
    assert_eq!(
        live.tables()
            .iter()
            .map(|table| table.name.as_str())
            .collect::<Vec<_>>(),
        vec!["old_data"]
    );
    live.close().unwrap();
    drop(live);
    assert!(Connection::restore_backup(&source, &target, Default::default(), false).is_err());
    Connection::restore_backup(&source, &target, Default::default(), true).unwrap();
    let restored = Connection::open(&target).unwrap();
    assert_eq!(restored.query("SELECT * FROM samples").unwrap().len(), 1);
}

#[test]
fn durable_commit_reports_deferred_checkpoint_failure_without_rejecting_the_commit() {
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("deferred.tapir");
    let db = Connection::open(&file).unwrap();
    db.execute("CREATE TABLE samples (id INTEGER PRIMARY KEY, body TEXT)")
        .unwrap();
    db.execute("BEGIN").unwrap();
    let payload = "checkpoint data ".repeat(900);
    for id in 0..350 {
        db.execute_with_params(
            "INSERT INTO samples VALUES (?, ?)",
            &[
                tapirus::Value::Integer(id),
                tapirus::Value::Text(payload.clone()),
            ],
        )
        .unwrap();
    }
    db.set_io_fault(Some(plan(
        IoPoint::CheckpointWrite,
        0,
        IoFailure::PartialWrite { bytes: 11 },
    )))
    .unwrap();
    db.commit().unwrap();
    assert!(db.storage_health().checkpoint_error.is_some());
    assert!(!db.storage_health().reopen_required);
    assert_eq!(
        db.query("SELECT COUNT(*) AS total FROM samples").unwrap()[0]
            .get::<i64>("total")
            .unwrap(),
        350
    );
    db.set_io_fault(None).unwrap();
    db.checkpoint().unwrap();
    assert!(db.storage_health().checkpoint_error.is_none());
    db.close().unwrap();
    drop(db);
    assert_eq!(
        Connection::open(file)
            .unwrap()
            .query("SELECT * FROM samples")
            .unwrap()
            .len(),
        350
    );
}

#[test]
fn encrypted_torn_header_recovers_with_raw_key_and_wrong_key_is_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("encrypted.tapir");
    let config = tapirus::Config {
        encryption_key: Some([7; 32]),
        ..Default::default()
    };
    let db = Connection::open_with_config(&file, config.clone()).unwrap();
    db.execute("CREATE TABLE samples (id INTEGER)").unwrap();
    db.execute("INSERT INTO samples VALUES (9)").unwrap();
    db.set_io_fault(Some(plan(
        IoPoint::CheckpointWrite,
        0,
        IoFailure::PartialWrite { bytes: 17 },
    )))
    .unwrap();
    assert!(db.checkpoint().is_err());
    drop(db);
    assert!(
        Connection::open_with_config(
            &file,
            tapirus::Config {
                encryption_key: Some([8; 32]),
                ..Default::default()
            }
        )
        .is_err()
    );
    let reopened = Connection::open_with_config(&file, config).unwrap();
    assert_eq!(reopened.query("SELECT * FROM samples").unwrap().len(), 1);
    assert!(reopened.check_integrity().unwrap().is_ok());
}

#[test]
fn persistent_wal_reset_error_blocks_operations_until_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("persistent.tapir");
    let db = Connection::open(&file).unwrap();
    db.execute("CREATE TABLE samples (id INTEGER)").unwrap();
    db.execute("INSERT INTO samples VALUES (1)").unwrap();
    db.set_io_fault(Some(IoFaultPlan {
        point: IoPoint::WalReset,
        after: 0,
        failure: IoFailure::PartialWrite { bytes: 7 },
        persistent: true,
    }))
    .unwrap();
    assert!(db.checkpoint().is_err());
    assert!(db.storage_health().reopen_required);
    assert!(db.query("SELECT * FROM samples").is_err());
    assert!(db.execute("INSERT INTO samples VALUES (2)").is_err());
    drop(db);
    assert_eq!(
        Connection::open(file)
            .unwrap()
            .query("SELECT * FROM samples")
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn disabling_automatic_checkpoint_preserves_wal_until_explicit_maintenance() {
    let db = Connection::open_in_memory_with_config(tapirus::Config {
        auto_checkpoint: false,
        ..Default::default()
    })
    .unwrap();
    db.execute("CREATE TABLE samples (id INTEGER)").unwrap();
    for id in 0..600 {
        db.execute_with_params(
            "INSERT INTO samples VALUES (?)",
            &[tapirus::Value::Integer(id)],
        )
        .unwrap();
    }
    assert!(db.storage_health().wal_frames >= 1000);
    db.checkpoint().unwrap();
    assert_eq!(db.storage_health().wal_frames, 0);
}
