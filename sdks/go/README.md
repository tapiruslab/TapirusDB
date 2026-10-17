# TapirusDB Go client

This document describes source present in this repository. It does not establish registry publication, package support guarantees, or feature parity with the Rust crate.

The current audit identified API-contract and packaging risks across language bindings. Before relying on this client, inspect [`sdks/go/tapirus.go`](../../sdks/go/tapirus.go) and verify the exact version against a running daemon or native artifact. Test parameter binding, response decoding, write, close, process restart, and reopen behavior.

For implemented core scope and limitations, start with the [root README](../../README.md) and [SDK source index](../README.md).
