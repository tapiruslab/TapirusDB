'use strict';

const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { spawnSync } = require('node:child_process');
const { open, TapirusSessionDatabase } = require(process.env.TAPIRUS_TEST_PACKAGE || './index.js');

async function main() {
  const reopenPath = process.env.TAPIRUS_TEST_REOPEN_PATH;
  if (reopenPath) {
    const db = open(reopenPath);
    const rows = db.query('SELECT id, body FROM users ORDER BY id;');
    assert.deepEqual(rows.map(row => row.id), [1, 2, 4]);
    assert.equal(rows[1].body, "O'Reilly");
    db.close();
    return;
  }

  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'tapirus-node-sdk-'));
  const databasePath = path.join(directory, 'persistent.tapir');
  try {
    const limitedPath = path.join(directory, 'limited.tapir');
    const limited = open(limitedPath, { memory: { maxDocuments: 2 } });
    limited.storeMemoryBatch([{ content: 'first' }, { content: 'second' }]); limited.close();
    assert.throws(() => open(limitedPath, { memory: { maxDocuments: 1 } }));
    const reopenedLimited = open(limitedPath, { memory: { maxDocuments: 2 } });
    assert.throws(() => reopenedLimited.storeMemory({ content: 'third' })); reopenedLimited.close();
    const db = open(databasePath);
    assert.equal(db.isNative, true);
    db.execute('CREATE TABLE users (id INTEGER PRIMARY KEY, body TEXT NOT NULL, amount INTEGER, vec VECTOR(2));');
    db.execute('INSERT INTO users VALUES (?, ?, ?, ?);', [1, 'Alice', 10, [1, 0]]);

    const changeIds = [];
    const subscription = db.subscribe('users', change => changeIds.push(change.row_id));
    db.execute('BEGIN;');
    db.execute('INSERT INTO users VALUES (?, ?, ?, ?);', [2, "O'Reilly", 20, [0, 1]]);
    db.execute('SAVEPOINT nested;');
    db.execute('INSERT INTO users VALUES (?, ?, ?, ?);', [3, 'rolled back', 30, [1, 1]]);
    db.execute('ROLLBACK TO nested;');
    db.execute('INSERT INTO users VALUES (?, ?, ?, ?);', [4, 'committed', 40, [1, 1]]);
    assert.deepEqual(changeIds, [], 'uncommitted changes must not emit CDC events');
    db.execute('COMMIT;');

    await new Promise(resolve => setTimeout(resolve, 20));
    assert.deepEqual(changeIds, [2, 4], 'only committed single-row insert IDs should be published');
    assert.equal(subscription.unsubscribe(), true);

    const literalAndParameter = db.query("SELECT '?' AS literal, ? AS bound;", ['value']);
    assert.deepEqual(literalAndParameter, [{ literal: '?', bound: 'value' }]);
    assert.equal(db.query('SELECT COUNT(*) AS count FROM users;')[0].count, 3);
    assert.equal(db.query('SELECT ? AS exact;', [9007199254740993n])[0].exact, 9007199254740993n);

    db.execute('BEGIN;');
    db.storeMemory({ content: 'Discard this draft', namespace: 'manuals' });
    db.execute('ROLLBACK;');
    assert.deepEqual(db.searchMemories({ query: 'draft', namespace: 'manuals' }), []);
    const source = db.storeMemory({ content: 'Engine repair guide', embedding: [1, 0], namespace: 'manuals', sessionId: 'workshop', tags: ['source:manual'] });
    const sources = db.searchMemories({ query: 'repair', embedding: [1, 0], namespace: 'manuals', sessionId: 'workshop' });
    assert.equal(sources[0].entry.id, source);
    assert.equal(sources[0].entry.content, 'Engine repair guide');
    assert.ok(sources[0].semantic_score > 0.99);
    assert.deepEqual(db.searchMemories({ query: 'repair', namespace: 'other' }), []);
    const keyed = { content: 'Updated repair manual', externalId: 'manual-1', sourceVersion: 'v1', embedding: [0, 1], embeddingModel: 'sdk/test-v1', namespace: 'lifecycle' };
    const keyedId = db.upsertMemory(keyed);
    assert.equal(db.upsertMemory({ ...keyed, content: 'Revised repair manual', expectedRevision: 1 }), keyedId);
    assert.throws(() => db.upsertMemory({ ...keyed, expectedRevision: 1 }));
    assert.throws(() => db.upsertMemoryBatch([{ ...keyed, externalId: 'new' }, { ...keyed, embedding: [1, 0, 0] }]));
    assert.equal(db.searchMemories({ query: 'repair', namespace: 'lifecycle' }).length, 1);
    assert.equal(db.searchMemoriesDetailed({ embedding: [0, 1], embeddingModel: 'sdk/test-v1', namespace: 'lifecycle' }).results[0].entry.revision, 2);
    assert.ok(db.prepareMemoryIndexes().indexed_vectors >= 2);
    assert.equal(db.storageHealth().reopen_required, false);
    db.storeMemory({ content: 'Expired repair note', expiresAt: 1, namespace: 'expiry' });
    assert.deepEqual(db.searchMemories({ query: 'repair', namespace: 'expiry' }), []);
    assert.equal(db.purgeExpiredMemories(), 1);

    const nearest = db.vectorSearch('users', 'vec', [0, 1], 1);
    assert.equal(nearest[0].id, 2);

    assert.throws(() => db.execute('INSERT INTO users VALUES (?, ?, ?, ?);', [5, null, 50, [0, 0]]));
    assert.equal(db.query('SELECT * FROM users;').length, 3);
    db.close();
    assert.throws(() => db.query('SELECT * FROM users;'), /closed/i);

    const child = spawnSync(process.execPath, [__filename], {
      encoding: 'utf8',
      env: { ...process.env, TAPIRUS_TEST_REOPEN_PATH: databasePath },
    });
    assert.equal(child.status, 0, `second process failed:\n${child.stdout}\n${child.stderr}`);
    const sessions = new TapirusSessionDatabase(path.join(directory, 'sessions.tapir'));
    const a = sessions.session(); const b = sessions.session();
    a.execute('CREATE TABLE session_records (id INTEGER)');
    a.begin(); a.execute('INSERT INTO session_records VALUES (1)');
    assert.throws(() => b.query('SELECT * FROM session_records'), /session|busy/i);
    assert.throws(() => b.commit(), /session|busy/i);
    a.rollback();
    assert.deepEqual(b.query('SELECT * FROM session_records'), []);
    b.transaction(session => { session.execute('INSERT INTO session_records VALUES (2)'); session.upsertMemory({ content: 'session source', externalId: 'source' }); });
    assert.equal(a.memoryStats().documents, 1);
    assert.throws(() => b.transaction(() => { b.execute('INSERT INTO session_records VALUES (3)'); throw new Error('abort'); }), /abort/);
    assert.equal(a.query('SELECT * FROM session_records').length, 1);
    a.close(); b.close(); sessions.close();
    console.log('Node.js native SDK persistence, binding, vector, constraint, and CDC checks passed.');
  } finally {
    fs.rmSync(directory, { recursive: true, force: true });
  }
}

main().catch(error => {
  console.error(error);
  process.exitCode = 1;
});
