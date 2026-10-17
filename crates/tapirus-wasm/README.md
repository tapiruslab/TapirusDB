# TapirusDB WASM Crate

This crate exposes TapirusDB functionality through `wasm-bindgen`. A generated package includes JavaScript glue and the compiled WebAssembly module; the raw `.wasm` file is not a drop-in replacement for that package.

The standalone Rust workspace runs SQL and AI memory through this package. It saves committed snapshots in IndexedDB and opens/exports the native `.tapir` format. The larger React Studio also contains a separate TypeScript demo; use its Rust workspace link for the actual native-format engine.

```sh
rustup target add wasm32-unknown-unknown
# Install wasm-bindgen-cli matching Cargo.lock, then run from the repo root:
node scripts/build-wasm.js
node scripts/build-node-sdk.js --debug
node scripts/test-wasm.mjs
node scripts/test-browser-workspace.mjs
```

`scripts/build-wasm.js` enforces matching wasm-bindgen versions and generates both JavaScript glue and WASM. `WASM_BINDGEN_BIN` can name a local CLI executable. `scripts/package-browser-workspace.js` prepares Studio's standalone Rust workspace; `--site` prepares the local website's Studio entry.

The runtime test checks typed SQL parameters, AI retrieval, WASM reopen, and native SDK → WASM → native file compatibility. The browser test checks bound SQL, rollback, text escaping, IndexedDB reload and persisted source retrieval. Browser copies are unencrypted; use downloaded backups and the native SDK for encrypted files. Resource and performance measurements are workload specific.
