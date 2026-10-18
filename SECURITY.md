# Security Policy

Report suspected vulnerabilities privately to [security@tapirusdb.com](mailto:security@tapirusdb.com). Include the affected version, operating system, reproduction steps and impact. Do not include production database files or secret keys in an initial report.

## Supported versions

The current source version is `1.0.2`. Apply the newest patch release available for the version line you deploy. Releases older than `1.0.0` are not covered by this policy.

## Security properties and limits

- The core crate uses `#![forbid(unsafe_code)]`. The separate C FFI crate necessarily contains unsafe boundary code, and dependencies are outside that crate-level lint. Safe Rust prevents classes of memory unsafety; it does not guarantee that a program cannot panic, leak resources or lose data.
- Encrypted pages use XChaCha20-Poly1305 with 256-bit keys and 24-byte nonces. Passphrases are derived with PBKDF2-HMAC-SHA256. The database header exposes format metadata needed to open the file.
- `DatabaseCipher` stores key bytes in a cloneable fixed-size array. The current implementation does not guarantee that key material is zeroized when the cipher is dropped. Applications should limit process access and protect passphrases accordingly.
- Embedded connections do not open network listeners by themselves. The optional `tapirus serve` daemon exposes network endpoints; configure its bind address and API key for the deployment environment. Do not expose an unauthenticated daemon to an untrusted network.
- Authentication does not replace filesystem permissions, encrypted transport, secret management or application-level authorization.

## Vulnerability reports

Send reports to [security@tapirusdb.com](mailto:security@tapirusdb.com) with subject `[SECURITY] TapirusDB Vulnerability Report`. Include:

1. A concise impact description and reproduction steps.
2. The TapirusDB version, compiler/runtime version and operating system.
3. Any suggested mitigation, if available.

Coordinate public disclosure with the maintainers after a fix or mitigation is available.
