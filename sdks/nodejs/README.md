# TapirusDB Node.js SDK

New source APIs include upsert, atomic batches, TTL, model identity, revision
checks and indexed retrieval diagnostics. `TapirusSessionDatabase` provides
independently owned transactions over serialized storage. See
[AI and embedded workflows](../../docs/AI_EMBEDDED_WORKFLOWS.md).

This package is a synchronous Node.js binding to the TapirusDB Rust engine. It requires a matching native `.node` addon. It does not switch to an in-memory JavaScript implementation if that addon is missing; opening a database fails with a diagnostic error instead.

## Install and open a database

```sh
npm install tapirus
```

```js
const { open } = require('tapirus');

const db = open('app.tapir');
db.execute('CREATE TABLE IF NOT EXISTS notes (id INTEGER PRIMARY KEY, body TEXT NOT NULL);');
db.execute('INSERT INTO notes VALUES (?, ?);', [1, 'Stored on disk']);
console.log(db.query('SELECT id, body FROM notes WHERE id = ?;', [1]));
db.close();
```

Use `open(':memory:')` when a transient database is intended. `close()` checkpoints the connection and propagates checkpoint errors. For deployments where native artifacts are built locally, set `TAPIRUS_NATIVE_PATH` to the addon file.

## Bound parameters

Use `?` placeholders for values. The SDK sends parameters through the engine's prepared-statement API, so question marks inside quoted SQL strings remain literal text.

```js
db.execute('INSERT INTO notes VALUES (?, ?);', [2, "O'Reilly"]);
const rows = db.query("SELECT '?' AS literal, ? AS bound;", ['value']);
```

Supported JavaScript parameter values are strings, booleans, finite numbers, `bigint`, `null`, numeric arrays for vectors, and `Uint8Array` for blobs. Integers outside JavaScript's safe number range must use `bigint`.

## Vector search and memory

```js
const nearest = db.vectorSearch('notes_with_vectors', 'embedding', [0.1, 0.2, 0.3], 5);
const memoryId = db.remember('The user prefers concise answers', 0.7, ['preference']);
const memories = db.recall('answer style', 5);
```

The vector column must exist on the named table and the query dimension must match its schema. TAP classification, verification, routing and scoring are heuristic engine operations; their scores are not calibrated probabilities.

## Committed change notifications

For model-supplied embeddings, namespace/session filters, transactional source storage and source citations, use `storeMemory`, `searchMemories` and `forgetMemory`. See the [AI memory guide](../../docs/AI_MEMORY.md).

```js
const subscription = db.subscribe('notes', change => {
  console.log(change.op, change.table, change.row_id);
});

db.execute('BEGIN;');
db.execute('INSERT INTO notes VALUES (?, ?);', [3, 'Committed later']);
db.execute('COMMIT;');
subscription.unsubscribe();
```

Notifications are delivered after commit and after the database locks are released. Changes removed by rollback are not delivered. `row_id` is the inserted row ID for a single-row insert; it is `0` when an operation has no single known row ID.

## Native package maintenance

The supported npm package is `tapirus` in `sdks/nodejs`. Its release workflow builds platform addons and runs the persistence smoke test before publishing. The older `packages/npm` wrapper is synchronized for local compatibility and marked private; it is not a second published SDK.

Build and validate locally from the repository root:

```sh
node scripts/build-node-sdk.js --debug
node sdks/nodejs/test.js
node scripts/test-node-package.js
```

The package test installs the generated npm tarball into a fresh consumer directory and checks persistence in a second Node process. SQL integers beyond the safe JavaScript number range are returned as `bigint`.

The wrapper source is declared MIT; the packaged native addon embeds the BUSL-1.1 core. `CORE_LICENSE` and `NOTICE` accompany the binary, and the npm license expression includes both existing component licenses.
