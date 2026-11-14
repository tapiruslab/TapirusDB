# TapirusDB WASM Crate (`@tapirus/wasm`)

`@tapirus/wasm` compiles the full Safe-Rust TapirusDB Quad engine (`sql`, `vector`, `graph`, `document`, `ai-memory`, and `XChaCha20-Poly1305` encryption) to WebAssembly via `wasm-bindgen`, exposing JavaScript bindings and the compiled `.wasm` binary inside `pkg/`.

## Key Capabilities

- **Native `.tapir` Interchange:** Open, query, mutate, and export portable `.tapir` snapshots directly compatible with the native CLI, Rust core, Python SDK, and Node.js SDK.
- **End-to-End Encrypted Browser Vaults:** Create and restore XChaCha20-Poly1305 encrypted in-memory databases and `.tapir` snapshots directly in JS/WASM using `TapirusWasm.new_encrypted(passphrase)`, `is_encrypted()`, and `TapirusWasm.import_encrypted_bytes(bytes, passphrase)`.
- **Parameterized SQL & Transactions:** Execute bound queries (`execute_params`, `query_params_json`) with `BEGIN`, `COMMIT`, `ROLLBACK`, savepoints, CTEs, triggers, and constraints.
- **Hybrid AI Memory & GraphRAG:** Run `store_memory_json`, `search_memories_json`, `memory_request_json`, `graph_rag_query_json`, `rabitq_quantize_json`, and local TAP inference (`tap_classify`, `tap_verify`) inside the browser or Node/edge runtimes.

## Build & Verify

```sh
rustup target add wasm32-unknown-unknown
# Ensure wasm-bindgen-cli matches Cargo.lock, then run from the repository root:
node scripts/build-wasm.js
node scripts/test-wasm.mjs
node scripts/test-wasm-package.mjs
node scripts/test-browser-workspace.mjs
```

- `scripts/build-wasm.js` generates `@tapirus/wasm` in `crates/tapirus-wasm/pkg/` (set `WASM_BINDGEN_BIN` for a custom CLI path).
- `node scripts/test-wasm-package.mjs` packs `@tapirus/wasm`, installs the tarball offline into a fresh consumer directory, and validates bound SQL, rollback, and snapshot reopen.
- `scripts/package-browser-workspace.js` prepares the standalone browser workspace in `studio/public/native` (`--site` prepares the website Studio entry).
