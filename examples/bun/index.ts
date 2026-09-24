import { Tapirus } from "./tapirus";

console.log(`🚀 TapirusDB Native Engine v${Tapirus.version()} on Bun`);

// Open an encrypted database or in-memory instance
const db = new Tapirus(":memory:");

try {
  // 1. Relational SQL & AI Vector Column
  db.execute("CREATE TABLE products (id INTEGER PRIMARY KEY, name TEXT, embedding VECTOR(3));");
  db.execute("INSERT INTO products (id, name, embedding) VALUES (1, 'Cyberpunk Helmet', [0.1, 0.8, 0.3]);");
  db.execute("INSERT INTO products (id, name, embedding) VALUES (2, 'Quantum Drive', [0.9, 0.1, 0.0]);");

  // 2. ACID Transactions
  db.execute("BEGIN;");
  db.execute("UPDATE products SET name = 'Cyberpunk Helmet Mark II' WHERE id = 1;");
  db.execute("COMMIT;");

  // 3. Query Clean JSON
  const rows = db.query("SELECT id, name FROM products;");
  console.log("\n📦 Products Table:");
  console.table(rows);

  // 4. Vector Similarity Search
  const vectorResults = db.query(
    "SELECT id, name FROM products VECTOR NEAR embedding = [0.15, 0.75, 0.25] TOP 1;"
  );
  console.log("\n⚡ Top Vector Match (k=1):");
  console.log(vectorResults);
} finally {
  db.close();
  console.log("\n✓ Connection closed safely.");
}
