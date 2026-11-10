# SDK stack and package validation

## Current public release update (10 November 2026)

[GitHub v1.0.2](https://github.com/tapiruslab/TapirusDB/releases/tag/v1.0.2) is public with 110 listed assets. The 9 November registry snapshot below records Rust 1.0.1, Python 1.0.0, Node 1.0.1 and PHP 1.0.2; these states are separate from the source and GitHub release. The public `@tapirus/db` npm artifact is legacy; new Node.js installations should use `tapirus`. Recheck the exact registry and target platform before installation. The local source candidate is 1.0.3, but its release assets and registry publications are not treated as verified here.

## Source and local validation scope

The source tree targets TapirusDB **1.0.3** across the Rust engine, native SDKs,
WASM package metadata, examples, and installer defaults. This is a local release
candidate. Version agreement does not establish registry publication or support
for every operating system and architecture.

| Distribution | Runtime and source | Local package check | Publication evidence |
| --- | --- | --- | --- |
| Rust crates | Rust 1.85+; `tapirus`, `tapirus-micro`, `tapirus-ffi`, `tapirus-wasm` | Four extracted 1.0.2 crate archives compiled together offline with sibling path patches. This is the last recorded package check and does not simulate installation from crates.io. | Registry snapshot on 9 November found `tapirus` 1.0.1; 1.0.3 publication needs a fresh check. |
| Python wheel | Python 3.10+; `sdks/python/` | A Windows CPython 3.10 ABI wheel installed under Python 3.12 and passed native engine, reopen, and encryption checks. A source archive also exists. Rebuild and test each release target. | Registry snapshot found `tapirus` 1.0.0. The separate `python/` ctypes package is not this distribution. |
| Node.js npm | Node.js 22+; `sdks/nodejs/` | The Windows x64 native addon passed tarball installation in a separate consumer, persistence, binding, vector, constraint, and CDC checks. Other native targets require their own builds. | Registry snapshot found `tapirus` 1.0.1. `packages/npm/` is a private synchronized legacy entry; public `@tapirus/db` is not the supported package. |
| WASM npm | Generated `crates/tapirus-wasm/pkg/` | The 1.0.2 tarball was installed in a separate Node.js consumer and passed SQL parameter, rollback, snapshot reopen, and error checks on this Windows host. This is the last recorded package check; browser persistence is a separate check. | Registry report found no `@tapirus/wasm` package. The source manifest is private. |
| Go module | Go 1.20+; `sdks/go/` | `scripts/package-go-sdk.py` builds a module zip and tests import through a fresh local file proxy. The HTTP client also needs a real TapirusDB daemon for data checks. | Source is visible on pkg.go.dev; the Go proxy `@latest` result needs verification before calling it published. |
| PHP package | PHP 8.0+, JSON and cURL; `sdks/php/` | The local package archive exists. The 2 November daemon test checked the HTTP client on PHP 8.5 and verified data after restart. This host lacks PHP CLI. | Registry snapshot found `tapiruslab/tapirusdb` v1.0.2. |
| CLI/FFI archive | Native release workflow and `crates/tapirus-ffi/` | Release workflow builds on native target hosts, installs each archive, and checks its checksum and SQL persistence. A local candidate covers only its build host. | The public v1.0.2 assets are verified in the dated record; v1.0.3 needs a completed hosted run and verified downloadable assets. |

The **2 November 2026** read-only registry inventory is retained as historical
`target/stack-packages-2026-11-02.json`: 25 dependency registry queries completed
without errors. It recorded no public `v1.0.2` GitHub release. The advisory
report at `target/stack-advisories-2026-11-02.json` checked 141 Cargo.lock registry
packages and found no active advisories at that time. The current advisory
checker records the SQLite finding separately as a development-only dependency
finding; it is not silently ignored. These are dated checks, not a guarantee
about later registry state or package compatibility.

## Validation and bundle commands

Run these from the repository root with the required language toolchains installed:

```text
python scripts/check-release-version.py
cargo fmt --all -- --check
cargo package --workspace --exclude tapirus-nodejs --exclude tapirus-python --locked --offline --no-verify
python scripts/test-rust-packages.py
node scripts/test-wasm-package.mjs
npm --prefix sdks/nodejs run test:package
maturin sdist --manifest-path sdks/python/Cargo.toml --out target/stack-packages/python
python scripts/normalize-python-sdist.py --sdist target/stack-packages/python --offline --check
python scripts/test-python-sdk.py --wheel target/stack-packages/python
python scripts/test-http-clients.py
python scripts/package-go-sdk.py
python scripts/check-dependency-advisories.py
python scripts/check-stack-packages.py --online
python scripts/package-stack-bundle.py --native-archive PATH_TO_VALIDATED_NATIVE_ARCHIVE
```

`package-stack-bundle.py` also requires the local Node.js tarball, native Python
wheel and source archive, Go and PHP zips, WASM tarball, four Rust crate archives,
and matching advisory and online registry reports at their default paths. The native
archive must have a matching build record and SHA-256 sidecar for the current
version, lockfile and source commit. It records
hashes in `BUILD.json` and writes a candidate zip and SHA-256 sidecar. The bundle
does not publish to any registry. Its contents must be rebuilt and tested from
the final source before distribution.

The Rust package consumer uses local path patches for unpublished sibling
crates because Cargo 1.98.1 can report `no hash listed` during a workspace
package verification. It checks the extracted archive sources and preserves
the locked registry dependency versions and checksums. Registry installation
remains a separate acceptance step after publishing.

## Version and release policy

`scripts/check-release-version.py` rejects mixed engine, SDK, example and
installer versions. The public release process still requires the exact tagged
source, passing hosted CI, native target builds, package consumer checks, and
downloaded release asset verification. Check each registry again immediately
before publication. Keep package claims limited to architectures and runtimes
actually tested with the final artifacts. The broader CLI release gates and
benchmark limits are recorded in [release validation](RELEASE_VALIDATION.md).
