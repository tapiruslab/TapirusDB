# Legacy local Node wrapper

This directory is retained for local monorepo compatibility and is marked `private` in `package.json`. It is not the supported published SDK. Use [`sdks/nodejs`](../../sdks/nodejs/README.md) and the `tapirus` package for supported native distributions.

The entry point, TypeScript declarations and regression smoke test are synchronized from `sdks/nodejs` by `node scripts/sync-node-sdk.js`. A matching native addon is required. Set `TAPIRUS_NATIVE_PATH` to a locally built addon when running this wrapper from source.
