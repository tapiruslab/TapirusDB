# Security Policy

Report suspected vulnerabilities privately to [security@tapirusdb.com](mailto:security@tapirusdb.com). Include the affected version, operating system, reproduction steps and impact. Do not include production database files or secret keys in an initial report.

## Supported versions

The current source version is `1.0.4`. Apply the newest patch release available for the version line you deploy. Releases older than `1.0.0` are not covered by this policy.

## Security properties and limits

- The core crate uses `#![forbid(unsafe_code)]`. The separate C FFI crate necessarily contains unsafe boundary code, and dependencies are outside that crate-level lint. Safe Rust prevents classes of memory unsafety; it does not guarantee that a program cannot panic, leak resources or lose data.
- Encrypted pages use XChaCha20-Poly1305 with 256-bit keys and 24-byte nonces. Passphrases are derived with PBKDF2-HMAC-SHA256. The database header exposes format metadata needed to open the file.
- `DatabaseCipher` is non-`Clone` and scrubs its raw key, salt, and epoch fields using `zeroize` when dropped; intermediate PBKDF2/HMAC working buffers are also zeroized after derivation. Note that passphrases or raw key arrays passed in caller-owned `Config` structs remain under application lifetime control, so callers should clear or drop secrets promptly after opening a connection.
- Embedded connections do not open network listeners by themselves. In current source, the optional `tapirus serve` daemon binds to loopback (`127.0.0.1`) by default, requires an API key for a non-loopback bind, accepts keys only in the `Authorization` or `X-API-Key` header, disables cross-origin `Access-Control-Allow-Origin` by default unless explicitly allowlisted via `--cors-origin` or `TAPIRUS_CORS_ORIGIN`, and rejects oversized headers/bodies or excess simultaneous clients. Published binaries older than this build may predate these changes.
- **Reverse Proxy & TLS Requirement:** `tapirus serve` serves **plaintext HTTP only** and does not implement native TLS termination, HSTS, or DDoS/rate limiting. For any non-loopback, team, staging, or production deployment, bind TapirusDB to `127.0.0.1:3005` and terminate HTTPS/TLS 1.3 at a reverse proxy (for example Caddy, Nginx, Envoy, or a WireGuard/Cloudflare Tunnel) before forwarding traffic with an API key. Example Caddyfile:

```caddyfile
db.internal.example.com {
    reverse_proxy 127.0.0.1:3005
}
```
- The daemon uses a synchronous listener and a bounded thread per client. The request limits are 16 KiB of headers, 4 MiB of body, 64 simultaneous clients and 10 seconds to receive a request. It is not a hardened reverse proxy or an asynchronous multi-tenant database server.
- Loaded `.tapmodel` weights are limited to 128 MiB and checked for consistent dimensions and finite values. The default TAP weights are deterministic heuristics, not a validated or calibrated model.
- Authentication does not replace filesystem permissions, encrypted transport, secret management or application-level authorization.

## Vulnerability reports

Send reports to [security@tapirusdb.com](mailto:security@tapirusdb.com) with subject `[SECURITY] TapirusDB Vulnerability Report`. Include:

1. A concise impact description and reproduction steps.
2. The TapirusDB version, compiler/runtime version and operating system.
3. Any suggested mitigation, if available.

Coordinate public disclosure with the maintainers after a fix or mitigation is available.
