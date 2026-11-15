# Release Validation & Verification Summary

Updated: **15 November 2026** (Source version **`1.1.0`**, MSRV **Rust `1.88+`**).

## Core Verification Gate

| Verification Gate | Command / Pipeline | Observed Result |
| --- | --- | --- |
| **Full Rust Workspace Suite** | `cargo test --locked --workspace --offline` | **367 passed, 0 failed** across **61 test runs** (`2` ignored subprocess crash helpers invoked by durability suites) |
| **Formatter & Strict Lints** | `cargo fmt --all -- --check` & `cargo clippy --locked --workspace --all-targets --all-features --offline -- -D warnings` | **Passed** (`exit 0`, zero warnings across workspace including `tapirus-ffi` and `tapirus-wasm`) |
| **MSRV Compiler Check** | `cargo +1.88.0 check --locked --workspace --all-targets --all-features` | **Passed** in CI (`msrv` job) |
| **TLA+ Formal WAL Check** | `tlc2.TLC -workers auto -config docs/formal_verification/TAPIRUS_WAL.cfg docs/formal_verification/TAPIRUS_WAL.tla` | **Passed** in CI (`552,520` states generated, `120,535` distinct states, `0` invariant violations) |
| **OSV Dependency Advisories** | `python scripts/check-dependency-advisories.py` | **223** `Cargo.lock` packages checked — **0 active production findings** (`libsqlite3-sys 0.30.1` `RUSTSEC-2026-0459` scoped `development-only`, `release_blocking: false`) |
| **Edition & Target Builds** | `edition_profiles` (`5 passed, 0 failed`), `tapirus-micro`, `@tapirus/wasm` (`2,034,651` B), `@tapirus/wasm-lite` (`1,225,340` B raw / `422,497` B gzip) | **All passed** (`exit 0`); CI also cross-compiles `thumbv7em-none-eabihf` & `riscv32imac-unknown-none-elf` |
| **Native SDK Consumer Suites** | Python `3.10–3.15` wheels, Node `22/24/26` tarballs, `@tapirus/wasm`, `@tapirus/wasm-lite`, Go & PHP real-daemon runners | Verified locally (`Node` + `WASM` + `WASM-lite`) and across multi-OS CI runners |

## Scoped Benchmark Baseline (`release_readiness`)

`cargo bench --bench release_readiness --offline` validates returned SQL rows, full database reopen persistence (`both_databases_reopened_with_1100_rows = true`), and HNSW top-10 cosine neighbor recall against exact brute-force ground truth (`1,000` vectors, `32` dimensions, `100` queries, WAL `synchronous=FULL` parity against SQLite `3.46.0` WAL):

| Measurement (`Windows x86_64`, fat LTO, `rustc 1.98.1`) | TapirusDB `v1.1.0` | SQLite 3.46.0 WAL |
| --- | ---: | ---: |
| Warm point read (`p50 / p95 / p99`) | **5.4 / 9.9 / 19.5 us** | 18.6 / 22.8 / 31.2 us |
| Autocommit insert (`p50 / p95 / p99`) | **3.267 / 8.916 / 18.298 ms** | 3.300 / 5.397 / 13.120 ms |
| Batch 1,000 SQL inserts + commit | **4.179 ms** | 6.091 ms |
| HNSW exact-neighbor search (`p50 / p95 / p99`) | **81.8 / 127.7 / 236.0 us** | — |
| HNSW top-10 Recall (`1,000 x 32D`, `k=10`) | **99.8%** | — |

Raw run metadata (source commit, compiler, flags, dataset sizes, sample counts, and `Cargo.lock` SHA-256) are emitted to `target/release-readiness-results.json`.

## Cryptography & Packaging Pipeline

- **Supply-Chain Hygiene:** MSRV `1.88` tracks patched `chacha20poly1305 0.11.0`, `ctutils 0.4.8`, `sha2 0.11.1`, and `rustls 0.23` (`ring` + `webpki-roots`).
- **Release Artifacts & Attestation:** `.github/workflows/release.yml` produces multi-architecture CLI/FFI archives with SHA-256 sidecars, CycloneDX SBOMs, source build metadata, and GitHub build-provenance attestations, and validates installed archives prior to publishing.

See [Current Release Status](CURRENT_STATUS.md), [Stack Support](STACK_SUPPORT.md), and [Engineering Assurance Architecture](HIGH_ASSURANCE_SAFETY.md).
