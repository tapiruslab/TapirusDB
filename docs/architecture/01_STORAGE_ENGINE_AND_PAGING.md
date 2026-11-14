# Storage Pages and WAL: Engine Architecture

## Page & File Layout

TapirusDB stores all relational tables, secondary indexes, vector indexes, graphs, JSON document collections, and AI memory inside a single page-oriented `.tapir` file paired with a `.tapir-wal` write-ahead log.

- **Default page size:** `4,096` bytes with `TAPIRUS\0` file header magic, format version, freelist metadata, WAL sequence counters, CRC checksums, and encryption/compression flags.
- **Page 1 Header AAD Binding:** Immutable Page 1 format and KDF header fields (`magic`, `page_size`, `version`, `encryption_flags`, `salt`, `kcv`) are authenticated via Poly1305 AEAD Associated Data when encryption is enabled.
- **Transparent Safe-Rust LZ4 Compression:** Pages above Page 1 can be transparently LZ4-compressed before encryption and WAL/disk writes (`conn.enable_compression()`), with on-demand decompression on page cache load.

## B+Tree Storage

Table rows and single- or multi-column composite secondary indexes are persisted in slotted-page B+Trees inside `src/btree/`:

- Interior and leaf page headers encode page type, flags, cell count, content offset, freeblock offset, rightmost child pointer, and overflow page chains for large payloads.
- Borrowed `Cow<[u8]>` in-page inserts avoid redundant heap allocations on standard OLTP paths, paired with sequential right-append leaf splits.

## WAL, Encryption & Recovery

- **Per-Page XChaCha20-Poly1305 AEAD:** Encrypted databases protect every 4 KiB page using 256-bit keys (PBKDF2-HMAC-SHA256 at 600,000 iterations), fresh 24-byte CSPRNG `XNonce` nonces, and 16-byte Poly1305 tags, with non-`Clone` `Zeroize` key lifecycle hygiene.
- **WAL Frame Integrity & Crash Recovery:** Mutations write framed pages with CRC verification and commit markers to `.tapir-wal` before checkpointing into `.tapir`. Fault-injection and crash-recovery suites verify recovery across torn frames, corrupted tails, write/sync failures, and abrupt child termination.
- **Hot Online Backups & Replication:** Built-in `backup`, `vacuum_into`, and incremental WAL frame export/apply (`export_wal_frames`, `apply_wal_frame`) preserve consistent committed snapshots without taking writers offline.

## Concurrency & MVCC Readers

- **SWMR Multi-Process WAL (`Config::concurrent_access`):** Coordinates multiple processes against the same `.tapir` file, acquiring `.tapir-lock` exclusively during writes and refreshing committed WAL state (`refresh_from_disk()`).
- **Non-Blocking MVCC Snapshots (`SessionDatabase::snapshot_reader()`):** Delivers lock-free committed snapshot readers tied to atomic `mvcc_epoch()` counters even while an owning `Session` executes an active uncommitted write transaction.

See [WAL, MVCC & Recovery Architecture](02_WAL_TRANSACTIONS_AND_RECOVERY.md), [SECURITY.md](../../SECURITY.md), and [README.md](../../README.md).
