# TapirusDB v1.2.1 — Independent Third-Party Security Audit RFP & Evidence Pack

> **Honest Status Notice:**
> This file is a maintainer-prepared **Request for Proposal (RFP) & Audit Scope Brief** to commission an independent third-party security audit firm (for example: Trail of Bits, NCC Group, Quarkslab, Cure53, or Include Security).
> An AI assistant or maintainer cannot sign or certify a third-party audit report. No external signed PDF exists in [`docs/security/reports/`](reports/) until an unaffiliated security firm completes an engagement and issues their signed deliverable.

---

## 1. Engagement Overview

- **Target Repository:** `https://github.com/tapiruslab/TapirusDB`
- **Target Release Baseline:** `v1.2.1` (Phases 1–9, MSRV Rust `1.88+`)
- **Primary Architecture Reference:** [`SECURITY_ARCHITECTURE_AND_THREAT_MODEL.md`](SECURITY_ARCHITECTURE_AND_THREAT_MODEL.md)
- **Safety Invariant:** `#![forbid(unsafe_code)]` across `src/` (`tapirus` core crate & CLI daemon); explicit FFI `unsafe` isolated to `crates/tapirus-ffi/`

---

## 2. In-Scope Attack Surfaces for v1.2.1

Auditors should review the entire storage, cryptography, daemon, and WebAssembly surface, paying close attention to the `v1.1.0`–`v1.2.1` additions:

1. **Cryptography, AEAD Framing & Zeroization (`src/crypto/mod.rs`, `src/pager/`)**
   - Page-level XChaCha20-Poly1305 (256-bit key, 24-byte `getrandom` nonce, 16-byte tag)
   - Page 1 immutable header Poly1305 AEAD AAD binding (`0..12`, `48..81`) vs mutable counters (`12..48`)
   - PBKDF2-HMAC-SHA256 (`600,000` iterations) + constant-time `subtle::ConstantTimeEq` KCV check
   - Non-`Clone` `DatabaseCipher`, `Config::zeroize()` credential scrubbing on open, and Browser/WASM encrypted `.tapir` snapshot export/import
2. **AST SQL RBAC & Expanded SQL Parser/Executor (`src/bin/tapirus/http.rs`, `src/sql/parser.rs`, `src/sql/executor/`, `src/sql/datetime.rs`)**
   - `authorize_sql_statement_ast` enforcement under `--api-key-role KEY=readonly|readwrite[:tables]` (including table allowlists on subqueries inside `SELECT`, `WHERE`, `HAVING`, `CTEs`, and `UPDATE SET`)
   - Expanded SQL AST & execution branches: `WITH RECURSIVE` CTEs (recursion depth cap `10,000`), `Statement::Compound` (`UNION` / `UNION ALL` / `INTERSECT` / `EXCEPT` with trailing `ORDER BY` / `LIMIT` / `OFFSET`), correlated & scalar subqueries with memoization, `CROSS JOIN`, `CASE WHEN` / `IIF` / `CAST`, `GENERATED ALWAYS AS (...)` computed columns, partial & expression indexes (`CREATE [UNIQUE] INDEX ... (expr) WHERE ...`), `ON CONFLICT DO UPDATE SET` (`EXCLUDED.<col>` expressions), and pure Safe-Rust UTC date/time evaluation (`src/sql/datetime.rs`)
   - Prevention of DDL escalation, `__sys_*` catalog/memory table access, multi-statement `;` smuggling, and resource exhaustion via recursive CTEs or nested subqueries
3. **Batch SQL Insert Fast Path & B+Tree CoW / Zero-Copy Scans (`src/sql/executor/`, `src/btree/engine.rs`, `src/lib.rs`)**
   - Pre-validated transaction insert fast path bypassing 4 KB per-row savepoint cloning when no triggers, secondary indexes, `CHECK`, `FOREIGN KEY`, `UNIQUE` constraints, generated columns, temporal history, FTS, or vector columns exist — verify atomicity/constraint parity and error-rollback safety
   - Zero-copy `btree.scan_with_ref(...)` cursor loops and multi-key `IN` / `OR` index union scans
4. **Browser WASM Profiles (`crates/tapirus-wasm/`)**
   - Full Quad (`@tapirus/wasm`) and SQL/Crypto-only (`@tapirus/wasm-lite`) exports, bound parameters (`execute_params`, `query_params_json`), and `TapirusWasm::new_encrypted` / `import_encrypted_bytes`
5. **Lakehouse / Spreadsheet Importers & HTTP/MCP Daemon (`src/lakehouse/`, `src/bin/tapirus/http.rs`, `src/bin/tapirus/mcp.rs`)**
   - Native Rustls TLS 1.2/1.3 (`--tls-cert`, `--tls-key`), `--api-key-namespace`, CORS allowlist, HTTP size/concurrency caps, plus `.xlsx` / `.parquet` / `.sqlite` / `.pdf` input bounds checking

---

## 3. Reproducible Pre-Audit Verification Gates

An auditor can reproduce the baseline CI & formal verification checks locally on the tagged commit:

```sh
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets --all-features --offline -- -D warnings
cargo test --locked --workspace --offline
python scripts/check-dependency-advisories.py
python scripts/check-release-version.py --tag v1.2.1
```

Formal WAL crash-safety specification (checked in CI `formal-verification` job):
- `docs/formal_verification/TAPIRUS_WAL.tla` + `TAPIRUS_WAL.cfg`

---

## 4. Expected Deliverables Upon Engagement

1. Signed PDF security assessment report (placed under `docs/security/reports/<YYYY>-<firm>-v1.2.1.pdf` alongside `.sha256`).
2. Severity-classified findings (Critical / High / Medium / Low / Informational) mapped to audited commit SHA and remediations.
3. Public attestation summary linked in `SECURITY.md` and `docs/RELEASE_VALIDATION.md`.

