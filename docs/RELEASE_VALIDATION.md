# Release validation — 16 October 2026

This page preserves the 16 October validation snapshot. The 17 October AI,
embedded and indexed memory work is documented in
[AI and embedded workflows](AI_EMBEDDED_WORKFLOWS.md).
The table below must not be treated as a measurement of the later source revision.

This records the local Windows working tree. The release and SDK workflows require automated verification before publishing; new workflow definitions have been parsed locally, and hosted runs are still required for the other platform targets.

## Evidence collected

| Check | Result |
|---|---|
| `cargo test --workspace --all-targets --offline` | 297 passed, zero failed. Two ignored tests are subprocess helpers invoked by crash-recovery tests. |
| `cargo clippy --workspace --all-targets --all-features --offline -- -D warnings` | Passed after correcting the original 67 diagnostics. |
| `cargo fmt --all -- --check` | Formatting is enforced by CI; repository Rust sources have been normalized. |
| Native Node runtime | SQL parameters, exact large integers, vector search, constraints, committed CDC, AI source retrieval and second-process file reopen passed. |
| npm consumer installation | Generated tarball installed offline in a separate consumer directory, then the native SDK tests passed. |
| Native Python wheel consumer | Actual abi3 wheel installed offline in a fresh directory. SQL typed parameters, exact integers, blobs, vectors, AI scopes/rollback, collections, close, encrypted files and second-process reopen passed on Python 3.12.10. |
| Python source distribution | Maturin's archive retained unused workspace lock entries. Packaging now prunes 17 entries while retaining registry versions/checksums and all other archive member bytes; the extracted source compiles with `--locked --offline`. |
| Known dependency advisories | OSV checked 159 Cargo.lock registry packages, including optional/dev/other-target dependencies; zero matching known advisories after the updates. This is a dated advisory check, not proof of exploitability or absence of unknown flaws. |
| Actual Rust WASM runtime | Bound SQL, plain JSON rows, AI retrieval, reopen and native SDK ↔ WASM `.tapir` interchange passed. |
| Headless Edge browser | SQL, rollback, HTML text escaping, IndexedDB reload and persisted hybrid source retrieval passed. The report is written to `target/browser-workspace-results.json`. |
| Windows installer | Local archive installed into an isolated directory under `target`; incorrect checksum was rejected before installation. User PATH was not modified by the test. |
| Release metadata | Fixture archive SHA-256 and dependency references passed; the updated inventory matched the official CycloneDX 1.5 schema structure with 163 dependency components. String format checks (URI/email) were skipped by the local validator. |

The new AI durability tests deliberately terminate a child process without database destructors. Committed source insertion, association/deletion and exclusion of an uncommitted source are checked after reopening. Mixed SQL/AI savepoint and rollback tests also verify the reopened state.

## Retrieval and SQL measurements

`cargo bench --bench release_readiness --offline` compares checked SQL results and reopens both databases after writes. It uses TapirusDB WAL commits and SQLite 3.46.0 WAL with `synchronous=FULL`. A deterministic in-memory HNSW workload compares exact cosine neighbors over 1,000 vectors of 32 dimensions and 100 queries, k=10. Runtime assertions reject recall below 90%.

The initial fixture exposed a reverse-edge truncation defect: HNSW recall@10 was only 4.5%. Ranking reverse edges by distance corrected that fixture to **99.9%**. Regression tests additionally use independent query vectors with two index seeds, late insertions, snapshot restore, selective allow-lists, heavy deletion and index rebuild. These fixtures do not establish recall for other datasets or production scales.

| Local release measurement | TapirusDB | SQLite |
|---|---:|---:|
| Warm point read p50 / p95 / p99 | 4.9 / 14.6 / 18.6 microseconds | 18.8 / 28.7 / 56.5 microseconds |
| Autocommit insert p50 / p95 / p99 | 3.482 / 4.117 / 5.855 milliseconds | 2.979 / 3.238 / 3.496 milliseconds |
| Batch of 1,000 inserts including commit | 12.145 milliseconds | 6.872 milliseconds |

HNSW p50 / p95 / p99 was 81.9 / 114.3 / 204.2 microseconds at 99.9% recall@10. This is one small workload on Windows x86_64 (AMD64 Family 25 Model 116 Stepping 1), Rust 1.98.1, default features, optimized release with fat LTO. Parsing and result assertions are included; embedding-model cost is excluded. Reads were faster here; writes were slower. These observations are not general speed or SQLite compatibility claims. `target/release-readiness-results.json` records settings, sample counts, source HEAD/dirty status, binary hash and Cargo.lock hash. The working tree is not a tagged release.

## Dependency changes

The native Python binding moved from PyO3 0.20.3 to 0.29.3, with Python 3.10 as the new minimum. The old dependency falls in ranges described by [RustSec's PyO3 advisories](https://rustsec.org/packages/pyo3.html); the migration includes typed Bound APIs and an explicit GIL requirement. Rustls moved from 0.23.45 to 0.23.46 after [four October 2026 advisories](https://rustsec.org/packages/rustls.html) were found in the locked graph. Inclusion in a vulnerable package range does not establish that a particular affected API was reachable in this application.

`python scripts/check-dependency-advisories.py` writes `target/dependency-advisories.json` and fails for nonwithdrawn findings or service errors. It sends public registry package names and versions to OSV. CI requires this check before release verification can succeed.

## Build and packaging

```sh
node scripts/build-node-sdk.js --debug
node scripts/test-node-package.js
node scripts/build-wasm.js
node scripts/test-wasm.mjs
node scripts/test-browser-workspace.mjs
node scripts/package-browser-workspace.js
maturin build --manifest-path sdks/python/Cargo.toml --locked --out target/python-wheels
python scripts/test-python-sdk.py --wheel target/python-wheels
maturin sdist --manifest-path sdks/python/Cargo.toml --out target/python-sdist
python scripts/normalize-python-sdist.py --sdist target/python-sdist --check
```

The WASM build needs `wasm32-unknown-unknown` and `wasm-bindgen-cli` matching `Cargo.lock`. Set `WASM_BINDGEN_BIN` for a local CLI path. Browser validation uses `BROWSER_BIN` when Chrome/Edge is not at its default path.

The standalone Rust browser workspace is prepared in `studio/public/native`. `node scripts/package-browser-workspace.js --site` prepares the local website's Studio entry and records engine/glue hashes in `engine-manifest.json`. Neither command deploys a public site.

Release packaging creates a SHA-256 file beside each archive, a source/compiler/lockfile build record, and a CycloneDX dependency inventory. The inventory covers Cargo's resolved all-feature dependency graph, including packages not linked into a particular binary; license fields reflect dependency declarations. Tagged release workflows additionally request GitHub build-provenance attestations. These new artifacts must be generated by the actual release run before being described as published or signed.

Installers default to verified prebuilt assets. Windows also supports `-FromSource`, `-InstallDir`, `-ArchivePath`, `-ChecksumPath` and `-NoPath`. Unix supports `TAPIRUS_FROM_SOURCE=1`, `TAPIRUS_INSTALL_DIR`, `TAPIRUS_ARCHIVE_PATH` and `TAPIRUS_CHECKSUM_PATH`; it prints the shell PATH command. Source installs use a version tag and a locked dependency graph.

## Remaining acceptance work

- Choose and synchronize a new release version/tag before publishing these changes. The current working tree still uses 1.0.1 and is not a newly published release.
- Run the new hosted CI jobs on Linux/macOS/musl, Node versions and release targets; local Windows results do not establish those outcomes.
- Validate signed release artifacts and package installation on clean machines. The local installer test uses a fixture archive rather than a downloaded public release.
- Expand the recorded small-workload release benchmark to realistic datasets, repeated trials, independent hardware, memory measurements and migration workloads.
- Extend I/O fault injection, long-running recovery/restore testing and independent security review. No cryptographic review or safety certification is claimed.
- Evaluate SQL/API compatibility for the intended SQLite migration workload. The database has its own file format and shared-session connection model; full SQLite compatibility has not been established.
- The standalone Rust workspace and the larger React Studio have separate browser state. React Studio's existing TypeScript demo is not a native-format persistence layer.
- Commercial usage, IP ownership and customer/pilot evidence require actual records. This technical validation does not establish an acquisition valuation or a new audit score.
