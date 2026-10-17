import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { createRequire } from 'node:module';
import init, { TapirusWasm } from '../crates/tapirus-wasm/pkg/tapirus_wasm.js';

await init({ module_or_path: fs.readFileSync(new URL('../crates/tapirus-wasm/pkg/tapirus_wasm_bg.wasm', import.meta.url)) });
let db = new TapirusWasm();
db.execute('CREATE TABLE documents (id INTEGER PRIMARY KEY, content TEXT NOT NULL, embedding VECTOR(2));');
db.execute_params('INSERT INTO documents VALUES (?, ?, ?);', JSON.stringify([1, "O'Reilly guide", [1, 0]]));
assert.deepEqual(JSON.parse(db.query_json('SELECT id, content FROM documents;')), [{ id: 1, content: "O'Reilly guide" }]);
assert.equal(JSON.parse(db.query_json('SELECT COUNT(*) AS count FROM documents;'))[0].count, 1);
assert.deepEqual(JSON.parse(db.query_params_json("SELECT '?' AS literal, ? AS bound;", '["value"]')), [{ literal: '?', bound: 'value' }]);
db.store_memory_json(JSON.stringify({ content: 'Engine maintenance guide', embedding: [1, 0], namespace: 'manuals', session_id: 'one' }));
const search = { query: 'maintenance', embedding: [1, 0], namespace: 'manuals', session_id: 'one' };
assert.equal(JSON.parse(db.search_memories_json(JSON.stringify(search)))[0].entry.content, 'Engine maintenance guide');
const request = (operation, value = {}) => JSON.parse(db.memory_request_json(operation, JSON.stringify(value)));
const document = { content: 'Updated maintenance source', external_id: 'manual-1', namespace: 'lifecycle', embedding: [0, 1], embedding_model: 'wasm-fixture-v1' };
const id = request('upsert', document);
assert.equal(request('upsert', { ...document, expected_revision: 1, content: 'Revised maintenance source' }), id);
assert.throws(() => request('upsert', { ...document, expected_revision: 1 }));
assert.throws(() => request('store_batch', [{ ...document, external_id: 'new' }, { ...document, external_id: 'wrong', embedding: [1, 0, 0] }]));
assert.equal(request('search_detailed', { query: 'maintenance', namespace: 'lifecycle' }).results.length, 1);
assert.equal(request('prepare').indexed_vectors, 2);
assert.equal(request('storage_health').reopen_required, false);
const bytes = db.export_bytes();
assert.equal(Buffer.from(bytes.slice(0, 8)).toString(), 'TAPIRUS\0');
assert.throws(() => TapirusWasm.import_bytes_with_memory_config(bytes, '{"max_documents":1}'));
const limitedImport = TapirusWasm.import_bytes_with_memory_config(bytes, '{"max_documents":2}');
assert.throws(() => limitedImport.store_memory_json('{"content":"third"}')); limitedImport.free();
db.free();
db = TapirusWasm.import_bytes(bytes);
assert.equal(JSON.parse(db.query_json('SELECT * FROM documents;')).length, 1);
assert.equal(JSON.parse(db.search_memories_json(JSON.stringify(search))).length, 1);
assert.equal(JSON.parse(db.memory_request_json('search_detailed', JSON.stringify({ query: 'maintenance', namespace: 'lifecycle' }))).results[0].entry.revision, 2);
db.free();

// Round trip the same physical .tapir file through the native Rust addon.
const require = createRequire(import.meta.url);
const { open } = require('../sdks/nodejs');
const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'tapirus-wasm-native-'));
try {
  const file = path.join(directory, 'roundtrip.tapir');
  fs.writeFileSync(file, bytes);
  const native = open(file);
  assert.equal(native.query('SELECT * FROM documents;').length, 1);
  native.execute('INSERT INTO documents VALUES (?, ?, ?);', [2, 'Native roundtrip', [0, 1]]);
  native.close();
  const imported = TapirusWasm.import_bytes(fs.readFileSync(file));
  assert.equal(JSON.parse(imported.query_json('SELECT * FROM documents ORDER BY id;'))[1].content, 'Native roundtrip');
  imported.free();
} finally {
  fs.rmSync(directory, { recursive: true, force: true });
}
console.log('Real Rust WASM: SQL, bindings, AI retrieval, and native .tapir roundtrip passed.');
