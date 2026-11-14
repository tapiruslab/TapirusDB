# Current Release & Distribution Status

Updated: **14 November 2026**.

## Source & Release Baseline

- **Current source branch:** [main branch](https://github.com/tapiruslab/TapirusDB/tree/main)
- **Current version:** `1.0.5` across the Rust workspace, CLI binaries, C FFI (`tapirus-ffi`), WebAssembly (`@tapirus/wasm`), bare-metal (`tapirus-micro`), Python SDK (`sdks/python`), and Node.js SDK (`sdks/nodejs`).
- **Latest git tag:** [v1.0.5](https://github.com/tapiruslab/TapirusDB/releases/tag/v1.0.5)
- **Rust MSRV:** `1.88+` (enforced by CI `msrv` job on `1.88.0`).

## Distribution Matrix

| Distribution | Package / Target | Location & Runtime | Verification Path |
| --- | --- | --- | --- |
| **Rust Engine (`tapirus`)** | Core Quad + `lite`, `document`, `graph-only`, `vector-only` | `Cargo.toml` · Rust `1.88+` | `cargo test --locked --workspace --offline` (`355` tests across `59` suites) + CI across Linux x86_64/ARM64/Alpine musl, macOS Intel/Apple Silicon, and Windows MSVC |
| **Browser / WASM (`@tapirus/wasm`)** | Encrypted & plain in-browser Quad WASM + `.tapir` interchange | `crates/tapirus-wasm/` | Verified in CI via Node consumer + headless browser IndexedDB persistence; supports `TapirusWasm.new_encrypted(passphrase)` and `import_encrypted_bytes` |
| **Python Native SDK (`tapirus`)** | `PyO3` `abi3` native wheel + CLI tools (`tapirus-migrate`, `tapirus-agent`, `tapirus-eval`) | `sdks/python/` · Python `3.10–3.15` | Multi-OS CI wheel builds + second-process persistence, encryption, and AI scope consumer suite |
| **Node.js Native SDK (`tapirus`)** | Synchronous `N-API` `.node` addon | `sdks/nodejs/` · Node `22 / 24 / 26` | Cross-OS CI consumer tarball installation + parameterized SQL, CDC, vector, and AI memory suite |
| **C ABI / SQLite Shim (`tapirus-ffi`)** | `cdylib` + `staticlib` (`tapirus_*` & `sqlite3_*`) | `crates/tapirus-ffi/` | Verified prepared statements, `sqlite3_bind_*`, `sqlite3_exec`, deferred `sqlite3_step`, and AI Memory C ABI |
| **Bare-Metal Flash (`tapirus-micro`)** | `no_std`, zero-alloc `TPMIC001` slots + `TPIOCAP1` capsules | `crates/tapirus-micro/` | Host flash fault tests + CI cross-compile for `thumbv7em-none-eabihf` and `riscv32imac-unknown-none-elf` |
| **Go & PHP Clients** | Authenticated HTTP clients for `tapirus serve` | `sdks/go/`, `sdks/php/` | Real-daemon Go and PHP integration verification in CI |

## Security, Dependency & Formal Verification Status

- **Dependency Advisory Gate:** `python scripts/check-dependency-advisories.py` checks `139` `Cargo.lock` registry packages against OSV (`0` active production advisories; `chacha20poly1305 0.11.0`, `ctutils 0.4.8`, `sha2 0.11.1`). A single `development-only` dev-dependency entry (`libsqlite3-sys 0.30.1`, `RUSTSEC-2026-0459`) exists strictly in the benchmark comparison harness and is excluded from production builds (`release_blocking: false`).
- **Browser & Native Encryption:** Both native engines and `@tapirus/wasm` (`TapirusWasm.new_encrypted` / `import_encrypted_bytes`) provide 256-bit XChaCha20-Poly1305 page AEAD encryption, PBKDF2-HMAC-SHA256 derivation, Page 1 header AEAD AAD authentication, and `Zeroize` key scrubbing on non-`Clone` `DatabaseCipher`.
- **Formal WAL Verification:** `docs/formal_verification/TAPIRUS_WAL.tla` passes exhaustive **TLC2** model checking in CI (`552,520` generated states, `120,535` distinct states, `0` invariant violations).

See [SECURITY.md](../SECURITY.md), [Security Architecture & Threat Model](security/SECURITY_ARCHITECTURE_AND_THREAT_MODEL.md), [Release Validation](RELEASE_VALIDATION.md), and [Stack Support](STACK_SUPPORT.md) for full details.
