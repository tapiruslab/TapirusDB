# Engineering Assurance & Verification Architecture

TapirusDB applies compiler-enforced memory safety, authenticated cryptography, secret zeroization, automated fault injection, and formal model checking across its storage and server runtime.

## 1. Compiler-Enforced Memory Safety (`#![forbid(unsafe_code)]`)

- The core `tapirus` engine crate, the bare-metal `tapirus-micro` crate, and the CLI/daemon binary `src/bin/tapirus.rs` strictly forbid `unsafe` Rust at the crate root.
- Raw C-ABI pointers (`sqlite3_*` and `tapirus_*`) are isolated cleanly inside the dedicated boundary crate [`crates/tapirus-ffi`](../crates/tapirus-ffi/src/lib.rs).

## 2. Cryptographic Storage & Secret Lifecycle

- **Per-Page AEAD:** Encrypted databases secure every 4,096-byte page with 256-bit **XChaCha20-Poly1305**, fresh 24-byte CSPRNG `XNonce` values (`getrandom`), and a 16-byte authentication tag.
- **Header Integrity AAD:** `build_page_aad` binds each page index together with Page 1's immutable format and KDF header fields (`magic`, `page_size`, `version`, `encryption_flags`, `salt`, `kcv`) into the Poly1305 authentication tag, blocking page-swapping and header-downgrade tampering.
- **Key Derivation & Constant-Time Checks:** Passphrases are derived with **PBKDF2-HMAC-SHA256** (600,000 default iterations) and verified via `subtle::ConstantTimeEq`.
- **Zeroization & Secret Scrubbing:** `DatabaseCipher` is explicitly non-`Clone` and implements `zeroize::Zeroize` + `Drop` to scrub raw key, salt, and epoch material alongside intermediate PBKDF2/HMAC stack buffers. `Config` implements `Zeroize` with `[REDACTED]` `Debug` formatting, and `Connection::open*` / `tapirus serve` consume and scrub credential fields immediately upon cipher initialization.
- **Browser/WASM Encryption:** `TapirusWasm.new_encrypted(passphrase)` and `TapirusWasm.import_encrypted_bytes(bytes, passphrase)` bring the same XChaCha20-Poly1305 + PBKDF2 storage encryption directly to WebAssembly runtimes.
- **Patched Dependency Baseline:** Rust `1.88+` MSRV keeps `chacha20poly1305 0.11.0`, `ctutils 0.4.8`, and `sha2 0.11.1` free of active production advisories (verified continuously by `scripts/check-dependency-advisories.py` against OSV).

## 3. Zero-Trust Server & Multi-Tenant AI Isolation

- **Native TLS 1.2/1.3:** `tapirus serve` provides built-in Rustls HTTPS (`--tls-cert` / `--tls-key` backed by `ring`), loopback-by-default binding, bounded request headers (`16 KiB`), bodies (`4 MiB`), read deadlines (`10s`), and concurrency (`64` clients), plus defensive security headers and opt-in CORS allowlisting (`--cors-origin`).
- **AST-Enforced SQL RBAC:** `--api-key-role KEY=readonly|readwrite[:table1+table2]` inspects parsed SQL ASTs directly, isolating permitted read/write statements and blocking DDL, `__sys_*` internal table queries, unauthorized `JOIN`/subquery table references, and multi-statement smuggling.
- **Per-Owner AI Memory Boundaries:** `MemoryAccessScope`, `Connection::scoped_memory(namespace, session_id)`, `--api-key-namespace`, and `tapirus mcp` `--namespace`/`--session-id` enforce hard namespace and session isolation across all AI Memory APIs.

## 4. Formal WAL Verification & Crash-Fault Testing

- **TLA+ TLC2 Model Checking:** [TAPIRUS_WAL.tla](formal_verification/TAPIRUS_WAL.tla) specifies Single-Writer WAL transactions, checkpoints, crashes, and recovery, verified in CI by TLC2 (`552,520` states generated, `120,535` distinct states explored, `0` errors) across four invariants: `TypeOK`, `RecoveredDiskMatchesCommitted`, `CheckpointedDiskMatchesCommitted`, and `NoUncommittedCheckpoint`.
- **Fault Injection & Cross-Process Recovery:** Dedicated fault-injection suites (`tests/io_fault_recovery_tests.rs`, `tests/durability_regressions.rs`, `tests/ai_memory_durability_tests.rs`) exercise torn writes, corrupted WAL tails, partial frames, I/O flush failures, and abrupt child-process termination.

See [SECURITY.md](../SECURITY.md) and [Security Architecture & Threat Model](security/SECURITY_ARCHITECTURE_AND_THREAT_MODEL.md) for deployment guidance and third-party audit scope.
