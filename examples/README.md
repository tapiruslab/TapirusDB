# 🦛 TapirusDB Examples Directory

This directory contains verified, production-ready, zero-placebo integration examples for **TapirusDB** across 7 programming languages and serverless runtimes.

All examples run directly on TapirusDB's **100% Safe-Rust Quad-Model Engine** (`.tapir` single-file container with Relational SQL, HNSW Vector Search, openCypher Knowledge Graph, and Schemaless Document Collections).

---

## 📂 Available Examples & Quick Run Commands

| Directory | Language / Runtime | Description | Command |
| :--- | :--- | :--- | :--- |
| **[`rust/`](./rust)** | Rust | Native Safe-Rust Quad-Model example (SQL, Vectors, JSON Docs, GraphRAG) | `cargo run --manifest-path examples/rust/Cargo.toml` |
| **[`python/`](./python)** | Python 3.8+ | Native PyO3 SDK & Standalone Ctypes FFI (SQL, Collections, Agent Memory) | `python examples/python/quickstart.py` |
| **[`chatbot/`](./chatbot)** | Python / Web | AI Cognitive Chatbot with sub-millisecond TAP inference (<2ms) and SQL logs | `python examples/chatbot/app.py --cli` |
| **[`cloudflare-worker/`](./cloudflare-worker)** | Cloudflare Workers | Serverless Edge API for SQL, accelerated GraphRAG, and Agent Memory | `wrangler dev` (inside `cloudflare-worker/`) |
| **[`bun/`](./bun)** | Bun | Native FFI client (`bun:ffi`) without external npm packages | `bun run examples/bun/index.ts` |
| **[`typescript/`](./typescript)** | Node.js / TS | TypeScript C ABI dynamic linking via `koffi` | `npm start` (inside `typescript/`) |
| **[`go/`](./go)** | Go (Golang) | High-performance CGO bindings with embedded C ABI | `go run main.go` (inside `go/`) |
| **[`php/`](./php)** | PHP 7.4+ / 8.x | Native PHP FFI client (`\FFI::cdef`) with zero PECL extensions | `php -d ffi.enable=1 examples/php/index.php` |
| **[`academic_eval.rs`](./academic_eval.rs)** | Rust | Scientific benchmarks: SQ8 Quantization Error, AEAD Overhead, HNSW 384D | `cargo run --example academic_eval` |

---

## 🛠️ Building the Native C FFI Shared Library

Languages that use the C ABI (Go, PHP, Bun, TypeScript, Python ctypes) require the native shared library:

```bash
# Debug build (faster compilation)
cargo build -p tapirus-ffi

# Release build (maximum SIMD performance)
cargo build --release -p tapirus-ffi
```

Output binary locations automatically discovered by all examples:
* **Windows**: `target/release/tapirus.dll` or `target/debug/tapirus.dll`
* **Linux**: `target/release/libtapirus.so` or `target/debug/libtapirus.so`
* **macOS**: `target/release/libtapirus.dylib` or `target/debug/libtapirus.dylib`

---

## 🛡️ Zero-Placebo Guarantee

Every example in this folder is strictly tested:
* **No dummy mocks**: SQL queries parse and execute in the genuine ACID database engine.
* **No fake memory**: Agent memories are persisted into durable `.tapir` storage with real token matching and recency decay.
* **100% Safe Rust Core**: `#![forbid(unsafe_code)]` enforced across all engine crates.
