# GraphRAG and Remote Storage: Status Notes

> Status: exploratory notes. This page is not a deployment guide or a compatibility statement for cloud or serverless platforms.

The repository contains graph and vector APIs, plus remote range-storage interfaces and optional cloud-related code. A module or feature flag does not prove that a specific provider, browser, serverless runtime, or large dataset has been validated.

## Before using remote storage

- Build the exact feature set and test authentication, pagination, range semantics, retries, and I/O errors against the provider.
- Measure transferred bytes, cold and warm latency, memory, and failure recovery with the intended database size.
- Confirm that the API and transaction behavior used by the application are supported by the remote path.
- Maintain an independently tested backup and restore plan.

No cold-start, bandwidth, cost, provider-compatibility, or GraphRAG quality figure is promised here. See the current source and release documentation for implementation details.
