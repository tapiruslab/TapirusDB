# SDK Stack & Package Validation Matrix

Updated: **14 November 2026** (synchronized at **`1.0.5`**, MSRV **Rust `1.88+`**).

## Unified Source & Package Matrix

| Distribution | Runtime Floor & Source | Packaging & CI Verification |
|---|---|---|
| **Rust Core & Editions** | Rust `1.88+` · `tapirus`, `tapirus-micro`, `tapirus-ffi`, `tapirus-wasm` | `cargo test --locked --workspace --offline` (`355` tests across `59` suites), Rust `1.88.0` `msrv` job, strict Clippy (`-D warnings`), and bare-metal `thumbv7em-none-eabihf` / `riscv32imac-unknown-none-elf` builds. |
| **Python Native Wheel (`tapirus`)** | Python `3.10–3.15` · `sdks/python/` (`PyO3` `abi3`) | Native wheel builds across Linux x64, macOS Intel/ARM64, and Windows x64; tests bound SQL, 64-bit ints, blobs, vectors, `SessionDatabase`, AI scopes, encryption, reopen, plus `tapirus-migrate`, `tapirus-agent`, and `tapirus-eval`. |
| **Node.js Native SDK (`tapirus`)** | Node.js `22 / 24 / 26` · `sdks/nodejs/` (`N-API`) | Cross-OS `.node` addon builds & consumer tarball installation tests covering SQL, `bigint`, vector search, CDC `subscribe`, AI Memory, and multi-process reopen. Legacy `packages/npm` stays synchronized and private. |
| **Browser / WASM (`@tapirus/wasm`)** | WebAssembly (`wasm32-unknown-unknown`) · `crates/tapirus-wasm/` | Built with `wasm-bindgen`; tested in CI via Node tarball consumer + browser IndexedDB persistence + `TapirusWasm.new_encrypted` / `import_encrypted_bytes`. |
| **C ABI / SQLite Shim (`tapirus-ffi`)** | C ABI (`cdylib` / `staticlib`) · `crates/tapirus-ffi/` | Verified `sqlite3_open_v2`, `sqlite3_prepare_v2`, `sqlite3_bind_*`, `sqlite3_step`, `sqlite3_column_*`, `sqlite3_exec`, and AI Memory C ABI roundtrips. |
| **Go Client** | Go `1.20+` · `sdks/go/` | Authenticated HTTP client for `tapirus serve` verified in CI against a live Rustls/API-key daemon. |
| **PHP Client** | PHP `8.0+` (`curl` + `json`) · `sdks/php/` | Authenticated Composer HTTP client verified in CI against a live TapirusDB daemon. |
| **CLI & FFI Release Archives** | Linux x64/ARM64, macOS Intel/ARM64, Windows x64 | Automated `.github/workflows/release.yml` builds native archives with SHA-256 checksums, CycloneDX SBOMs, and GitHub build-provenance attestations. |

## Validation & Bundle Commands

```text
python scripts/check-release-version.py
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets --all-features --offline -- -D warnings
cargo test --locked --workspace --offline
python scripts/check-dependency-advisories.py
python scripts/test-rust-packages.py
node scripts/test-wasm-package.mjs
npm --prefix sdks/nodejs run test:package
maturin sdist --manifest-path sdks/python/Cargo.toml --out target/stack-packages/python
python scripts/normalize-python-sdist.py --sdist target/stack-packages/python --offline --check
python scripts/test-python-sdk.py --wheel target/stack-packages/python
python scripts/test-http-clients.py
python scripts/package-go-sdk.py
```

`scripts/check-release-version.py` enforces exact version parity (`1.0.5`) across Cargo manifests, SDK metadata, examples, and installers prior to release tagging.

See [Current Release Status](CURRENT_STATUS.md) and [Release Validation](RELEASE_VALIDATION.md).
