# Security Policy

Report suspected vulnerabilities privately to [security@tapirusdb.com](mailto:security@tapirusdb.com). Include the affected version, operating system, reproduction steps and impact. Do not include production database files or secret keys in an initial report.

## Supported versions

The current source version is `1.0.5`. Apply the newest patch release available for the version line you deploy. Releases older than `1.0.0` are not covered by this policy.

## Security properties and limits

- The core crate uses `#![forbid(unsafe_code)]`. The separate C FFI crate necessarily contains unsafe boundary code, and dependencies are outside that crate-level lint. Safe Rust prevents classes of memory unsafety; it does not guarantee that a program cannot panic, leak resources or lose data.
- Encrypted pages use XChaCha20-Poly1305 with 256-bit keys and 24-byte nonces. Passphrases are derived with PBKDF2-HMAC-SHA256. The Page 1 header exposes minimal format/KDF metadata (`magic`, `page_size`, `version`, `encryption_flags`, `salt`, `kcv`) needed to bootstrap key derivation and decryption, and those immutable header fields are cryptographically authenticated via Poly1305 AEAD Associated Data (AAD) on Page 1.
- `DatabaseCipher` is non-`Clone` and scrubs its raw key, salt, and epoch fields using `zeroize` when dropped; intermediate PBKDF2/HMAC working buffers are also zeroized after derivation. `Config` implements `zeroize::Zeroize` and redacts secrets in `Debug` output, and `Connection::open*` / `tapirus serve` immediately take and zeroize passphrase/key credentials after initializing `DatabaseCipher` so live connections and clones do not retain plaintext secrets. (`Config` preserves public fields and `Default` struct-update ergonomics; external callers holding a separate `Config` variable before/without opening should call `config.zeroize()` or drop promptly.)
- In Browser/WASM, `TapirusWasm.new_encrypted(passphrase)` and `TapirusWasm.import_encrypted_bytes(bytes, passphrase)` support end-to-end ChaCha20-Poly1305 encrypted in-memory execution and encrypted `export_bytes()` snapshots for OPFS/IndexedDB persistence.
- Embedded connections do not open network listeners by themselves. In current source, the optional `tapirus serve` daemon binds to loopback (`127.0.0.1`) by default, requires an API key for a non-loopback bind, accepts keys only in the `Authorization` or `X-API-Key` header, supports `--api-key-namespace KEY=NS` memory isolation and `--api-key-role KEY=readonly|readwrite[:table1+table2]` AST-enforced SQL RBAC, emits defensive HTTP security headers (`X-Content-Type-Options: nosniff`, `X-Frame-Options: DENY`, `Referrer-Policy: no-referrer`, `Cache-Control: no-store` on JSON), disables cross-origin `Access-Control-Allow-Origin` by default unless explicitly allowlisted via `--cors-origin` or `TAPIRUS_CORS_ORIGIN`, and rejects oversized headers/bodies or excess simultaneous clients. Published binaries older than this build may predate these changes.
- **Native Rustls HTTPS & Reverse Proxy Guidance:** `tapirus serve` supports native TLS 1.2/1.3 HTTPS via `--tls-cert <CERT_PEM>` and `--tls-key <KEY_PEM>` (or `TAPIRUS_TLS_CERT` / `TAPIRUS_TLS_KEY`) backed by `rustls` + `ring` (`--features tls`, enabled in default builds). Without TLS certificates supplied, it serves plaintext HTTP and does not provide HSTS or DDoS/rate limiting; for plaintext setups bind to `127.0.0.1:3005` behind a TLS reverse proxy (for example Caddy, Nginx, Envoy, or a WireGuard/Cloudflare Tunnel). Example Caddyfile:

```caddyfile
db.internal.example.com {
    reverse_proxy 127.0.0.1:3005
}
```
- The daemon uses a synchronous listener and a bounded thread per client. The request limits are 16 KiB of headers, 4 MiB of body, 64 simultaneous clients and 10 seconds to receive a request. It is not a hardened reverse proxy or an asynchronous multi-tenant database server.
- Loaded `.tapmodel` weights are limited to 128 MiB and checked for consistent dimensions and finite values. The default TAP weights are deterministic heuristics, not a validated or calibrated model.
- Authentication does not replace filesystem permissions, encrypted transport, secret management or application-level authorization.
- For cryptographic threat modeling, trust boundaries, and third-party audit scope, see [docs/security/SECURITY_ARCHITECTURE_AND_THREAT_MODEL.md](docs/security/SECURITY_ARCHITECTURE_AND_THREAT_MODEL.md). For the formal WAL crash-safety specification verified by TLC in CI, see [docs/formal_verification/README.md](docs/formal_verification/README.md).

## Vulnerability reports

Send reports to [security@tapirusdb.com](mailto:security@tapirusdb.com) with subject `[SECURITY] TapirusDB Vulnerability Report`. Include:

1. A concise impact description and reproduction steps.
2. The TapirusDB version, compiler/runtime version and operating system.
3. Any suggested mitigation, if available.

Coordinate public disclosure with the maintainers after a fix or mitigation is available.
