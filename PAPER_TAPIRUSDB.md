# TapirusDB: Design Notes for an Embedded Multi-Model Database

**Status:** Unreviewed technical overview. This document is not a peer-reviewed paper, benchmark report, safety case, or certification.

> **Evidence notice:** An audit dated 15 October 2026 found correctness issues and benchmark methodology problems in the repository. Earlier benchmark numbers, memory estimates, security conclusions, and formal-verification claims in this document are withdrawn. No performance result below should be cited as a TapirusDB product measurement.

## Abstract

TapirusDB is an embedded Rust database project that combines a single-file storage format with relational SQL, vector, graph, and JSON document APIs. It includes a write-ahead log, optional page encryption, and bindings for several languages. This note describes the intended architecture and the limits of the available evidence. It does not establish full SQL compatibility, general ACID guarantees, crash safety across all filesystems, production suitability, or performance relative to other systems.

## 1. Scope

TapirusDB targets applications that want database functionality in the same process. It is not a distributed database, does not provide multi-master consensus, and is not a drop-in replacement for SQLite, PostgreSQL, or a dedicated vector or graph service.

The SQL parser supports a subset of SQL. Applications should check the implemented parser and executor behavior for each query shape, constraint, and transaction operation they depend on. The vector, graph, and document APIs are separate interfaces; availability of several APIs does not by itself prove cross-model transaction semantics for every operation.

## 2. Storage and recovery design

The repository contains a page-oriented database file, a write-ahead log, checkpoint and recovery code, and regression tests. These components are intended to preserve committed state across supported recovery scenarios. Their existence does not establish correct behavior for every process crash, operating-system failure, filesystem, controller, or device.

The architecture guide is a conceptual description. Validate the actual implementation with crash-injection and reopen tests on the target storage stack. The main crate forbids explicit unsafe Rust, but safe Rust does not prove transactional correctness or prevent panics and data loss caused by logic or I/O errors.

## 3. Vector representation

The repository contains vector search and quantization code. Payload arithmetic can estimate the bytes needed for a packed representation, but it excludes index structures, metadata, allocator overhead, cached pages, query state, and runtime memory. No total resident-memory or end-to-end latency figure is claimed here. Measure recall, latency, and memory using the target dimensions, dataset, build, and hardware.

## 4. Security and assurance boundary

Page encryption is optional. Key management, process isolation, authentication, network binding, backup handling, and host security remain deployment responsibilities. The C ABI crate has an unsafe boundary separate from the main crate.

TLA+ model files exist for selected WAL transitions. The repository does not include TLC execution results or a refinement proof linking the model to the Rust implementation. No functional-safety certification or independent security assessment is claimed.

## 5. Benchmark status

The earlier benchmark tables and derived comparisons have been removed because the audit found cases that may be optimized away, mislabeled distance calculations, extrapolated memory estimates, and workloads with different durability settings. Re-run corrected harnesses and publish the source revision, build flags, hardware, data, storage settings, warm or cold state, and latency distribution before making a performance claim.

The new small-workload release harness checks returned values, committed file reopen and exact-neighbor recall. Its dated local measurements and limits are recorded in [release validation](docs/RELEASE_VALIDATION.md); they do not establish results at production scales or across platforms.

## 6. Reproducibility and limitations

A reproducible evaluation should include:

- the exact source revision and compiler/toolchain;
- the database configuration, durability mode, and filesystem;
- the dataset, query mix, and index parameters;
- separate warm-cache and cold-cache results where relevant;
- correctness checks alongside performance measurements;
- p50, p95, and p99 latency, throughput, and measured peak resident memory.

Passing a finite test suite is evidence only for the exercised cases. It does not amount to a proof of correctness or a deployment certification.

## Further reading

- [API cookbook](docs/COOKBOOK.md)
- [WAL and recovery architecture notes](docs/architecture/02_WAL_TRANSACTIONS_AND_RECOVERY.md)
- [Assurance scope and limitations](docs/HIGH_ASSURANCE_SAFETY.md)
- [Release validation and measured scope](docs/RELEASE_VALIDATION.md)
