# Contributing to TapirusDB

Thank you for your interest in contributing to **TapirusDB**! We welcome contributions that help push the boundaries of embedded database engineering and AI-native systems.

---

## 🛡️ Core Architectural Invariants (Non-Negotiable)

To maintain enterprise reliability, safety, and performance, all contributions must adhere strictly to these non-negotiable principles:

1. **100% Pure Safe Rust (`#![forbid(unsafe_code)]`)**:
   - The entire codebase is compiled with `#![forbid(unsafe_code)]`.
   - **Zero `unsafe` blocks are permitted under any circumstances.** Any Pull Request introducing `unsafe` code will be automatically rejected.
2. **Single-File Encapsulated Storage (`.tapir`)**:
   - All models (Relational, Documents, Vectors, Graph) must reside atomically within a single `.tapir` file (with transient `.tapir-wal` and `.tapir-shm` frames during active transactions).
   - Never introduce secondary metadata, external state files, or background system daemons.
3. **Sub-Microsecond Latency & Zero Waste**:
   - Critical path execution (e.g. vector search, graph traversal, page lookups) must avoid unnecessary heap allocations and redundant serialization passes.
   - Baseline idle RAM footprint must remain **< 4 MB**.
4. **Crash Safety & Formal Verifiability**:
   - Every on-disk write must be protected by CRC32 checksums and atomic WAL commit records. No partial page writes or silent data corruption.

---

## 🛠️ Development Workflow

### Prerequisites
- **Rust Toolchain:** Rust 1.85 or later (Edition 2024).
- Install via [rustup.rs](https://rustup.rs/):
  ```bash
  rustup update stable
  ```

### Building the Project
```bash
# Debug build
cargo build

# Optimized release binary
cargo build --release
```

### Running Test Suites
TapirusDB includes comprehensive unit, integration, and crash-recovery test suites:
```bash
# Run all tests across workspace
cargo test

# Run tests with SIMD feature enabled
cargo test --features simd
```

### Formatting & Linting
Before submitting your code, ensure it passes all linter and formatting checks:
```bash
# Check formatting
cargo fmt --all -- --check

# Run Clippy with zero-warning enforcement
cargo clippy --all-targets --all-features -- -D warnings
```

### Running Benchmarks
```bash
cargo bench
```

---

## 🔀 Submitting a Pull Request (PR)

1. **Fork the Repository** on GitHub.
2. **Create a Feature Branch**:
   ```bash
   git checkout -b feat/your-feature-name
   ```
3. **Commit Your Changes**:
   Use clear, conventional commit messages:
   - `feat(sql): add support for window functions`
   - `fix(wal): ensure sync before checkpoint completion`
   - `perf(vector): optimize cosine distance inner loop`
   - `docs: improve graph query examples`
4. **Push to Your Fork**:
   ```bash
   git push origin feat/your-feature-name
   ```
5. **Open a Pull Request** against the `master` branch.
   - Include a concise explanation of what the change accomplishes.
   - Attach benchmark numbers or unit test additions covering your code.

---

## 📜 Licensing Notice

By contributing to TapirusDB, you agree that your contributions will be licensed under the **Business Source License 1.1 (BSL-1.1)** converting to **Apache License 2.0** on September 16, 2030, as outlined in the [LICENSE](LICENSE) file.
