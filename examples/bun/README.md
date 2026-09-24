# 🦛 TapirusDB on Bun

Blazing fast embedded multi-model AI database running in **Bun** via zero-overhead native `bun:ffi` without any npm packages!

## ⚡ Why Bun + TapirusDB?
- **Zero npm Dependencies:** Uses Bun's native FFI engine directly.
- **Microsecond Execution:** Direct pointer communication with TapirusDB's 100% Safe Rust core.
- **Full Quad-Model Persistence:** SQL, JSON documents, vectors, and property graph inside a single `.tapir` container.

## 🛠️ Build & Run

Ensure the native shared library is compiled:
```bash
cargo build --release -p tapirus-ffi
```

Run with Bun:
```bash
bun run index.ts
```
