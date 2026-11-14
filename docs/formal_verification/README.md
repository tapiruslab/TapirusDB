# WAL TLA+ Specification: Scope and CI Verification

This directory contains the TLA+ crash-safety and WAL recovery model ([TAPIRUS_WAL.tla](TAPIRUS_WAL.tla)) together with its finite TLC configuration ([TAPIRUS_WAL.cfg](TAPIRUS_WAL.cfg)).

## Model Scope

- Models TapirusDB's Single-Writer write-ahead log protocol (`BeginTx`, `WriteBuffer`, `AppendWalFrame`, `CommitTx`, `AbortTx`, `Checkpoint`, `Crash`, `Recover`, `Done`).
- Verifies four safety invariants over the bounded configuration (`MaxPages = 2`, `MaxTxId = 2`, `MaxVal = 2`, `MaxWalLen = 4`, `Nil = 0`):
  - `TypeOK`
  - `RecoveredDiskMatchesCommitted`
  - `CheckpointedDiskMatchesCommitted`
  - `NoUncommittedCheckpoint`

## CI Verification Results

CI executes `tlc2.TLC` on every push and pull request in [`.github/workflows/ci.yml`](../../.github/workflows/ci.yml) (`formal-verification` job).

- **Verified CI run (`TLC2 Version 2.19`, commit `d036710`):**
  - **States generated:** `552,520`
  - **Distinct states explored:** `120,535`
  - **Queue remaining:** `0`
  - **Invariant violations / errors:** `0` (`Model checking completed. No error has been found.`)

## Running TLC Locally

With Java (`default-jre-headless` / OpenJDK 17+) and `tla2tools.jar`:

```bash
java -XX:+UseParallelGC -cp tla2tools.jar tlc2.TLC -workers auto -config docs/formal_verification/TAPIRUS_WAL.cfg docs/formal_verification/TAPIRUS_WAL.tla
```

## Boundaries

The passing TLC run exhaustively verifies every reachable state of the bounded TLA+ specification (`MaxPages = 2`, `MaxTxId = 2`, `MaxVal = 2`, `MaxWalLen = 4`). It serves as a formal protocol-design verification alongside the Rust fault-injection test suite in `tests/io_fault_recovery_tests.rs` and `tests/durability_regressions.rs`.
