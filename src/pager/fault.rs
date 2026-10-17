//! Per-connection deterministic I/O faults for recovery validation.
//! Injection is available only with the explicit `fault-injection` Cargo feature.
use std::io::{self, Write};

/// Storage boundary where a deterministic failure can be injected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IoPoint {
    /// WAL frame header or payload write.
    WalWrite,
    /// WAL buffered flush.
    WalFlush,
    /// WAL durable sync.
    WalSync,
    /// Base database write during checkpoint/recovery.
    CheckpointWrite,
    /// Base database durable checkpoint sync.
    CheckpointSync,
    /// WAL truncation during rollback/reset.
    WalTruncate,
    /// Fresh WAL header after a successful base checkpoint.
    WalReset,
    /// Destination backup write, without affecting source database health.
    BackupWrite,
    /// Destination backup durable sync.
    BackupSync,
}

/// Failure mode for deterministic storage experiments.
#[derive(Debug, Clone, Copy)]
pub enum IoFailure {
    /// Return an I/O error before performing the operation.
    DiskFull,
    /// Write and flush a prefix, then return an I/O error.
    PartialWrite {
        /// Prefix bytes allowed before failure.
        bytes: usize,
    },
}

/// A fault applies to one connection, never to unrelated parallel tests.
#[derive(Debug, Clone)]
pub struct IoFaultPlan {
    /// Boundary to instrument.
    pub point: IoPoint,
    /// Number of matching operations allowed before the first failure.
    pub after: usize,
    /// Failure behavior.
    pub failure: IoFailure,
    /// Keep failing matching operations until the plan is cleared.
    pub persistent: bool,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct StorageIo {
    #[cfg(feature = "fault-injection")]
    state: std::sync::Arc<parking_lot::Mutex<Option<IoFaultPlan>>>,
}

fn injected_error() -> io::Error {
    #[cfg(windows)]
    let code = 112;
    #[cfg(not(windows))]
    let code = 28;
    io::Error::from_raw_os_error(code)
}

impl StorageIo {
    #[cfg(feature = "fault-injection")]
    pub fn configure(&self, plan: Option<IoFaultPlan>) -> crate::Result<()> {
        if plan.as_ref().is_some_and(|p| {
            matches!(p.failure, IoFailure::PartialWrite { .. })
                && !matches!(
                    p.point,
                    IoPoint::WalWrite
                        | IoPoint::CheckpointWrite
                        | IoPoint::WalReset
                        | IoPoint::BackupWrite
                )
        }) {
            return Err(crate::Error::ConstraintViolation(
                "Partial-write faults require a write boundary".into(),
            ));
        }
        *self.state.lock() = plan;
        Ok(())
    }
    fn next(&self, point: IoPoint) -> Option<IoFailure> {
        #[cfg(feature = "fault-injection")]
        {
            let mut guard = self.state.lock();
            let plan = guard.as_mut()?;
            if plan.point != point {
                return None;
            }
            if plan.after > 0 {
                plan.after -= 1;
                return None;
            }
            let failure = plan.failure;
            if !plan.persistent {
                *guard = None;
            }
            Some(failure)
        }
        #[cfg(not(feature = "fault-injection"))]
        {
            let _ = point;
            None
        }
    }
    pub fn before(&self, point: IoPoint) -> io::Result<()> {
        if self.next(point).is_some() {
            return Err(injected_error());
        }
        Ok(())
    }
    pub fn write(&self, point: IoPoint, writer: &mut impl Write, bytes: &[u8]) -> io::Result<()> {
        match self.next(point) {
            Some(IoFailure::PartialWrite { bytes: prefix }) => {
                writer.write_all(&bytes[..prefix.min(bytes.len())])?;
                writer.flush()?;
                Err(injected_error())
            }
            Some(IoFailure::DiskFull) => Err(injected_error()),
            None => writer.write_all(bytes),
        }
    }
}
