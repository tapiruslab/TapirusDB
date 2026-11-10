# Current release status

Last checked: **10 November 2026**. This is a dated public-status snapshot,
not a live registry feed. Recheck the exact package and platform before
installing or making a support commitment.

## Source and release

- Current source: [main branch](https://github.com/tapiruslab/TapirusDB/tree/main).
  Check its commit history for the exact revision; this dated snapshot does not
  pin the moving branch to one hash.
- Current source version: `1.0.3` in the local release candidate. Treat the
  exact tag, workflow result and downloaded assets as separate evidence.
- Public GitHub release: [v1.0.2](https://github.com/tapiruslab/TapirusDB/releases/tag/v1.0.2).
- The current source contains HTTP and TAP hardening beyond the public v1.0.2
  binaries. Do not describe those source changes as shipped until the v1.0.3
  release workflow and downloaded-asset checks are verified.

## Registry snapshot

| Distribution | Observed version or state | Interpretation |
| --- | --- | --- |
| Rust crate `tapirus` | 1.0.1 | Older than the GitHub release; verify before installing. |
| Python package `tapirus` | 1.0.0 | Older than the GitHub release; Python 3.10+ source package is separate. |
| npm package `tapirus` | 1.0.1 | Older than the GitHub release; native addon targets must also match. |
| npm package `@tapirus/db` | 1.0.1 | Legacy public artifact; use `tapirus` for new Node.js installations. |
| npm package `@tapirus/wasm` | Not published | Use the bundled browser workspace or build from source. |
| Composer package | 1.0.2 | HTTP client for a separately running `tapirus serve` daemon. |
| Go module | Source visible on pkg.go.dev; proxy `@latest` needs verification | Do not infer registry publication from source visibility. |

The supported package APIs are not automatically interchangeable. Check the
package README, runtime floor, native target and exact artifact before using a
recipe in production.

## Security and assurance boundary

- The advisory checker reports `RUSTSEC-2026-0459` for `libsqlite3-sys` only
  through the bundled SQLite used by the root crate's `rusqlite` development
  and benchmark dependency. It is retained in the report for visibility but is
  not part of the normal engine dependency path; strict audit mode can still
  fail on it.
- The repository has no cryptographic review, formal refinement proof, safety
  certification, universal performance guarantee or managed database SLA.
- Browser workspace copies are unencrypted. The native SDK is required for
  encrypted files.

See [security policy](../SECURITY.md), [release validation](RELEASE_VALIDATION.md)
and [stack support](STACK_SUPPORT.md) for evidence and limitations.
