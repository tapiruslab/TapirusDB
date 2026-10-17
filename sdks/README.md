# Language bindings and SDK source

The repository contains several language integration paths. Their APIs, persistence behavior, packaging, and runtime requirements are not interchangeable or guaranteed to be equivalent.

| Language | Source path | Notes |
|---|---|---|
| Rust | `src/lib.rs` | Main embedded Rust crate. |
| Python | `sdks/python/` | Native PyO3 wheel, Python 3.10+. `python/` is a separate legacy ctypes integration. |
| Node.js | `sdks/nodejs/` | Authoritative native npm package. `packages/npm/` is a synchronized private legacy entry. |
| Go | `sdks/go/` | Check the client request and response shape against the daemon version. |
| PHP | `sdks/php/` | Check the client request and response shape against the daemon version. |

This table documents source locations, not registry availability, support guarantees, or cross-SDK feature parity. For important data, verify write, close, process restart, and reopen behavior using the exact package artifact you plan to ship. See the root [README](../README.md) for core scope and limitations.
