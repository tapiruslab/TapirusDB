# WAL, MVCC Snapshots & Recovery Architecture

TapirusDB implements a Single-Writer Multiple-Reader (SWMR) page-level Write-Ahead Log (`src/pager/wal.rs`) paired with non-blocking MVCC committed snapshot readers (`src/session.rs`) inside a `#![forbid(unsafe_code)]` Safe-Rust storage core.

## Write-Ahead Log Protocol

- Committed mutations append CRC32-verified page frames and explicit commit markers to `.tapir-wal` before any page is checkpointed into the main `.tapir` database file.
- When `DatabaseCipher` is active, each page frame payload is authenticated with 256-bit XChaCha20-Poly1305 AEAD (`page_id` + Page 1 header AAD binding on Page 1).
- Uncommitted writes remain isolated to the active writer session and dirty page buffer, ensuring incomplete tails or aborted transactions never leak into committed database state.

## Crash Recovery & Fault Tolerance

- Reopening a database replays valid committed WAL frames sequentially from the latest durable checkpoint LSN, automatically discarding incomplete or uncommitted trailing frames.
- Regression and fault-injection suites (`tests/durability_regressions.rs`, `tests/io_fault_recovery_tests.rs`, `tests/crash_injection_tests.rs`, `tests/ai_memory_durability_tests.rs`) validate recovery under torn writes, header corruption, WAL reset failures, savepoints, rollbacks, and child-process `SIGKILL`/termination.
- `Connection::storage_health()` reports deferred maintenance diagnostics so applications can observe storage health without masking durable commits.

## Concurrency, SWMR & MVCC Snapshot Isolation

- **`SessionDatabase` Exclusive Writers + MVCC Snapshot Readers:**
  - `db.session()` / `db.session_with_timeout(Duration)` acquire ownership of logical write sessions with clean commit/rollback boundaries (unclosed active transactions automatically roll back on drop).
  - `db.snapshot_reader()` acquires a non-blocking `SnapshotSession` pinned to the latest committed `mvcc_epoch()` immediately, without waiting for concurrent uncommitted writers.
- **Multi-Process SWMR (`Config::concurrent_access`):** Coordinates separate OS processes safely against a shared `.tapir` + `.tapir-wal` store (`Connection::open_concurrent*`, `Connection::open_read_only*`, `refresh_from_disk()`).

## Formal TLA+ Verification (`TLC2`)

The Single-Writer WAL protocol and crash-recovery state machine are formally specified in [`docs/formal_verification/TAPIRUS_WAL.tla`](../formal_verification/TAPIRUS_WAL.tla) and checked automatically on every push/PR in `.github/workflows/ci.yml`:

- **Verified invariants:** `TypeOK`, `RecoveredDiskMatchesCommitted`, `CheckpointedDiskMatchesCommitted`, `NoUncommittedCheckpoint`.
- **Exhaustive TLC2 CI result (`commit d036710`, bounded configuration `MaxPages=2, MaxTxId=2, MaxVal=2, MaxWalLen=4`):** `552,520` states generated, `120,535` distinct states explored, `0` invariant violations.

See [`docs/formal_verification/README.md`](../formal_verification/README.md) and [`SECURITY.md`](../../SECURITY.md) for reproduction commands and threat boundaries.
