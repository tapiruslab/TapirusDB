const assert = require('assert');
const { TapirusDatabase } = require('./index');

async function runTests() {
  console.log('Testing @tapirus/db SDK...');

  // 1. Version check
  assert.ok(TapirusDatabase.version().startsWith('1.0'));

  // 2. In-memory open
  const db = await TapirusDatabase.openInMemory();

  // 3. Reactive Subscription & CDC
  let eventFired = false;
  const sub = db.subscribe('users', (change) => {
    assert.strictEqual(change.table, 'users');
    assert.strictEqual(change.op, 'INSERT');
    eventFired = true;
  });

  // 4. Execution
  await db.execute('CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT, embedding VECTOR(3));');
  await db.execute('INSERT INTO users VALUES (1, "Test User", [0.1, 0.9, 0.0]);');
  assert.strictEqual(eventFired, true, 'CDC change event should have fired');

  // 5. Query verification
  const rows = await db.query('SELECT * FROM users;');
  assert.strictEqual(rows.length, 1);
  assert.strictEqual(rows[0].name, 'Test User');

  // 6. Vector search verification
  const vecMatches = await db.query('SELECT * FROM users VECTOR NEAR embedding = [0.15, 0.85, 0.0] TOP 1;');
  assert.strictEqual(vecMatches.length, 1);
  assert.strictEqual(vecMatches[0].id, 1);

  // 7. Agent Memory verification
  const memId = await db.remember('Agent preference: loves concise Safe-Rust code');
  assert.ok(memId > 0);
  const promptCtx = await db.recallPrompt('Safe-Rust');
  assert.ok(promptCtx.includes('concise Safe-Rust code'));

  // 8. Unsubscribe
  sub.unsubscribe();

  // 9. Accelerated GraphRAG Query
  const ragResult = await db.graphRagQuery({
    query: 'Safe-Rust',
    limit: 3
  });
  assert.strictEqual(ragResult.query, 'Safe-Rust');
  assert.ok(ragResult.results.length > 0, 'Should return GraphRAG entity results');

  // 10. VACUUM INTO
  await db.vacuumInto('backup.tapir');

  db.close();
  console.log('All @tapirus/db tests passed successfully!');
}

runTests().catch((err) => {
  console.error('Test failed:', err);
  process.exit(1);
});
