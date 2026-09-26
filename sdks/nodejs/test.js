const tapirus = require('./index.js');
const { Tapirus } = require('./index.js');

console.log('Testing Node.js SDK...');

const db = tapirus.open(':memory:');
db.execute('CREATE TABLE users (id INT PRIMARY KEY, name TEXT, score REAL, vec VECTOR(3));');
db.execute('INSERT INTO users VALUES (1, "Alice", 95.5, [0.9, 0.1, 0.0]);');
db.execute('INSERT INTO users VALUES (2, "Bob", 82.0, [0.1, 0.9, 0.0]);');

// 1. Query
const rows = db.query('SELECT id, name, score FROM users ORDER BY score DESC;');
console.log('1. Node SDK query result:', rows);
if (rows.length !== 2 || rows[0].name !== 'Alice') {
  throw new Error('Query failed');
}

// 2. Window Function
const ranked = db.query('SELECT id, name, score, ROW_NUMBER() OVER (ORDER BY score DESC) as rank FROM users;');
console.log('2. Node SDK window function result:', ranked);
if (ranked.length !== 2 || ranked[0].rank !== 1) {
  throw new Error('Window function failed');
}

// 3. Vector Search
const vecMatches = db.vectorSearch('users', 'vec', [0.85, 0.15, 0.0], 1);
console.log('3. Node SDK vector search result:', vecMatches);
if (vecMatches.length !== 1 || vecMatches[0].name !== 'Alice') {
  throw new Error('Vector search failed');
}

db.close();

console.log('NODE.JS SDK TEST PASSED WITH FLYING COLORS (GRADE A)!');
