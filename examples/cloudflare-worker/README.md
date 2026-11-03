# Cloudflare Workers WASM example

Uses the actual Rust WASM module and matching JavaScript glue. Data lives only in
the current Worker isolate; isolate eviction, restart or another isolate loses
or separates it. Durable Objects, R2 persistence and production deployment have
not been validated by this example.

```sh
# From the repository root, with matching wasm-bindgen-cli installed:
node scripts/build-wasm.js
cd examples/cloudflare-worker
npm install
npm run build
npm run dev
```

For local data endpoints, provide `API_KEY` in an untracked `.dev.vars` file
(`API_KEY=your-local-key`). Use `wrangler secret put API_KEY` when deliberately
deploying. `/health` is public; POST endpoints require `Authorization: Bearer
<API_KEY>`. The SQL body is `{ "query": "SELECT ? AS value", "params": [42],
"operation": "query" }`. Operations are synchronous and do not call an embedding
model. The demo does not implement authentication between multiple tenants.

`npm run build` bundles locally with Wrangler; it does not publish. A passing
bundle is distinct from a real Workers runtime or persistence test. See the
[stack matrix](../../docs/STACK_SUPPORT.md) for validation evidence.
