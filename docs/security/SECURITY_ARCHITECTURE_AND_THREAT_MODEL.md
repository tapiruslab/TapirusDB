# TapirusDB Security Architecture, Threat Model & External Audit Readiness Dossier

> **Audit Status Notice (Transparent Disclosure):**
> This document is a **first-party engineering security dossier** prepared by the TapirusDB codebase maintainers to define scope, threat boundaries, cryptographic primitives, and reproducible verification checks for an **independent third-party cryptographic and security audit**.
> It is **not** an independent third-party audit report. Independent audit attestation requires engaging an external security firm or unaffiliated cryptographic reviewer to inspect the release tag and publish their own signed findings.

---

## 1. Core Security Guarantees & Architecture

| Security Domain | Mechanism & Standard | Primary Implementation |
| --- | --- | --- |
| **Memory Safety** | `#![forbid(unsafe_code)]` across core library and CLI server | [`src/lib.rs`](../../src/lib.rs), [`src/bin/tapirus.rs`](../../src/bin/tapirus.rs) |
| **Page AEAD Encryption** | XChaCha20-Poly1305 (256-bit key, 24-byte CSPRNG `XNonce`, 16-byte Poly1305 tag per 4,096-byte page) | [`src/crypto/mod.rs`](../../src/crypto/mod.rs) |
| **Header Authentication** | Poly1305 AEAD AAD binds `page_id` + Page 1 immutable security/format fields (`magic`, `page_size`, `version`, `encryption_flags`, `salt`, `kcv`) | [`build_page_aad`](../../src/crypto/mod.rs) |
| **Key Derivation & KCV** | PBKDF2-HMAC-SHA256 (600,000 iterations default) + constant-time `subtle::ConstantTimeEq` KCV verification | [`pbkdf2_hmac_sha256`](../../src/crypto/mod.rs), [`verify_kcv`](../../src/crypto/mod.rs) |
| **Secret Zeroization** | Non-`Clone` `DatabaseCipher` (`Zeroize` + `Drop`), PBKDF2/HMAC working buffers zeroized, `Config` redacts `Debug` and is taken/zeroized immediately on `Connection::open*` & `tapirus serve` | [`DatabaseCipher`](../../src/crypto/mod.rs), [`take_cipher_from_config`](../../src/lib.rs) |
| **Browser / WASM Encryption** | End-to-end encrypted in-memory execution + encrypted `.tapir` snapshot export (`export_bytes`) for OPFS/IndexedDB | [`TapirusWasm::new_encrypted`](../../crates/tapirus-wasm/src/lib.rs) |
| **HTTP Daemon Transport & Auth** | Native Rustls TLS 1.2/1.3 (`--tls-cert`/`--tls-key`, `ring` provider), constant-time API key comparison, namespace-scoped AI memory tokens (`--api-key-namespace`), AST-enforced SQL RBAC (`--api-key-role`), defensive response headers, loopback bind default | [`src/bin/tapirus.rs`](../../src/bin/tapirus.rs) |
| **Dependency Baseline** | MSRV raised to Rust `1.88` to track patched cryptographic dependencies (`chacha20poly1305 0.11`, `ctutils 0.4.8`, `sha2 0.11.1`) verified against OSV | [`Cargo.toml`](../../Cargo.toml), [`scripts/check-dependency-advisories.py`](../../scripts/check-dependency-advisories.py) |
| **Formal WAL Model** | Single-writer WAL crash-safety specification (`TAPIRUS_WAL.tla`) checked by TLC in CI | [`docs/formal_verification/TAPIRUS_WAL.tla`](../formal_verification/TAPIRUS_WAL.tla) |

---

## 2. Threat Model & Trust Boundaries

### In Scope (Protected Assets & Attack Vectors)
1. **Offline Storage Compromise:** Reading or tampering with `.tapir` / `.tapir-wal` files or browser OPFS exports without the passphrase/256-bit key.
   - *Mitigation:* Per-page XChaCha20-Poly1305 AEAD, CSPRNG nonces (`getrandom`), page-index & Page-1-header AAD binding preventing page swapping and header-downgrade tampering.
2. **Memory Residue & Logging Leaks:** Accidental leakage of passphrases/raw keys via `Clone`, debug formatting, or long-lived server/connection state.
   - *Mitigation:* Non-`Clone` `DatabaseCipher`, `zeroize` on cipher, stack key/HMAC buffers, and `Config` credential scrubbing upon open (`[REDACTED]` `Debug`).
3. **Network / Multi-Tenant HTTP Daemon Abuse (`tapirus serve`):** Eavesdropping, SQL injection/DDL escalation, cross-tenant memory leakage, and CORS abuse.
   - *Mitigation:* Native Rustls HTTPS, default loopback binding, opt-in CORS allowlist, constant-time token comparison, AST-level SQL statement/table allowlist authorization (blocking DDL, `__sys_*`, unauthorized `JOIN`/subqueries, and multi-statement `;` smuggling), and namespace-scoped memory connections.

### Out of Scope / Explicit Non-Claims
- **Host / OS Compromise:** An attacker with root/kernel memory read access on the running host while `DatabaseCipher` is unlocked can extract the active page-encryption key from process RAM.
- **SQL Injection Inside Caller Applications:** Embedded API callers must use parameterized bindings (`?` / `query_with_params`) rather than string interpolation.
- **FFI Unsafe Boundary:** `crates/tapirus-ffi` intentionally exposes C-ABI raw pointers (`unsafe` boundary for external callers) and assumes C callers pass valid null-terminated buffers/lifetimes.
- **DDoS / Edge WAF:** `tapirus serve` enforces bounded threads (`64` clients), timeouts (`10s`), and header/body size caps (`16 KiB` / `4 MiB`), but relies on edge proxies/firewalls for network-layer DDoS absorption.

---

## 3. Recommended Scope for an Independent Third-Party Audit

When commissioning an external audit firm (e.g., Trail of Bits, NCC Group, Quarkslab, Cure53, Include Security), prioritize these four workstreams:

1. **Cryptographic Design & Implementation Review (`src/crypto/`, `src/pager/`)**
   - Validate PBKDF2-HMAC-SHA256 RFC 2898 compliance, KCV domain separation, CSPRNG nonce generation, Page 1 header AAD boundary (`0..12`, `48..81` vs mutable counters `12..48`), and WAL frame encryption/recovery edge cases.
2. **HTTP Server Auth, Parser & RBAC Bypass Testing (`src/bin/tapirus.rs`)**
   - Fuzz HTTP header framing, Bearer/`X-API-Key` parsing, Rustls TLS configuration, and AST SQL RBAC (`authorize_sql_statement_ast`) for bypasses via CTEs, views, subqueries, or vector search paths.
3. **Multi-Process WAL Locking & Crash Consistency (`src/pager/mod.rs`, `src/pager/wal.rs`)**
   - Audit `.tapir-lock` coordination, fingerprint validation, crash recovery fault injection (`tests/io_fault_recovery_tests.rs`), and correspondence between `TAPIRUS_WAL.tla` and Rust code.
4. **C FFI & WASM Boundary Fuzzing (`crates/tapirus-ffi/`, `crates/tapirus-wasm/`)**
   - Fuzz C-ABI pointer/length preconditions (`sqlite3_*` compatibility layer) and WASM encrypted snapshot import/export paths.

---

## 4. Publishing an External Audit Report

Once an external audit completes:
1. Place the auditor's signed PDF/Markdown report under `docs/security/reports/<YYYY>-<firm>-report.pdf` (with SHA-256 checksum).
2. Record the audited Git commit/tag SHA, remediation commits, and auditor scope note in `SECURITY.md` and `README.md`.
