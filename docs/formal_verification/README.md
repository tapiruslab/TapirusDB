# 📐 Formal Verification: TapirusDB Write-Ahead Logging (WAL) Protocol

This directory contains the formal mathematical specification of TapirusDB's Write-Ahead Logging (WAL) and crash recovery engine using **TLA+ (Temporal Logic of Actions)**.

---

## 🎯 Verification Goals

1. **Atomicity Invariant (`NoUncommittedDataPersisted`)**: Proves that after an arbitrary crash (power failure or kernel panic), dirty uncommitted pages from aborted transactions NEVER contaminate physical storage.
2. **Durability Invariant (`CommittedDataDurability`)**: Proves that all transactions marked committed in the WAL are faithfully reconstructed upon crash recovery.
3. **Checkpoint Correctness**: Proves that replaying WAL from `lastCheckpointLsn` preserves consistency and prevents torn pages.

---

## 🛠️ Model Checking with TLC

To verify the model against all state transitions:

```bash
# Using TLA+ Tools / tlc
java -cp tla2tools.jar tlc2.TLC TAPIRUS_WAL.tla -config TAPIRUS_WAL.cfg
```

Model parameters:
- `MaxPages = 3`: 3 discrete slotted pages.
- `MaxTxId = 2`: Concurrent transactional workloads.
- Evaluates complete state space of interleaved `WriteBuffer`, `AppendWalFrame`, `CommitTx`, `Checkpoint`, `Crash`, and `Recover` operations.
