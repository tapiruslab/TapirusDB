# Assurance scope and limitations

TapirusDB is an embedded Rust database project. This document records the limits of the assurance evidence in this repository; it is not a safety case, certification, or suitability assessment.

## Rust memory-safety boundary

The main `tapirus` crate forbids explicit unsafe Rust. This is useful compiler-enforced protection against many classes of memory misuse in that crate. It does not prove functional correctness, transaction durability, absence of panics, bounded resource use, dependency correctness, cryptographic suitability, or safety of the host application. The C ABI crate contains unsafe boundary code and needs its own review.

## Storage and recovery

The repository contains WAL and recovery code, regression tests, and a TLA+ specification. The specification has no TLC run output or refinement proof tying it to the Rust implementation. Passing tests provide evidence for the exercised cases only; they do not establish a general crash-safety or zero-data-loss guarantee. Review the current audit and reproduce the durability scenarios for the exact build and storage platform before relying on important data.

## Security and encryption

Page encryption is optional. The presence of XChaCha20-Poly1305 does not by itself establish secure deployment or key lifecycle management. `DatabaseCipher` retains a cloneable key; this repository does not guarantee key zeroization. Configure server bind addresses and API keys for the deployment, and review the server and FFI trust boundaries.

## Functional safety and regulated use

No ISO 26262, IEC 62304, DO-178C, or other functional-safety certification is claimed. This repository does not contain a hazard analysis, safety case, certified development process, or independent assessment that would support such a claim. Do not use this document to qualify the engine for automotive, medical, aerospace, defense, or other safety-critical operation.

## Performance and resource use

Historical benchmark and RAM figures in older documents are not release claims. The audit identified benchmark methodology and comparability problems. Re-run and publish workload-specific measurements before using performance, memory, binary-size, or capacity figures in a design decision.