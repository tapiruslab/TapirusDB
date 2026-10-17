# TapirusDB project status and roadmap

This page tracks work and its supporting evidence. See [release validation](docs/RELEASE_VALIDATION.md) and current source before depending on a capability.

## Current focus

1. Preserve data correctness with regression coverage for WAL recovery, rollback metadata, prepared statements, graph persistence, vector-index freshness, committed snapshots, and complete SQL parsing.
2. Keep HTTP and SDK contracts aligned, including authentication, bound parameters, response shapes, native loading, and close behavior.
3. Maintain the standalone Rust browser workspace, native file interchange and persistence checks. Extend the larger React Studio's integration using the same verified engine.
4. Re-run benchmarks only after correctness work, using observable results, equivalent durability settings, representative data, and published reproduction details.
5. Maintain documentation against implemented APIs; label proposals and historical claims as such.

## Next product gates from the 17 October 2026 evidence

1. Persist or page the derived AI index. A 100,000 × 384 vector experiment
   rebuilt HNSW after reopen in 581 seconds; current source and index data
   reside in RAM. Add a versioned, checksummed cache with recovery and
   encryption rules before promising fast startup at this scale.
2. Improve recall at bounded query cost. On independent synthetic queries,
   128 retained vector candidates recovered 47.3% of exact top-10 neighbors;
   1,024 recovered 96.4% with materially longer queries. Benchmark graph
   construction, IVF/quantized alternatives and representative model vectors.
3. Evaluate ranking on independent BM/English device tasks and more public
   retrieval sets. The authored paraphrase fixture gave 55% hybrid versus
   80% vector Recall@5 with one pinned E5 model; tune fusion on development
   data and reserve a separate held-out set for acceptance.
4. Validate power-cut behavior, flash endurance, stack, ECC and driver sync on
   named Cortex-M and RISC-V boards. Cross-compilation and host simulation
   establish portability of the subset, not hardware qualification.
5. Complete hosted platform CI, source/wheel/npm/WASM consumer installation,
   signed release provenance and independent security and storage review
   before setting a 90++ release score.

## Proposals, not current capabilities

Distributed clustering, broad protocol compatibility layers, a modular edition system, and certified safety-critical use require further implementation and evidence. The standalone Rust browser workspace and transactional AI memory are implemented; their validation scope is recorded in the release guide.
