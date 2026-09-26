# Deep Architecture Guide: 4KB Slotted-Page Storage Engine & Buffer Pool
========================================================================

> **Author**: Ahmad Faiz • Tapirus Tech Lab  
> **Status**: Production Reference Specification  
> **Target Audience**: Systems Engineers, Database Architects, and Core Contributors

---

## 1. Executive Summary

TapirusDB achieves true single-file embedded zero-corruption persistence through a **4,096-byte (4KB) Slotted-Page B+Tree Architecture** implemented entirely in **100% Safe Rust (`#![forbid(unsafe_code)]`)**.

Unlike naive file-append key-value stores or heavy multi-file database directories, every entity—relational tuple, HNSW vector node, property graph edge, and JSON document—is serialized into bounded 4KB pages. This layout maximizes hardware cache line efficiency, aligns directly with OS virtual memory page boundaries, and guarantees atomic write durability.

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│                       TapirusDB Single-File Container (.tapir)                  │
├───────────────────┬───────────────────┬───────────────────┬─────────────────────┤
│ Page 0: File Hdr  │ Page 1: Schema B+ │ Page 2: Vector Hdr│ Page N: Data Leaves │
│ (Magic, KCV, Size)│ (Table Catalog)   │ (HNSW Entry Point)│ (4096-Byte Slotted) │
└───────────────────┴───────────────────┴───────────────────┴─────────────────────┘
```

---

## 2. 4KB Slotted-Page Physical Byte Layout

Every page in a `.tapir` container file is exactly 4,096 bytes. The slotted-page structure allows variable-length records (e.g. TEXT strings, dynamic JSON objects, variable-dimension vectors) to be stored within a fixed-size block without external fragmentation.

```
 0                   1                   2                   3
 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|       Page Type (1 Byte)      |     Flags (1 Byte)            |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|     Cell Count (2 Bytes)      |   Free Space Offset (2 Bytes) |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                 Right Child Page ID (4 Bytes)                 |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|  Cell Offset 0 (2 Bytes)      |  Cell Offset 1 (2 Bytes)      |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                             ...                               |
|                  Cell Pointer Array (Grows Downward)          |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                                                               |
|                       UNALLOCATED FREE SPACE                  |
|                                                               |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                  Cell Payload N (Grows Upward)                |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                  Cell Payload 0 (4KB Page Boundary)           |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
```

### 2.1 Header Fields
* **Page Type (Byte 0)**:
  * `0x01`: B+Tree Interior Index Page (stores routing keys and child page IDs).
  * `0x02`: B+Tree Leaf Data Page (stores table rows, values, and primary keys).
  * `0x03`: Overflow / Spill Page (stores oversized BLOBs or large text).
  * `0x04`: Vector HNSW Node Page (stores quantized centroids and neighbor edge lists).
  * `0x05`: Graph CSR Page (stores vertex offsets and contiguous edge arrays).
* **Cell Count (Bytes 2..3)**: Total number of active records indexed in this page.
* **Free Space Offset (Bytes 4..5)**: Pointer to the beginning of the free contiguous byte region.
* **Right Child Page ID (Bytes 6..9)**: Rightmost sibling pointer for sequential $O(1)$ scans.
* **Cell Pointer Array (Bytes 10..10+2*N)**: Array of 16-bit byte offsets pointing to individual cell bodies at the bottom of the page.

### 2.2 Invariant Guarantees
1. **No External Fragmentation**: Pointers grow **downward** from offset 10; payload bytes grow **upward** from offset 4095.
2. **Defragmentation on Demand**: When cells are deleted, their pointer is zeroed (tombstone). During `VACUUM` or when free space is fragmented, payloads are shifted to the page bottom in a single $O(P)$ memory sweep.

---

## 3. B+Tree Node Splitting & Search Mechanics

### 3.1 Search Traversal: $O(\log_B N)$
When executing `SELECT * FROM table WHERE id = 42`:
1. The engine reads the root page from the Schema Catalog.
2. Binary search is executed over the 16-bit cell offset array.
3. If the page is an Interior Page, it identifies the child page pointer $\le 42$ and recurses.
4. If the page is a Leaf Page, it locates the exact cell payload, decodes the column values zero-copy, and returns the row.

### 3.2 Proactive Page Splitting
When an `INSERT` statement causes the free space between the pointer array and the payload region to drop below the required cell size:
1. A new 4KB page is allocated from the free list or appended to the container.
2. Half of the cell pointers and their corresponding payloads are copied to the sibling page.
3. Sibling pointers (`next_page_id`) are updated atomically.
4. The median key is promoted to the parent interior node. If the root splits, a new root page is minted, increasing tree height by 1.

---

## 4. Page Buffer Pool & Clock-Sweep Cache

TapirusDB employs an in-memory Page Buffer Pool with a **Clock-Sweep (Second-Chance) Replacement Policy**:

```
                  Clock Hand Pointer
                         │
                         ▼
       [Page 12 (Ref=1)] ──► [Page 45 (Ref=0)] ──► [Page 9 (Ref=1)]
```

* **Dirty Page Tracking**: Pages modified in memory are flagged as dirty and pinned until their Write-Ahead Log (WAL2) frame is synced to disk.
* **Pin Count Protection**: While an active iterator or search traversal is reading a page, its pin count is incremented, preventing the clock hand from evicting it.
* **Zero IPC Overhead**: Reading a page from the buffer pool is a direct Rust slice reference without context switching or socket communication.

---

## 5. Overflow & Large BLOB Spill Protocol

If a single record (e.g. a 50KB JSON payload or high-dimensional unquantized vector) exceeds the usable page payload space ($\approx 4040$ bytes):
1. The first 4KB chunk is stored on the primary leaf page along with a 4-byte `overflow_page_id` pointer.
2. The remaining data is segmented across a singly-linked chain of `0x03` Overflow Pages.
3. Reading an overflow record streams the linked pages sequentially without consuming contiguous physical file blocks.
