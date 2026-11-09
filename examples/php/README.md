# TapirusDB on PHP

Connect to the local FFI example from PHP 8.0+ without installing a PECL
extension. This example is separate from the maintained Composer HTTP client in
`sdks/php/`.

## 🚀 Features
- **Zero PECL Extensions:** Uses PHP's built-in `\FFI::cdef` engine.
- **Embedded access:** Query the APIs implemented by the native FFI surface.
- **Single-file example:** The sample writes `app.tapir`; measure recovery and
  resource use on the target workload before making performance claims.

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
