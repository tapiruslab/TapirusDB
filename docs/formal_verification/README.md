# WAL TLA+ Specification: Scope and CI Verification

This directory contains the TLA+ crash-safety and WAL recovery model ([TAPIRUS_WAL.tla](TAPIRUS_WAL.tla)) together with its finite TLC configuration ([TAPIRUS_WAL.cfg](TAPIRUS_WAL.cfg)).

## Model Scope

- Models TapirusDB's Single-Writer write-ahead log protocol (`BeginTx`, `WriteBuffer`, `AppendWalFrame`, `CommitTx`, `AbortTx`, `Checkpoint`, `Crash`, `Recover`, `Done`).
- Verifies four safety invariants over the bounded configuration (`MaxPages = 2`, `MaxTxId = 2`, `MaxVal = 2`, `MaxWalLen = 4`):
  - `TypeOK`
  - `RecoveredDiskMatchesCommitted`
  - `CheckpointedDiskMatchesCommitted`
  - `NoUncommittedCheckpoint`

## Running TLC Locally or in CI

CI executes TLC on every push/PR in `.github/workflows/ci.yml` (`formal-verification` job).

To run locally with Java (`default-jre-headless` / OpenJDK 17+) and `tla2tools.jar`:

```bash
java -XX:+UseParallelGC -cp tla2tools.jar tlc2.TLC -workers auto -config docs/formal_verification/TAPIRUS_WAL.cfg docs/formal_verification/TAPIRUS_WAL.tla
```

## Boundaries & Non-Claims

A passing TLC run proves the safety invariants hold across every reachable state of the bounded TLA+ specification (`MaxPages = 2`, `MaxTxId = 2`, `MaxVal = 2`, `MaxWalLen = 4`). It is a formal design verification artifact, not an automated refinement proof equivalence between TLA+ actions and the Rust implementation in `src/pager/`.
