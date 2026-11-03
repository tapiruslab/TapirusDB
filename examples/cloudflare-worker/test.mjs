// Use the exact Workers simulator shipped with the installed Wrangler version.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';
const require = createRequire(import.meta.url);
const wrangler = createRequire(require.resolve('wrangler/package.json'));
const { Miniflare, convertV4MiniflareOptions } = wrangler('miniflare');
const bundle = fileURLToPath(new URL('../../target/cloudflare-bundle/', import.meta.url));
const runtime = new Miniflare(convertV4MiniflareOptions({
  modulesRoot: bundle,
  modules: [
    { type: 'ESModule', path: path.join(bundle, 'worker.js') },
    ...fs.readdirSync(bundle).filter(name => name.endsWith('.wasm')).map(name => ({ type: 'CompiledWasm', path: path.join(bundle, name) })),
  ],
  compatibilityDate: '2026-11-02',
  bindings: { API_KEY: 'local-consumer-test' },
}));
const post = (route, body, token = 'local-consumer-test') => runtime.dispatchFetch(`http://localhost${route}`, {
  method: 'POST', headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${token}` }, body: JSON.stringify(body),
});
const success = async (route, body) => {
  const response = await post(route, body);
  const result = await response.json();
  assert.ok(response.ok, JSON.stringify(result));
  return result;
};
try {
  const health = await (await runtime.dispatchFetch('http://localhost/health')).json();
  assert.equal(health.engine, 'TapirusDB WASM');
  assert.equal(health.persistence, 'ephemeral per isolate');
  assert.equal((await post('/sql', { query: 'SELECT 1' }, 'wrong')).status, 401);
  await success('/sql', { query: 'CREATE TABLE records (id INTEGER PRIMARY KEY, note TEXT)' });
  await success('/sql', { query: 'INSERT INTO records VALUES (?, ?)', params: [1, "quote ' ? ;"] });
  assert.deepEqual((await success('/sql', { query: 'SELECT * FROM records WHERE id = ?', params: [1] })).rows, [{ id: 1, note: "quote ' ? ;" }]);
  assert.equal((await post('/sql', { query: 'INVALID SQL' })).status, 400);
  await success('/memory/remember', { content: 'Replace the maintenance filter', tags: ['manual'] });
  const recalled = await success('/memory/recall', { query: 'maintenance filter' });
  assert.match(JSON.stringify(recalled), /Replace the maintenance filter/);
  assert.equal((await success('/graph-rag', { query: 'maintenance' })).query, 'maintenance');
  console.log('Workers local workerd: actual Rust WASM, health, auth, bound SQL, errors, memory and GraphRAG passed.');
} finally {
  await runtime.dispose();
}
