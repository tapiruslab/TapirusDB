//! Error and Result definitions for TapirusDB

use thiserror::Error;

/// TapirusDB Result type alias
pub type Result<T> = std::result::Result<T, Error>;

/// Enumeration of all possible TapirusDB error variants
#[derive(Error, Debug)]
pub enum Error {
    /// Input/Output failure on disk file
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Invalid database file magic header
    #[error("Invalid magic bytes: file is not a valid TapirusDB database")]
    InvalidMagic,

    /// Page size is invalid (must be power of 2 between 512 and 65536)
    #[error("Invalid page size {0}: must be power of 2 between 512 and 65536")]
    InvalidPageSize(u16),

    /// Page corrupted or checksum mismatch
    #[error("Page {0} corrupted: checksum mismatch (expected {1:#x}, found {2:#x})")]
    PageCorrupted(u32, u32, u32),

    /// Page not found
    #[error("Page {0} not found in database file")]
    PageNotFound(u32),

    /// SQL syntax error
    #[error("SQL syntax error: {0}")]
    SqlSyntax(String),

    /// Table already exists
    #[error("Table '{0}' already exists")]
    TableExists(String),

    /// Table does not exist
    #[error("Table '{0}' does not exist")]
    TableNotFound(String),

    /// Vector dimension mismatch
    #[error("Vector dimension mismatch: expected {0} dimensions, got {1}")]
    DimensionMismatch(usize, usize),

    /// Decryption failed due to invalid key or corrupted MAC
    #[error("Decryption failed for page {0}: invalid encryption key or corrupted ciphertext")]
    DecryptionFailed(u32),

    /// Database is encrypted but no key was provided
    #[error("Database is encrypted: encryption key required to open")]
    EncryptedDatabase,

    /// Transaction error (e.g. no active transaction to commit or rollback)
    #[error("Transaction error: {0}")]
    TransactionError(String),

    /// General corruption
    #[error("Database corrupted: {0}")]
    Corrupted(String),

    /// Constraint violation (e.g. unique constraint or duplicate primary key)
    #[error("Constraint violation: {0}")]
    ConstraintViolation(String),

    /// Database is busy or locked by another process
    #[error("Database busy: {0}")]
    Busy(String),

    /// Serialization or deserialization error
    #[error("Serialization error: {0}")]
    Serialization(String),
}
