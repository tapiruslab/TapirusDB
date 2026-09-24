<?php
require_once __DIR__ . '/Tapirus.php';

use Tapirus\Tapirus;

echo "🚀 TapirusDB Native Engine v" . Tapirus::version() . " on PHP " . PHP_VERSION . "\n\n";

$db = new Tapirus(':memory:');

try {
    // 1. Relational Table with AI Vector Embeddings
    $db->execute("CREATE TABLE articles (id INTEGER PRIMARY KEY, title TEXT, embedding VECTOR(3));");
    $db->execute("INSERT INTO articles (id, title, embedding) VALUES (1, 'PHP with TapirusDB', [0.2, 0.8, 0.1]);");
    $db->execute("INSERT INTO articles (id, title, embedding) VALUES (2, 'AI Neural Engines', [0.9, 0.05, 0.05]);");

    // 2. Atomic Transactions
    $db->execute("BEGIN;");
    $db->execute("UPDATE articles SET title = 'Enterprise PHP with TapirusDB' WHERE id = 1;");
    $db->execute("COMMIT;");

    // 3. Query Clean Associative Arrays
    $rows = $db->query("SELECT id, title FROM articles;");
    echo "📦 Query Results:\n";
    print_r($rows);

    // 4. Vector ANN Search
    $vectorMatches = $db->query("SELECT id, title FROM articles VECTOR NEAR embedding = [0.25, 0.75, 0.15] TOP 1;");
    echo "\n⚡ Nearest Vector Match:\n";
    print_r($vectorMatches);

} finally {
    $db->close();
    echo "\n✓ Connection closed safely.\n";
}
