# Deep Architecture Guide: WAL2 Transactions, ARIES Recovery & Durability
==========================================================================

> **Author**: Ahmad Faiz • Tapirus Tech Lab  
> **Status**: Production Reference Specification  
> **Target Audience**: Systems Engineers, Reliability Officers, and Core Contributors

---

## 1. The Write-Ahead Logging (WAL) Invariant

TapirusDB implements atomic, consistent, isolated, and durable (ACID) transactions using a **Write-Ahead Log (WAL2) Protocol** inspired by IBM's ARIES algorithms, engineered entirely in **100% Safe Rust (`#![forbid(unsafe_code)]`)**.

### The Fundamental WAL Invariant
> **No modified (dirty) 4KB database page is EVER written to the main `.tapir` container file until the corresponding WAL frame describing that change has been fully serialized and flushed (`fsync`) to the `.tapir-wal` journal file.**

This guarantees that in the event of an abrupt power failure, operating system crash, or kernel panic, the main `.tapir` file remains intact, and the recovery engine can cleanly reconstruct or roll back transactions.

---

## 2. WAL2 Journal Frame Physical Layout

The `.tapir-wal` journal consists of a 32-byte WAL Header followed by sequential 4,120-byte WAL Frames:

```
+-------------------------------------------------------------+
|                      WAL File Header (32 Bytes)             |
| Magic (4B) | Version (4B) | Page Size (4B) | Checksum (8B)  |
+-------------------------------------------------------------+
|                      WAL Frame 1 (4,120 Bytes)              |
| Frame Header (24 Bytes) + Page Payload (4,096 Bytes)        |
+-------------------------------------------------------------+
|                      WAL Frame 2 (4,120 Bytes)              |
| Frame Header (24 Bytes) + Page Payload (4,096 Bytes)        |
+-------------------------------------------------------------+
```

### 2.1 Frame Header Breakdown (24 Bytes)
```
 0                   1                   2                   3
 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                     Page Number (4 Bytes)                     |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                   Transaction ID (8 Bytes)                    |
|                                                               |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|       Commit Flag (1 Byte)    |       Reserved (3 Bytes)      |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                   CRC32C Checksum (4 Bytes)                   |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                                                               |
|                  4,096-Byte Page Data Payload                 |
|                                                               |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                   Trailer Checksum (4 Bytes)                  |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
```

* **Commit Flag**:
  * `0x00`: Intermediate transaction frame.
  * `0x01`: Transaction Commit Boundary (all prior frames within this TxID are atomically permanent).
* **Dual Checksums (CRC32C)**: Both the frame header and the complete 4,096-byte payload are protected by Castagnoli polynomials (`crc32fast`), detecting silent hardware bit-rot and partial-page write tearing.

---

## 3. The 3-Phase Crash Recovery State Machine

When a TapirusDB connection opens an existing database, it inspects whether a `.tapir-wal` journal exists. If present, it executes the **ARIES 3-Phase Recovery Protocol**:

```
 [Open .tapir] ──► [Check .tapir-wal]
                         │
                         ├── (No WAL) ──► Normal Read/Write
                         │
                         ▼ (WAL Exists)
           ┌───────────────────────────┐
           │ Phase 1: Analysis Pass    │ Scan WAL from offset 32 to EOF.
           │                           │ Identify active & committed TxIDs.
           └─────────────┬─────────────┘
                         ▼
           ┌───────────────────────────┐
           │ Phase 2: Redo Pass        │ Replay all committed frames in order
           │                           │ into the in-memory page buffer pool.
           └─────────────┬─────────────┘
                         ▼
           ┌───────────────────────────┐
           │ Phase 3: Undo Pass        │ Discard uncommitted frames.
           │                           │ Flush dirty pages to .tapir.
           └─────────────┬─────────────┘
                         ▼
             Truncate & Clear .tapir-wal
```

### Phase 1: Analysis Pass
* Scans all frames sequentially.
* Validates every frame's CRC32C checksum. If a torn frame is detected (e.g., interrupted power failure during write), recovery halts at the last valid frame boundary.
* Compiles a set of **Committed Transactions** and **Active (Uncommitted) Transactions**.

### Phase 2: Redo Pass
* For every frame belonging to a transaction with `Commit Flag == 0x01`, the 4KB page is written directly to the target page slot in the container.
* This restores the database to the exact microsecond before the crash.

### Phase 3: Undo Pass
* Any frames written by aborted or uncommitted transactions are discarded.
* The recovery manager executes an atomic checkpoint, flushes all recovered pages to the main `.tapir` container, and truncates the WAL file to zero bytes.

---

## 4. Checkpoint Isolation & Concurrency

TapirusDB supports concurrent multi-reader single-writer transactions without write blocking:

1. **Readers Never Block Writers**: Readers read committed pages directly from the buffer pool or main `.tapir` file, consulting the WAL index for recent updates.
2. **Checkpoint Isolation**: During a `CHECKPOINT` command, active uncommitted transactions continue writing new frames to the WAL without blocking the background checkpoint thread. Only frames up to the checkpoint log sequence number (LSN) are flushed.
3. **Zero Data Loss**: Formally specified and verified under **TLA+** model checking in [`docs/formal_verification/`](docs/formal_verification/).
