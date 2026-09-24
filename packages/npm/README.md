# @tapirus/db

Official TypeScript and JavaScript SDK for **TapirusDB** — The Pure Safe-Rust Embedded Multi-Model AI Database Engine.

```bash
# Install directly from local repository:
npm install ./packages/npm

# Or from NPM registry (when published):
npm install @tapirus/db
```

## Quickstart

```typescript
import { TapirusDatabase } from '@tapirus/db';

async function main() {
  // 1. Open or create database
  const db = await TapirusDatabase.open('app.tapir');

  // 2. Subscribe to real-time table mutations (Reactive Live Queries & CDC)
  const sub = db.subscribe('users', (change) => {
    console.log(`[CDC Event] ${change.op} on table ${change.table}:`, change.data);
  });

  // 3. Create table and execute mutations
  await db.execute('CREATE TABLE users (id INT PRIMARY KEY, name TEXT, bio TEXT);');
  await db.execute('INSERT INTO users VALUES (1, "Ahmad", "Rust Systems Architect");');

  // 4. Hybrid Search with Reciprocal Rank Fusion (RRF)
  const results = await db.hybridSearch({
    queryText: 'Rust systems developer',
    limit: 5,
    rrfK: 60
  });

  // 5. Hot Online Backup without locking active readers
  await db.vacuumInto('backup_snapshot.tapir');

  // Cleanup
  sub.unsubscribe();
  db.close();
}

main();
```
