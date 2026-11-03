# 🦛 TapirusDB on Go (Golang)

Connect to TapirusDB from Go using idiomatic CGO dynamic linking and C ABI wrappers.

SQL row numbers decode as `json.Number` to preserve signed 64-bit integers.
Convert them explicitly with `Int64()` or `Float64()`. This embedded example is
separate from the HTTP SDK in `sdks/go`. It has no parameter-binding API; avoid
interpolating untrusted values into SQL. `go test ./...` checks null decoding,
exact integers, errors, close and file reopen using the real C shared library.

## 🚀 Features
- **In-Process Performance:** Eliminate network round-trips; query your database directly from Go goroutines.
- **Quad-Model Data Engine:** Combine relational SQL queries with AI vectors and knowledge graph traversal.
- **ChaCha20-Poly1305 Security:** Full SIMD-accelerated AEAD encryption at rest.

## 🛠️ Requirements & Running

Ensure the native shared library is compiled:
```bash
cargo build --release -p tapirus-ffi
```

Run the Go application:
```bash
# On Linux:
export LD_LIBRARY_PATH="../../target/release:$LD_LIBRARY_PATH"
go run main.go

# On macOS:
export DYLD_LIBRARY_PATH="../../target/release:$DYLD_LIBRARY_PATH"
go run main.go

# On Windows:
$env:PATH = "..\..\target\release;$env:PATH"
go run main.go
```
