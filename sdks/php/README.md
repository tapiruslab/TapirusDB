# TapirusDB PHP client

This Composer library connects over HTTP to `tapirus serve`; it does not bundle
the database engine. It requires PHP 8.0+ with curl and JSON. Use a currently
maintained PHP release for deployment. The separate `examples/php` integration
uses the C shared library through PHP FFI and is not this Composer API.

On 2 November 2026 the HTTP client passed against the actual 1.0.2 daemon on
PHP 8.5.12: bound SQL, signed integers, rollback, auth rejection, invalid SQL,
vectors and GraphRAG. The runner restarted the daemon and verified persisted
rows. Run `python scripts/test-http-clients.py` from the repository root with
Go and PHP CLI/curl installed. No public Composer version is established by
this local result.

This document describes source present in this repository. It does not establish registry publication, package support guarantees, or feature parity with the Rust crate.

The current audit identified API-contract and packaging risks across language bindings. Before relying on this client, inspect [`sdks/php/src/Client.php`](../../sdks/php/src/Client.php) and verify the exact version against a running daemon or native artifact. Test parameter binding, response decoding, write, close, process restart, and reopen behavior.

For implemented core scope and limitations, start with the [root README](../../README.md) and [SDK source index](../README.md).
