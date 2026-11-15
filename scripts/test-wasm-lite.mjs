import assert from 'node:assert/strict';
import fs from 'node:fs';
import zlib from 'node:zlib';
import init, { TapirusWasm } from '../crates/tapirus-wasm/pkg-lite/tapirus_wasm_lite.js';

const wasmPath = new URL('../crates/tapirus-wasm/pkg-lite/tapirus_wasm_lite_bg.wasm', import.meta.url);
const wasmBytes = fs.readFileSync(wasmPath);
const gzipBytes = zlib.gzipSync(wasmBytes).length;
await init({ module_or_path: wasmBytes });

assert.equal(TapirusWasm.profile(), 'lite');

let db = new TapirusWasm();
assert.equal(typeof db.store_memory_json, 'undefined');
assert.equal(typeof db.tap_classify, 'undefined');

db.execute('CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT, active INTEGER)');
db.execute('CREATE UNIQUE INDEX idx_active_email ON users(email) WHERE active = 1');
db.execute_params('INSERT INTO users VALUES (?, ?, ?)', JSON.stringify([1, 'ada@tapirus.local', 1]));
db.execute_params('INSERT INTO users VALUES (?, ?, ?)', JSON.stringify([2, 'ada@tapirus.local', 0]));
assert.throws(() => db.execute_params('INSERT INTO users VALUES (?, ?, ?)', JSON.stringify([3, 'ada@tapirus.local', 1])));

const rec = JSON.parse(
  db.query_json('WITH RECURSIVE seq(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM seq WHERE n < 5) SELECT n FROM seq'),
);
assert.deepEqual(rec.map(r => r.n), [1, 2, 3, 4, 5]);

const plainBytes = db.export_bytes();
db.free();

db = TapirusWasm.import_bytes(plainBytes);
assert.equal(JSON.parse(db.query_json('SELECT COUNT(*) AS cnt FROM users WHERE active = 1'))[0].cnt, 1);
db.free();

let enc = TapirusWasm.new_encrypted('browser-secret-2026');
assert.equal(enc.is_encrypted(), true);
enc.execute("CREATE TABLE vault (id INTEGER PRIMARY KEY, secret TEXT)");
enc.execute_params('INSERT INTO vault VALUES (?, ?)', JSON.stringify([1, 'classified']));
const encBytes = enc.export_bytes();
enc.free();

assert.throws(() => TapirusWasm.import_encrypted_bytes(encBytes, 'wrong-secret'));
enc = TapirusWasm.import_encrypted_bytes(encBytes, 'browser-secret-2026');
assert.deepEqual(JSON.parse(enc.query_json('SELECT id, secret FROM vault')), [{ id: 1, secret: 'classified' }]);
enc.free();

console.log(
  `WASM lite (${TapirusWasm.version()}): profile=${TapirusWasm.profile()}, raw=${wasmBytes.length}B, gzip=${gzipBytes}B, SQL + CTE + partial index + encrypted roundtrip passed.`,
);

