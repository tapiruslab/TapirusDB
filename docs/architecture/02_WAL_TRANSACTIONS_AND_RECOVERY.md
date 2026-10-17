# WAL and recovery architecture notes

**Status:** Conceptual notes. This document does not establish full ACID behavior, ARIES conformance, or crash safety across all filesystems and devices.

TapirusDB stores data in a page-oriented `.tapir` file and uses a write-ahead log during mutations. The main Rust crate forbids explicit unsafe Rust. These implementation choices do not prove that every transaction, recovery, or I/O failure path is correct.

## Write-ahead log

The WAL is intended to record page changes before those changes are checkpointed into the database file. Frames contain checksums for detecting some incomplete or corrupted writes. Checksums are not proof against all storage faults or malicious modification.

The exact frame layout and behavior can change with the file format. Consult the current pager and WAL implementation before building tools that read or write these internal files directly.

## Recovery and checkpoints

Opening a database processes its WAL and restores state according to the frames the implementation recognizes as valid and committed. Checkpointing transfers recoverable state into the main database file and advances the journal state.

Regression cases for incomplete WAL tails, rollback metadata, savepoints, and persistence live in `tests/durability_regressions.rs`. Those tests cover selected scenarios. Validate crash behavior on the actual filesystem, storage device, and build used by an application before relying on important data.

## Connection and concurrency semantics

`ConnectionPool::acquire()` returns a clone of the same `Connection`. Acquired handles share session and transaction state; the current API does not create independent sessions. SQL operations use shared locks around pager and executor state. The pool API should not be interpreted as a guarantee of parallel SQL readers or multi-session isolation.

## Formal model

A TLA+ specification for selected WAL transitions is stored in `docs/formal_verification/`. This repository contains no TLC run output, explored-state report, invariant result, or refinement proof linking the model to Rust. The specification is a design artifact, not verification of the compiled database.

## Operational guidance

- Test commit, rollback, crash recovery, backup, restore, and reopen behavior on the target platform.
- Keep independent backups and periodically perform a restore test.
- Treat checksums as error detection, not a substitute for backups or authenticated encryption.
- Encryption is optional; applications remain responsible for passphrase handling and key lifecycle.
- See `SECURITY.md` and the dated audit report for current trust boundaries and known limitations.