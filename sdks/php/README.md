# TapirusDB PHP Client

Official Composer HTTP/TLS client (`PHP 8.0+` with `curl` and `json`) for `tapirus serve`.

- Supports authenticated API-key and TLS connections, parameterized SQL bindings, `BEGIN` / `COMMIT` / `ROLLBACK` transactions, vector search, and GraphRAG endpoints.
- For embedded in-process C shared library usage via PHP FFI, see `examples/php`.

## Verification

Run the live daemon integration suite from the repository root:

```sh
cargo build --locked -p tapirus --bin tapirus
python scripts/test-http-clients.py
```

The CI `Go and PHP real daemon consumers` job validates SQL parameters, signed integers, rollbacks, auth rejection, vector search, GraphRAG, and daemon reopen persistence on every push.

See [`sdks/php/src/Client.php`](src/Client.php), the [Root README](../../README.md), and the [SDK Index](../README.md).
