const assert = require('assert');
const { TapirusDatabase } = require('./index');

async function runTests() {
  console.log('Testing @tapirus/db SDK...');

  // 1. Version check
  assert.strictEqual(TapirusDatabase.version(), '1.0.0');

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
  await db.execute('INSERT INTO users VALUES (1, "Test User");');
  assert.strictEqual(eventFired, true, 'CDC change event should have fired');

  // 5. Unsubscribe
  sub.unsubscribe();

  // 6. Accelerated GraphRAG Query
  const ragResult = await db.graphRagQuery({
    query: 'AI Memory Graph',
    topSeeds: 2,
    maxHops: 2,
    limit: 3
  });
  assert.strictEqual(ragResult.query, 'AI Memory Graph');
  assert.ok(ragResult.results.length > 0, 'Should return GraphRAG entity results');
  assert.ok(ragResult.promptContext.includes('Verified Knowledge Graph Context'), 'Should include synthesized Markdown prompt context');
  assert.strictEqual(ragResult.results[0].hopDistance, 0, 'Seed node should have 0 hop distance');

  // 7. VACUUM INTO
  await db.vacuumInto('backup.tapir');

  db.close();
  console.log('All @tapirus/db tests passed successfully!');
}

runTests().catch((err) => {
  console.error('Test failed:', err);
  process.exit(1);
});
