# Release validation — 16 October 2026

This page preserves the 16 October validation snapshot. The 17 October AI,
embedded and indexed memory work is documented in
[AI and embedded workflows](AI_EMBEDDED_WORKFLOWS.md).
The table below must not be treated as a measurement of the later source revision.

The table below records the historical local Windows working tree. A later [hosted CI run for commit `8080302`](https://github.com/tapiruslab/TapirusDB/actions/runs/41013988379) finished successfully on 30 October 2026 with 28 jobs, including Linux x86_64/ARM64/musl, macOS Intel/Apple Silicon, Windows, Rust 1.85, native SDKs, WASM/browser, container persistence and benchmarks. This is CI evidence for that source commit; it is not evidence of a tagged release or published registry packages.

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

On 30 October 2026, the lockfile was updated to `block-buffer 0.12.1` (via `sha2 0.11.0`), `rustls-webpki 0.103.16`, and `webpki-roots 1.0.10`. The local OSV query returned zero findings across 163 registry packages in the updated lockfile. This is a known-advisory check, not a code or container-image security audit. After the update, `cargo test --locked --offline --workspace --all-features`, `cargo clippy --locked --offline --workspace --all-features --all-targets -- -D warnings`, and `cargo fmt --all --check` passed locally on Windows.

A separate local Rust 1.85.0 compilation uncovered trait-object upcasting in the remote pager, which requires a newer compiler. A delegating reader replaced that coercion; `cargo +1.85.0 check --locked --offline -p tapirus --all-targets --all-features` then passed on Windows. CI now repeats the 1.85 compiler check on Linux. This validates compilation, not the entire test suite on the minimum compiler.

## Local Docker smoke check (30 October 2026)

The Linux/amd64 image built from the current working tree on Docker Desktop. `scripts/test-docker-smoke.ps1` checks a non-root process with network disabled and a read-only root filesystem, creates a SQL table, inserts a row, then reopens the same database in a second process and verifies the value. It also requires invalid SQL to return a nonzero CLI exit status. The final runtime uses Alpine 3.24 without an unnecessary `libgcc` package; `docker image ls` reported 16.8 MB for `tapirusdb:local-audit-20261030`. The separate hosted `docker-image` CI job passed in run 41013988379. This is local plus CI container smoke evidence, not cross-architecture validation or proof of a published image. The local test uses an anonymous volume removed when the container exits.

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

The 1.0.2 release workflow now builds the five host/architecture targets on native runners, including Linux ARM64. Each native host installs its own packaged CLI archive using the installer and its generated checksum; standalone edition executables are also launched. A separate final job requires the complete set of 30 archives and 110 assets, validates source/lockfile/build records, dependency inventory and archive contents, uploads a draft, downloads its assets, validates them again, and only then makes the release public. The asset verifier has five local synthetic positive/negative tests, and the edited workflows passed local actionlint 1.7.12. None of this substitutes for an actual hosted run or post-publication download test.

On Windows, a local `--release` CLI and FFI build was packaged into a ZIP with the actual executable, DLL, import library, header, README and license. `release-metadata.py` generated its checksum, source record and a CycloneDX inventory with 168 components. `install.ps1` installed that ZIP with `-NoPath`; the installed CLI reported 1.0.2, wrote a table/row, reopened the same file and returned the stored value. The build record correctly reports a dirty working tree, so this local ZIP is test evidence only and must not be published as a clean release artifact.

Installers default to verified prebuilt assets. Windows also supports `-FromSource`, `-InstallDir`, `-ArchivePath`, `-ChecksumPath` and `-NoPath`. Unix supports `TAPIRUS_FROM_SOURCE=1`, `TAPIRUS_INSTALL_DIR`, `TAPIRUS_ARCHIVE_PATH` and `TAPIRUS_CHECKSUM_PATH`; it prints the shell PATH command. Source installs use a version tag and a locked dependency graph.

## Remaining acceptance work

- The `release-v1.0.2` branch synchronizes the engine, native SDKs, WASM metadata and installer defaults to the 1.0.2 candidate. It is not a published release. Complete the candidate checks and publish matching artifacts before changing production installer defaults.
- A read-only check on 30 October 2026 found no `v1.0.2` tag or GitHub release (`404`). The published `v1.0.1` release lists platform archives but no `.sha256` sidecars required by the current installers. Therefore neither the candidate default nor simply reverting it to 1.0.1 gives a verified public installation path. Publish a validated 1.0.2 release with checksums, then run clean-machine install tests before advertising the installers.
- The candidate's hosted CI passed in run 41013988379. The tag-triggered release build, native packaged-install steps, final 110-asset gate and downloaded-draft recheck still require a successful hosted run before any public release claim.
- Validate signed release artifacts and package installation on clean machines. The local installer test uses a fixture archive rather than a downloaded public release.
- Expand the recorded small-workload release benchmark to realistic datasets, repeated trials, independent hardware, memory measurements and migration workloads.
- Extend I/O fault injection, long-running recovery/restore testing and independent security review. No cryptographic review or safety certification is claimed.
- Evaluate SQL/API compatibility for the intended SQLite migration workload. The database has its own file format and shared-session connection model; full SQLite compatibility has not been established.
- The standalone Rust workspace and the larger React Studio have separate browser state. React Studio's existing TypeScript demo is not a native-format persistence layer.
- Commercial usage, IP ownership and customer/pilot evidence require actual records. This technical validation does not establish an acquisition valuation or a new audit score.
