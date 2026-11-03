// Ephemeral per-isolate demo. Workers cannot load native Node .node addons.
import { initSync, TapirusWasm } from '../../crates/tapirus-wasm/pkg/tapirus_wasm.js';
import wasmModule from '../../crates/tapirus-wasm/pkg/tapirus_wasm_bg.wasm';

initSync({ module: wasmModule });
let database;
function getDatabase() {
  database ??= new TapirusWasm();
  return database;
}
const json = (value, status = 200) => Response.json(value, { status });

export default {
  async fetch(request, env) {
    const path = new URL(request.url).pathname;
    if (path === '/health' && request.method === 'GET') {
      return json({ status: 'online', engine: 'TapirusDB WASM', version: TapirusWasm.version(), persistence: 'ephemeral per isolate' });
    }
    if (request.method !== 'POST' || !['/sql', '/graph-rag', '/memory/remember', '/memory/recall'].includes(path)) {
      return json({ error: 'Endpoint not found' }, 404);
    }
    if (!env.API_KEY) return json({ error: 'Configure the API_KEY Worker secret before using data endpoints' }, 503);
    if (request.headers.get('Authorization') !== `Bearer ${env.API_KEY}`) return json({ error: 'Unauthorized' }, 401);
    let body;
    try {
      body = await request.json();
    } catch {
      return json({ error: 'Expected a JSON object' }, 400);
    }
    if (!body || typeof body !== 'object' || Array.isArray(body)) return json({ error: 'Expected a JSON object' }, 400);
    try {
      const db = getDatabase();
      if (path === '/sql') {
        const { query, params = [] } = body;
        if (typeof query !== 'string' || !query.trim() || !Array.isArray(params)) return json({ error: 'Expected query string and params array' }, 400);
        const operation = body.operation ?? (/^\s*SELECT\b/i.test(query) ? 'query' : 'execute');
        if (operation === 'query') {
          const rows = JSON.parse(db.query_params_json(query, JSON.stringify(params)));
          return json({ rows, count: rows.length });
        }
        if (operation !== 'execute') return json({ error: 'operation must be query or execute' }, 400);
        return json({ affected: db.execute_params(query, JSON.stringify(params)) });
      }
      if (path === '/memory/remember') {
        if (typeof body.content !== 'string' || !body.content.trim()) return json({ error: 'Expected nonempty content' }, 400);
        if (!Array.isArray(body.tags ?? []) || !(body.tags ?? []).every(tag => typeof tag === 'string' && !tag.includes(','))) return json({ error: 'tags must be strings without commas' }, 400);
        const memoryId = db.memory_remember(body.content, body.importance ?? 0.5, (body.tags ?? []).join(','));
        return json({ memoryId: memoryId.toString() }, 201);
      }
      if (typeof body.query !== 'string' || !body.query.trim()) return json({ error: 'Expected nonempty query' }, 400);
      if (path === '/memory/recall') return json({ memories: JSON.parse(db.memory_recall_rrf_json(body.query, body.limit ?? 5, 60)) });
      return json(JSON.parse(db.graph_rag_query_json(body.query, body.topSeeds ?? 3, body.maxHops ?? 2, body.limit ?? 5)));
    } catch (error) {
      return json({ error: String(error.message ?? error) }, 400);
    }
  },
};
