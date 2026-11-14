# TapirusDB Language SDKs & Bindings

TapirusDB provides native in-process bindings, WebAssembly packages, C-ABI compatibility shims, and authenticated HTTP/TLS daemon clients across major languages, all sharing the interoperable `.tapir` database format (`v1.0.5`).

| Language / Runtime | Source Path | Architecture & Capabilities |
| --- | --- | --- |
| **Rust** | [`src/lib.rs`](../src/lib.rs) | Core `#![forbid(unsafe_code)]` engine: Quad SQL/Vector/Graph/Document, `SessionDatabase` + MVCC `snapshot_reader()`, XChaCha20-Poly1305 AEAD, `MemoryAccessScope`. |
| **Python** | [`sdks/python/`](python/README.md) | Native PyO3 `abi3` wheel (Python `3.10–3.15`) with bound SQL, vectors, `SessionDatabase`, encrypted `.tapir`, plus `tapirus-migrate`, `tapirus-agent` (ONNX), and `tapirus-eval`. |
| **Node.js** | [`sdks/nodejs/`](nodejs/README.md) | Native `N-API` synchronous `.node` addon (Node.js `22 / 24 / 26`) with bound SQL, `bigint`, vector search, CDC `subscribe`, `TapirusSessionDatabase`, and hybrid AI Memory. |
| **Browser / WASM** | [`crates/tapirus-wasm/`](../crates/tapirus-wasm/README.md) | `@tapirus/wasm` WebAssembly Quad engine with SQL, AI Memory, GraphRAG, `.tapir` import/export, and `TapirusWasm.new_encrypted(passphrase)`. |
| **C ABI / SQLite Shim** | [`crates/tapirus-ffi/`](../crates/tapirus-ffi/src/lib.rs) | `cdylib` / `staticlib` exporting `tapirus_*` and `sqlite3_*` (`sqlite3_open_v2`, `sqlite3_prepare_v2`, `sqlite3_bind_*`, `sqlite3_exec`, `sqlite3_step`). |
| **Go** | [`sdks/go/`](go/README.md) | Authenticated HTTP/TLS client for `tapirus serve` with `json.Number` 64-bit integer precision, bound SQL, vector, and GraphRAG endpoints. |
| **PHP** | [`sdks/php/`](php/README.md) | Composer HTTP/TLS client (`PHP 8.0+`, `curl`, `json`) for `tapirus serve` with bound parameters, rollback, vector, and GraphRAG support. |
| **Bare-Metal Flash** | [`crates/tapirus-micro/`](../crates/tapirus-micro/README.md) | `#![forbid(unsafe_code)]` `no_std`, zero-alloc flash slot store + quantized `i8` dot product and offline capsule export (`tapirus-iot`). |

See [Stack Support Matrix](../docs/STACK_SUPPORT.md) and [Current Release Status](../docs/CURRENT_STATUS.md) for CI targets and build commands.
