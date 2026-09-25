const tapirus = require('./index.js');
const { Tapirus } = require('./index.js');

const db = tapirus.open(':memory:');
db.execute('CREATE TABLE users (id INT, name TEXT);');
db.execute('INSERT INTO users VALUES (1, "Faiz");');
const rows = db.query('SELECT * FROM users;');
console.log('Node SDK query result (open):', rows);

const db2 = new Tapirus(':memory:');
db2.execute('CREATE TABLE items (id INT, name TEXT);');
db2.execute('INSERT INTO items VALUES (10, "Laptop");');
const rows2 = db2.query('SELECT * FROM items;');
console.log('Node SDK query result (new Tapirus):', rows2);

console.log('NODE SDK SUCCESS!');
