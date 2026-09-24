# 🦛 TapirusDB on PHP

Connect to TapirusDB in PHP 7.4+ or PHP 8.x using native FFI without installing any PECL extensions.

## 🚀 Features
- **Zero PECL Extensions:** Uses PHP's built-in `\FFI::cdef` engine.
- **Embedded Speed:** Query database tables, JSON documents, vectors, and graphs with sub-millisecond execution.
- **Single-File Container:** Entire database stored in `app.tapir` with atomic WAL crash resilience.

## 🛠️ Running the Example

Make sure `ffi.enable=true` is enabled in your `php.ini` (or pass `-d ffi.enable=1` on CLI).

Ensure `libtapirus.so` (or `tapirus.dll` on Windows) is compiled:
```bash
cargo build --release -p tapirus-ffi
```

Run script:
```bash
php -d ffi.enable=1 index.php
```
