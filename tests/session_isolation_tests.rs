use std::time::Duration;
use tapirus::{Error, MemoryDocument, MemorySearch, SessionDatabase};

#[test]
fn sessions_cannot_read_commit_or_rollback_another_sessions_transaction() {
    let database = SessionDatabase::open_in_memory().unwrap();
    let a = database.session().unwrap();
    let b = database.session().unwrap();
    a.execute("CREATE TABLE readings (id INTEGER PRIMARY KEY, value INTEGER)")
        .unwrap();
    a.begin().unwrap();
    a.execute("INSERT INTO readings VALUES (1, 42)").unwrap();
    a.store_memory(&MemoryDocument {
        content: "Uncommitted robot observation".into(),
        ..Default::default()
    })
    .unwrap();
    assert!(matches!(
        b.query("SELECT * FROM readings"),
        Err(Error::Busy(_))
    ));
    assert!(matches!(b.commit(), Err(Error::Busy(_))));
    assert!(matches!(b.rollback(), Err(Error::Busy(_))));
    assert!(matches!(
        b.search_memories(&MemorySearch {
            query: Some("robot".into()),
            ..Default::default()
        }),
        Err(Error::Busy(_))
    ));
    assert!(matches!(database.close(), Err(Error::Busy(_))));
    a.commit().unwrap();
    assert_eq!(b.query("SELECT * FROM readings").unwrap().len(), 1);
    assert_eq!(
        b.search_memories(&MemorySearch {
            query: Some("robot".into()),
            ..Default::default()
        })
        .unwrap()
        .results
        .len(),
        1
    );
}

#[test]
fn last_session_handle_drop_rolls_back_and_releases_ownership() {
    let database = SessionDatabase::open_in_memory().unwrap();
    let a = database.session().unwrap();
    a.execute("CREATE TABLE session_records (id INTEGER)")
        .unwrap();
    a.begin().unwrap();
    a.execute("INSERT INTO session_records VALUES (7)").unwrap();
    let clone = a.clone();
    drop(a);
    let b = database.session().unwrap();
    assert!(matches!(
        b.query("SELECT * FROM session_records"),
        Err(Error::Busy(_))
    ));
    drop(clone);
    assert!(b.query("SELECT * FROM session_records").unwrap().is_empty());
    b.begin().unwrap();
    b.execute("INSERT INTO session_records VALUES (8)").unwrap();
    b.close().unwrap();
    let c = database.session().unwrap();
    assert!(c.query("SELECT * FROM session_records").unwrap().is_empty());
    database.close().unwrap();
    assert!(c.query("SELECT * FROM session_records").is_err());
}

#[test]
fn waiting_session_proceeds_after_commit_and_timeout_is_bounded() {
    let database = SessionDatabase::open_in_memory().unwrap();
    let a = database.session().unwrap();
    a.execute("CREATE TABLE session_records (id INTEGER)")
        .unwrap();
    a.begin().unwrap();
    a.execute("INSERT INTO session_records VALUES (1)").unwrap();
    let timed = database
        .session_with_timeout(Duration::from_millis(5))
        .unwrap();
    assert!(matches!(
        timed.query("SELECT * FROM session_records"),
        Err(Error::Busy(_))
    ));
    let waiting = database
        .session_with_timeout(Duration::from_secs(2))
        .unwrap();
    let thread = std::thread::spawn(move || {
        waiting
            .query("SELECT * FROM session_records")
            .unwrap()
            .len()
    });
    a.commit().unwrap();
    assert_eq!(thread.join().unwrap(), 1);
}

#[test]
fn transaction_callback_errors_and_panics_rollback_without_leaking_ownership() {
    let database = SessionDatabase::open_in_memory().unwrap();
    let a = database.session().unwrap();
    a.execute("CREATE TABLE session_records (id INTEGER)")
        .unwrap();
    let result: tapirus::Result<()> = a.transaction(|session| {
        session.execute("INSERT INTO session_records VALUES (1)")?;
        Err(Error::ConstraintViolation("application failure".into()))
    });
    assert!(result.is_err());
    assert!(a.query("SELECT * FROM session_records").unwrap().is_empty());
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _: tapirus::Result<()> = a.transaction(|session| {
            session.execute("INSERT INTO session_records VALUES (2)")?;
            panic!("application panic")
        });
    }));
    assert!(panic.is_err());
    let b = database.session().unwrap();
    assert!(b.query("SELECT * FROM session_records").unwrap().is_empty());
    assert!(b.integrity_check().unwrap().is_ok());
}
