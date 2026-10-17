//! Independent logical sessions over one serialized native connection.
//!
//! A transaction belongs to exactly one session. Other sessions wait up to their
//! busy timeout or return Busy, and cannot observe that transaction's mutations.
//! This supplies isolation without claiming concurrent readers or MVCC.
use crate::{
    Config, Connection, Error, IntegrityReport, MemoryDocument, MemorySearch, MemorySearchResponse,
    Result, Row, Value,
};
use parking_lot::{Condvar, Mutex};
use std::{path::Path, sync::Arc};
use web_time::{Duration, Instant};

struct State {
    connection: Option<Connection>,
    owner: Option<u64>,
    next_id: u64,
    poisoned: bool,
}
struct Shared {
    state: Mutex<State>,
    changed: Condvar,
}

/// One database with independently owned logical sessions.
#[derive(Clone)]
pub struct SessionDatabase {
    shared: Arc<Shared>,
}

impl SessionDatabase {
    /// Open a native database. Its connection is exclusively managed by sessions.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_with_config(path, Config::default())
    }
    /// Open a database with explicit storage, encryption and memory limits.
    pub fn open_with_config(path: impl AsRef<Path>, config: Config) -> Result<Self> {
        Self::from_connection(Connection::open_with_config(path, config)?)
    }
    /// Create an in-memory session database.
    pub fn open_in_memory() -> Result<Self> {
        Self::open_in_memory_with_config(Config::default())
    }
    /// Create temporary sessions with explicit memory and cache limits.
    pub fn open_in_memory_with_config(config: Config) -> Result<Self> {
        Self::from_connection(Connection::open_in_memory_with_config(config)?)
    }
    fn from_connection(connection: Connection) -> Result<Self> {
        Ok(Self {
            shared: Arc::new(Shared {
                state: Mutex::new(State {
                    connection: Some(connection),
                    owner: None,
                    next_id: 1,
                    poisoned: false,
                }),
                changed: Condvar::new(),
            }),
        })
    }
    /// Create a session that returns Busy immediately when another owns a transaction.
    pub fn session(&self) -> Result<Session> {
        self.session_with_timeout(Duration::ZERO)
    }
    /// Create a session with a maximum wait for another session's transaction.
    /// Browser WASM always returns Busy immediately rather than blocking its event loop.
    pub fn session_with_timeout(&self, busy_timeout: Duration) -> Result<Session> {
        let mut state = self.shared.state.lock();
        if state.connection.is_none() {
            return Err(Error::TransactionError("Session database is closed".into()));
        }
        let id = state.next_id;
        state.next_id = id
            .checked_add(1)
            .ok_or_else(|| Error::ConstraintViolation("Session ID space exhausted".into()))?;
        Ok(Session {
            handle: Arc::new(SessionHandle {
                shared: self.shared.clone(),
                id,
                busy_timeout,
                closed: Mutex::new(false),
            }),
        })
    }
    /// Close the database and invalidate its sessions. Active transactions must
    /// first be committed, rolled back, or closed by their owning session.
    pub fn close(&self) -> Result<()> {
        let mut state = self.shared.state.lock();
        if state.owner.is_some() {
            return Err(Error::Busy("An active session owns a transaction".into()));
        }
        if let Some(connection) = &state.connection {
            connection.close()?;
        }
        state.connection.take();
        self.shared.changed.notify_all();
        Ok(())
    }
}

struct SessionHandle {
    shared: Arc<Shared>,
    id: u64,
    busy_timeout: Duration,
    closed: Mutex<bool>,
}

/// A logical session. Clones share this session; call database.session() for a
/// distinct transaction owner. Closing the last handle rolls back its transaction.
#[derive(Clone)]
pub struct Session {
    handle: Arc<SessionHandle>,
}

struct TransactionGuard<'a> {
    session: &'a Session,
    armed: bool,
}
impl Drop for TransactionGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            let _ = self.session.rollback();
        }
    }
}

impl Session {
    fn access<T>(&self, operation: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let started = Instant::now();
        let shared = &self.handle.shared;
        let mut state = shared.state.lock();
        loop {
            if *self.handle.closed.lock() {
                return Err(Error::TransactionError("Session is closed".into()));
            }
            if state.poisoned {
                return Err(Error::TransactionError(
                    "Session storage requires reopen after failed rollback".into(),
                ));
            }
            if state.connection.is_none() {
                return Err(Error::TransactionError("Session database is closed".into()));
            }
            if state.owner.is_none_or(|owner| owner == self.handle.id) {
                break;
            }
            let remaining = self.handle.busy_timeout.saturating_sub(started.elapsed());
            if remaining.is_zero() || cfg!(target_arch = "wasm32") {
                return Err(Error::Busy(
                    "Another session owns the active transaction".into(),
                ));
            }
            shared.changed.wait_for(&mut state, remaining);
        }
        let connection = state.connection.as_ref().expect("open session database");
        let result = operation(connection);
        state.owner = if connection.is_in_transaction() {
            Some(self.handle.id)
        } else {
            None
        };
        if state.owner.is_none() {
            shared.changed.notify_all();
        }
        result
    }
    /// Execute one SQL statement in this session.
    pub fn execute(&self, sql: &str) -> Result<usize> {
        self.execute_with_params(sql, &[])
    }
    /// Execute one SQL statement with typed bound values.
    pub fn execute_with_params(&self, sql: &str, values: &[Value]) -> Result<usize> {
        self.access(|connection| connection.execute_with_params(sql, values))
    }
    /// Query SQL without observing another session's uncommitted state.
    pub fn query(&self, sql: &str) -> Result<Vec<Row>> {
        self.query_with_params(sql, &[])
    }
    /// Query SQL using bound values.
    pub fn query_with_params(&self, sql: &str, values: &[Value]) -> Result<Vec<Row>> {
        self.access(|connection| connection.query_with_params(sql, values))
    }
    /// Start an explicitly owned transaction.
    pub fn begin(&self) -> Result<()> {
        self.execute("BEGIN").map(|_| ())
    }
    /// Commit this session's transaction.
    pub fn commit(&self) -> Result<()> {
        self.execute("COMMIT").map(|_| ())
    }
    /// Roll back this session's transaction.
    pub fn rollback(&self) -> Result<()> {
        self.execute("ROLLBACK").map(|_| ())
    }
    /// Run a closure in a transaction; callback errors cause rollback.
    pub fn transaction<T>(&self, callback: impl FnOnce(&Session) -> Result<T>) -> Result<T> {
        self.begin()?;
        let mut guard = TransactionGuard {
            session: self,
            armed: true,
        };
        match callback(self) {
            Ok(value) => {
                self.commit()?;
                guard.armed = false;
                Ok(value)
            }
            Err(error) => {
                self.rollback().map_err(|rollback| {
                    Error::TransactionError(format!("{error}; rollback also failed: {rollback}"))
                })?;
                guard.armed = false;
                Err(error)
            }
        }
    }
    /// Insert a source in this session's transaction.
    pub fn store_memory(&self, document: &MemoryDocument) -> Result<u64> {
        self.access(|connection| connection.store_memory(document))
    }
    /// Replace a source by its scoped external ID.
    pub fn upsert_memory(&self, document: &MemoryDocument) -> Result<u64> {
        self.access(|connection| connection.upsert_memory(document))
    }
    /// Upsert sources in one atomic statement.
    pub fn upsert_memory_batch(&self, documents: &[MemoryDocument]) -> Result<Vec<u64>> {
        self.access(|connection| connection.upsert_memory_batch(documents))
    }
    /// Retrieve source candidates and diagnostics within this session.
    pub fn search_memories(&self, search: &MemorySearch) -> Result<MemorySearchResponse> {
        self.access(|connection| connection.search_memories_detailed(search))
    }
    /// Language-binding memory transport with the same transaction ownership.
    pub fn memory_request_json(&self, operation: &str, payload: &str) -> Result<String> {
        self.access(|connection| connection.memory_request_json(operation, payload))
    }
    /// Checkpoint committed storage.
    pub fn checkpoint(&self) -> Result<usize> {
        self.access(Connection::checkpoint)
    }
    /// Create a committed native backup using the existing backup contract.
    pub fn backup(&self, path: impl AsRef<Path>) -> Result<u32> {
        self.access(|connection| connection.backup(path))
    }
    /// Inspect structural integrity through the owning session.
    pub fn integrity_check(&self) -> Result<IntegrityReport> {
        self.access(Connection::check_integrity)
    }
    /// Close all clones of this session and roll back its active transaction.
    pub fn close(&self) -> Result<()> {
        let shared = &self.handle.shared;
        let mut state = shared.state.lock();
        let mut closed = self.handle.closed.lock();
        if *closed {
            return Ok(());
        }
        if state.owner == Some(self.handle.id) {
            if let Some(connection) = &state.connection {
                connection.rollback()?;
            }
            state.owner = None;
            shared.changed.notify_all();
        }
        *closed = true;
        Ok(())
    }
}

impl Drop for SessionHandle {
    fn drop(&mut self) {
        let mut state = self.shared.state.lock();
        if state.owner == Some(self.id) {
            if let Some(connection) = &state.connection {
                if connection.rollback().is_err() {
                    state.poisoned = true;
                }
            }
            state.owner = None;
            self.shared.changed.notify_all();
        }
    }
}
