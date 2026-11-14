# TapirusDB Go Client

Official Go HTTP/TLS client (`Go 1.20+`) for connecting to a running `tapirus serve` daemon.

- **Exact 64-Bit Precision:** Decodes numeric SQL columns via `json.Number` so signed 64-bit integers retain exact precision (`Int64()` / `Float64()`).
- **Bound SQL & Multi-Model APIs:** Supports parameterized SQL queries, transaction control, vector similarity search (`int64` IDs), GraphRAG queries, and authenticated API key / TLS endpoints.

## Verification

Run the live daemon integration suite (with `tapirus`, Go, and PHP CLI installed) from the repository root:

```sh
cargo build --locked -p tapirus --bin tapirus
python scripts/test-http-clients.py
```

Running `go test ./...` inside `sdks/go/` verifies unit and protocol coverage. For embedded C-ABI Cgo integration, see `examples/go`.

See [`sdks/go/tapirus.go`](tapirus.go), the [Root README](../../README.md), and the [SDK Index](../README.md).
