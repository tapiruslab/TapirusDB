# 🦛 TapirusDB on Go (Golang)

Connect to TapirusDB from Go using idiomatic CGO dynamic linking and C ABI wrappers.

## 🚀 Features
- **In-Process Performance:** Eliminate network round-trips; query your database directly from Go goroutines.
- **Quad-Model Data Engine:** Combine relational SQL queries with AI vectors and knowledge graph traversal.
- **ChaCha20-Poly1305 Security:** Full hardware-accelerated encryption at rest.

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
