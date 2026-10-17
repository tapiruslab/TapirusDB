# Contributing to TapirusDB

Contributions should preserve data integrity, make API limits explicit, and include focused regression coverage for behavior changes.

## Boundaries

- The main Rust crate forbids explicit unsafe Rust. The C FFI crate has a separate unsafe boundary; review it independently.
- Do not infer crash safety, full ACID behavior, bounded memory, or stable file-format compatibility from these implementation choices.
- The WAL may use sidecar files. Test writes, close/reopen, rollback, checkpoint, and recovery on the paths your change affects.
- Performance claims need a reproducible benchmark whose result is observable and whose correctness and durability settings match the comparison. There is no accepted idle-RAM or latency threshold at this time.
- Keep SDK behavior explicit. A memory-only helper must not report file persistence, and wrappers must follow the daemon's actual request and response contract.

## Development

Use the workspace toolchain and commands documented by the repository. Common local checks are:

```bash
cargo check --workspace --all-features
cargo test --workspace --all-targets
```

The current CI and host may not provide every optional target or tool. Report the exact command, toolchain, and target when a check cannot run. Add tests for changed behavior; passing tests establish only the scenarios they exercise.

## Pull requests

Describe the behavior changed, relevant data-integrity or security boundary, tests run, and any unverified deployment assumptions. Avoid claims of certification, formal verification, or performance improvement without supporting artifacts.
