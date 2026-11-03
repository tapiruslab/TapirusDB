import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { Tapirus } from './tapirus';

const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'tapirus-bun-ffi-'));
try {
  const file = path.join(directory, 'consumer.tapir');
  let db = new Tapirus(file);
  assert.equal(String(Tapirus.version()), JSON.parse(fs.readFileSync(new URL('../../sdks/nodejs/package.json', import.meta.url), 'utf8')).version);
  db.execute('CREATE TABLE records (id INTEGER PRIMARY KEY, note TEXT)');
  db.execute("INSERT INTO records VALUES (1, 'O''Reilly')");
  db.execute('INSERT INTO records VALUES (2, NULL)');
  assert.deepEqual(db.query('SELECT * FROM records ORDER BY id'), [{ id: 1, note: "O'Reilly" }, { id: 2, note: null }]);
  assert.deepEqual(db.query("SELECT 'Null' AS literal"), [{ literal: 'Null' }]);
  assert.throws(() => db.query('INVALID SQL'));
  assert.throws(() => db.execute('INVALID SQL'));
  db.close();
  db.close();
  assert.throws(() => db.query('SELECT 1'), /closed/);
  db = new Tapirus(file);
  assert.equal(db.query('SELECT * FROM records').length, 2);
  db.close();
  console.log('Bun C FFI: real library, SQL, nulls, errors, close and file reopen passed.');
} finally {
  fs.rmSync(directory, { recursive: true, force: true });
}
