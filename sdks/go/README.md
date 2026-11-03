# TapirusDB Go client

This is an HTTP client for `tapirus serve`, not an embedded Go database. It
forwards bound SQL parameters. JSON numbers in SQL row maps are `json.Number`
so signed 64-bit integers retain precision; convert them explicitly with
`Int64()` or `Float64()` as appropriate. Vector result IDs are typed `int64`.

The 2 November 2026 local test used Go 1.27.3 against the actual 1.0.2 daemon.
It checked health, SQL bindings, rollback, unauthorized requests, errors, vector
and GraphRAG response contracts. A second daemon process reopened the same file
and verified committed rows. Run from the repository root:

```sh
cargo build --locked -p tapirus --bin tapirus
python scripts/test-http-clients.py
```

This combined test also needs PHP CLI with curl. `go test ./...` inside this
directory runs unit/examples; the real-daemon test explicitly skips when its
environment is not provided by the runner. The separate embedded Cgo example
lives in `examples/go` and has a different API.

This document describes source present in this repository. It does not establish registry publication, package support guarantees, or feature parity with the Rust crate.

The current audit identified API-contract and packaging risks across language bindings. Before relying on this client, inspect [`sdks/go/tapirus.go`](../../sdks/go/tapirus.go) and verify the exact version against a running daemon or native artifact. Test parameter binding, response decoding, write, close, process restart, and reopen behavior.

For implemented core scope and limitations, start with the [root README](../../README.md) and [SDK source index](../README.md).
