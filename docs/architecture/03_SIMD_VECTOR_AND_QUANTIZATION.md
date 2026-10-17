# Vector Search and Quantization: Implementation Notes

> Status: describes available source modules. Performance, recall, and accuracy depend on workload and have not been established by the source code alone.

## Available components

TapirusDB includes HNSW index code, vector distance functions, optional `wide`-based vector kernels behind the `simd` Cargo feature, scalar 8-bit quantization, product quantization utilities, and a RaBit-style bit-packed quantizer using deterministic sign flips and a normalized Walsh-Hadamard transform.

These components are not a single guaranteed execution path. The query planner, index configuration, feature flags, and calling API determine which implementation is used. Verify the selected path for the build you ship.

## Distance functions

The vector module provides cosine distance, squared Euclidean distance, and dot-product operations. The `simd` feature uses fixed-width vector operations through the `wide` crate; builds without it use scalar loops. Runtime CPU dispatch, a particular instruction set, and a fixed number of cycles are not guaranteed by these APIs.

## Quantization

`QuantizedVector8` stores an 8-bit representation with min/step metadata. `RaBitQuantizer` rotates a padded vector with deterministic sign flips and a Walsh-Hadamard transform, then stores sign bits and the original norm. The current distance estimate uses the stored sign bits and a rotated query.

The packed representation can reduce vector payload size. Actual total memory includes graph edges, IDs, index structures, allocator overhead, and any unquantized vectors retained by the caller. Quantization also changes similarity estimates; measure recall and ranking quality on a representative dataset before relying on it.

## HNSW and persistence

The repository includes in-memory HNSW indexes and a paged-vector storage primitive. These should not be described as a fully disk-resident index: the paged store maintains page payloads in an in-memory map, and separate snapshot code handles persistence. Measure process memory and reopen behavior for the exact API and workload you use.

## Benchmarking

No latency, throughput, compression ratio, or recall figure in this document is a release guarantee. A useful benchmark records the source revision, compiler and flags, CPU, dataset, index parameters, warm or cold state, query mix, recall, and memory. It should consume benchmark results observably so the optimizer cannot remove the measured work.
