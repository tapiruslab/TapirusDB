const tapirus = require('./index.js');

const db = tapirus.open(':memory:');
db.execute('CREATE TABLE users (id INT, name TEXT);');
db.execute('INSERT INTO users VALUES (1, "Faiz");');
const rows = db.query('SELECT * FROM users;');
console.log('Node SDK query result:', rows);
console.log('NODE SDK SUCCESS!');
