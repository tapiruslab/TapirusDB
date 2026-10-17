# Storage Pages and WAL: Implementation Notes

> Status: implementation notes for the current source tree. This is not a crash-safety certification or a stable on-disk format specification.

## Page and file layout

The pager defines a default page size of 4,096 bytes and a file header with the `TAPIRUS\0` magic, format version, freelist metadata, WAL sequence, CRC, and optional encryption and compression flags. The B+Tree module encodes table and index pages with slotted-page headers. Its page types also include vector-index, overflow, and freelist entries.

These structures describe the implementation. They do not by themselves prove atomic persistence, zero corruption, or compatibility across future releases. Treat the file format as version-specific and keep backups before upgrading or experimenting with recovery.

## B+Tree pages

Table and secondary-index records are stored in B+Tree pages. Page headers carry the page type, flags, cell count, content offset, freeblock offset, and, for interior pages, a rightmost child pointer. The encoder and decoder in `src/btree` define the actual byte layout; consult that code when changing the format.

Do not infer a general `O(log N)` latency bound from the tree shape alone. Disk reads, cache state, page splits, serialization, locking, and query planning affect observed behavior.

## WAL and transactions

The pager and WAL modules implement frame logging, commit markers, checkpointing, and recovery paths. Regression tests cover selected commit, rollback, reopen, and corruption cases. They do not establish behavior for every process crash, partial write, filesystem, device cache, or power-loss scenario.

A successful commit result is meaningful only for the operations and I/O path exercised. Preserve and inspect I/O errors, run recovery tests on the target platform, and maintain independent backups for valuable data.

## Buffering and concurrency

The pager maintains in-memory page state, while SQL operations use shared synchronization around pager and executor state. A cloned `ConnectionPool` handle shares a connection/session; it is not an independent transaction session. This design does not imply parallel SQL readers or bounded process memory.

For current API contracts and security limits, see the root `README.md` and `SECURITY.md`.
