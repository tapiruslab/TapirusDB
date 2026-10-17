use std::process::Command;
use tapirus::{Connection, MemoryDocument, MemorySearch};

#[test]
fn mixed_sql_and_ai_memory_obey_savepoints_and_reopen() -> tapirus::Result<()> {
    let directory = tempfile::tempdir()?;
    let file = directory.path().join("mixed.tapir");
    let db = Connection::open(&file)?;
    db.execute("CREATE TABLE sources (id INTEGER PRIMARY KEY, title TEXT);")?;
    db.execute("BEGIN;")?;
    db.execute("INSERT INTO sources VALUES (1, 'manual');")?;
    let document = MemoryDocument {
        content: "Repair guide for the engine".into(),
        embedding: Some(vec![1.0, 0.0]),
        namespace: Some("manuals".into()),
        ..Default::default()
    };
    let committed_id = db.store_memory(&document)?;
    db.execute("SAVEPOINT draft;")?;
    let rolled_back_id = db.store_memory(&MemoryDocument {
        content: "Draft replacement".into(),
        ..document.clone()
    })?;
    assert!(db.memory_forget_checked(committed_id)?);
    db.query("ROLLBACK TO draft;")?;
    assert!(db.memory_get(committed_id).is_some());
    assert!(db.memory_get(rolled_back_id).is_none());
    db.prepare("COMMIT;")?.query(&[])?;
    db.execute("BEGIN;")?;
    db.memory_forget_checked(committed_id)?;
    db.prepare("ROLLBACK;")?.execute(&[])?;
    assert!(db.memory_get(committed_id).is_some());
    db.close()?;
    drop(db);
    let reopened = Connection::open(&file)?;
    assert_eq!(reopened.query("SELECT * FROM sources;")?.len(), 1);
    let results = reopened.search_memories(&MemorySearch {
        query: Some("repair".into()),
        embedding: Some(vec![1.0, 0.0]),
        namespace: Some("manuals".into()),
        ..Default::default()
    })?;
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].entry.id, committed_id);
    assert!(results[0].semantic_score > 0.99);
    assert!(
        reopened
            .search_memories(&MemorySearch {
                query: Some("repair".into()),
                namespace: Some("private".into()),
                ..Default::default()
            })?
            .is_empty()
    );
    Ok(())
}

#[test]
fn ai_memory_survives_process_exit_and_excludes_uncommitted_data() -> tapirus::Result<()> {
    let directory = tempfile::tempdir()?;
    let file = directory.path().join("abrupt.tapir");
    let child = Command::new(std::env::current_exe()?)
        .args(["--ignored", "--exact", "ai_memory_process_writer"])
        .env("TAPIRUS_AI_CRASH_FILE", &file)
        .output()?;
    assert!(
        child.status.success(),
        "{}",
        String::from_utf8_lossy(&child.stderr)
    );
    let db = Connection::open(&file)?;
    assert_eq!(db.memory_count(), 1);
    assert!(
        db.memory_get(1).is_none(),
        "committed deletion must survive abrupt exit"
    );
    let results = db.search_memories(&MemorySearch {
        embedding: Some(vec![1.0, 0.0]),
        ..Default::default()
    })?;
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].entry.content, "Durable source");
    Ok(())
}

#[test]
#[ignore = "subprocess helper, invoked by the crash recovery test"]
fn ai_memory_process_writer() {
    let Some(file) = std::env::var_os("TAPIRUS_AI_CRASH_FILE") else {
        return;
    };
    let db = Connection::open(file).unwrap();
    let old = db.remember("Delete this source").unwrap();
    let durable = db
        .store_memory(&MemoryDocument {
            content: "Durable source".into(),
            embedding: Some(vec![1.0, 0.0]),
            ..Default::default()
        })
        .unwrap();
    db.memory_link(old, durable).unwrap();
    assert!(db.memory_forget_checked(old).unwrap());
    db.execute("BEGIN;").unwrap();
    db.remember("Uncommitted source").unwrap();
    // Deliberately skip every database destructor.
    std::process::exit(0);
}

#[test]
fn malformed_ai_inputs_do_not_change_memory() -> tapirus::Result<()> {
    let db = Connection::open_in_memory()?;
    assert!(
        db.memory_remember("bad", Some(&[f32::NAN]), 0.5, &[])
            .is_err()
    );
    assert!(db.memory_remember("bad", Some(&[]), 0.5, &[]).is_err());
    assert!(db.memory_remember("bad", None, f32::NAN, &[]).is_err());
    assert!(db.store_memory(&MemoryDocument::default()).is_err());
    assert!(db.search_memories(&MemorySearch::default()).is_err());
    assert_eq!(db.memory_count(), 0);
    Ok(())
}
