# Modular Architecture and Extensions: Proposal

> Status: design proposal. The adapters, editions, and extension sandbox described here are goals for future work, not capabilities implied by the current build.

This document records possible ways to modularize TapirusDB. The current repository does not establish PostgreSQL or MongoDB wire-protocol compatibility, a micro-kernel set of independently selectable database editions, or a stable third-party WebAssembly extension API.

## Candidate directions

1. **Protocol adapters:** Explore PostgreSQL or document-protocol adapters only after defining and testing the supported protocol subset, authentication behavior, and client compatibility matrix.
2. **Cargo feature boundaries:** Audit module dependencies and binary size before presenting feature combinations as separate product editions. A Cargo feature name alone does not prove that a module is excluded from a build.
3. **Extensions:** Define a versioned host interface, resource limits, capability model, and lifecycle before claiming that third-party extensions are sandboxed. WebAssembly can be one implementation option, but it does not by itself define those controls.

## Evidence required before calling this implemented

For each proposal, provide a working example, compatibility tests, reproducible build artifact, documented limitations, and CI coverage. Until those exist, use this page as a planning note rather than a product specification.
