# Security Policy

The TapirusDB team is committed to maintaining high standards of data security, cryptographic integrity, and memory safety.

---

## 🛡️ Supported Versions

We provide security updates and patches for the following versions:

| Version | Supported          | Status |
| ------- | ------------------ | ------ |
| `0.1.x` | :white_check_mark: | Active Development & Security Support |
| `< 0.1.0` | :x:              | Deprecated Prototype Releases |

---

## 🔒 Reporting a Vulnerability

If you discover or suspect a security vulnerability in TapirusDB, **please do not disclose it publicly or open a public GitHub issue.**

Please report it privately via email:

* **Email:** [security@tapirusdb.com](mailto:security@tapirusdb.com) or [faiz@tapirusdb.com](mailto:faiz@tapirusdb.com)
* **Subject:** `[SECURITY] TapirusDB Vulnerability Report - <Brief Description>`

### Please Include:
1. Description of the vulnerability and its potential impact.
2. Step-by-step reproduction steps or a minimal reproducible code snippet (`main.rs` or shell script).
3. The specific TapirusDB version, compiler version (`rustc --version`), and target operating system.
4. Any potential mitigations or patch suggestions if available.

### Response Timeline:
* **Initial Acknowledgment:** Within **48 hours**.
* **Triage & Status Update:** Within **5 business days**.
* **Remediation & Advisory Release:** Coordinated disclosure after a fix has been verified and released.

---

## 🏛️ Security Model & Built-In Defenses

TapirusDB is architected from the ground up with defensive security principles:

1. **Zero Unsafe Code (`#![forbid(unsafe_code)]`)**:
   - The engine is compiled under the strictest Rust compiler invariant.
   - Eliminates entire classes of critical vulnerabilities, including buffer overflows, heap use-after-free, double-free, dangling pointers, and data races.
2. **ChaCha20-Poly1305 AEAD Encryption**:
   - High-performance authenticated encryption at rest with 256-bit keys and 128-bit Poly1305 authentication tags.
   - SHA-256 key derivation with 16-byte Key Check Value (KCV) preventing unauthorized database unlocks or ciphertext tampering.
3. **Zero Network Attack Surface (Embedded Mode)**:
   - In standard library and embedded usage, TapirusDB runs directly in-process with zero listening network ports, zero daemons, and zero socket listeners.
4. **Cryptographic Key Sanitization**:
   - Key handles are zeroized in memory upon drop to protect against cold-boot or memory inspection attacks.
