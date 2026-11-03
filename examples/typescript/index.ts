import tapirus from 'tapirus';

const { open, version } = tapirus;

console.log(`TapirusDB native engine ${version()}`);
const db = open(':memory:');
try {
  db.execute('CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT, embedding VECTOR(3))');
  db.execute('INSERT INTO users VALUES (?, ?, ?)', [1, 'Alex Chen', [0.1, 0.9, 0.0]]);
  db.execute('INSERT INTO users VALUES (?, ?, ?)', [2, 'Sarah Connor', [0.8, 0.2, 0.0]]);
  const rows = db.query<{ id: number; name: string }>('SELECT id, name FROM users WHERE id = ?', [1]);
  if (rows.length !== 1 || rows[0].name !== 'Alex Chen') throw new Error('Bound query failed');
  console.log(rows);
  const nearest = db.vectorSearch<{ id: number }>('users', 'embedding', [0.15, 0.85, 0.0], 1);
  if (nearest.length !== 1 || nearest[0].id !== 1) throw new Error('Vector search failed');
  console.log(nearest);
} finally {
  db.close();
}
