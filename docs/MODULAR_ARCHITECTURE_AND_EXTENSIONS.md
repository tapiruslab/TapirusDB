# Modular Architecture and Extensions: Proposal

> Status: protocol adapters, a trimmed micro-kernel and third-party extension sandbox remain design proposals. First-party edition entry points are described in [Editions](EDITIONS.md).

This document records possible ways to modularize TapirusDB. The current repository provides separately selectable first-party CLI and Rust API entry points for Lite, Document, Graph and Vector, but does not establish PostgreSQL or MongoDB wire-protocol compatibility, fully isolated compile-time engine modules, or a stable third-party WebAssembly extension API.

## Candidate directions

1. **Protocol adapters:** Explore PostgreSQL or document-protocol adapters only after defining and testing the supported protocol subset, authentication behavior, and client compatibility matrix.
2. **Cargo feature boundaries:** Audit module dependencies and binary size before presenting feature combinations as separate product editions. A Cargo feature name alone does not prove that a module is excluded from a build.
3. **Extensions:** Define a versioned host interface, resource limits, capability model, and lifecycle before claiming that third-party extensions are sandboxed. WebAssembly can be one implementation option, but it does not by itself define those controls.

## Evidence required before calling this implemented

For each proposal, provide a working example, compatibility tests, reproducible build artifact, documented limitations, and CI coverage. Until those exist, use this page as a planning note rather than a product specification.
