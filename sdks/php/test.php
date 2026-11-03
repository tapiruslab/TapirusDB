<?php
declare(strict_types=1);
require __DIR__ . '/src/Client.php';

function check(bool $condition, string $message): void {
    if (!$condition) throw new RuntimeException($message);
}

$endpoint = getenv('TAPIRUS_TEST_ENDPOINT');
if (!$endpoint) throw new RuntimeException('Run scripts/test-http-clients.py to start the real daemon');
$client = new Tapirus\Client($endpoint, getenv('TAPIRUS_TEST_TOKEN'));
check($client->health()['version'] === getenv('TAPIRUS_TEST_VERSION'), 'Health version mismatch');
$client->execute('CREATE TABLE php_consumer (id INTEGER PRIMARY KEY, note TEXT, big INTEGER)');
$note = "quote ' ? ; -- source";
$insert = $client->execute('INSERT INTO php_consumer VALUES (?, ?, ?)', [1, $note, 9007199254740993]);
check($insert['affected'] === 1, 'Affected row count mismatch');
$rows = $client->query('SELECT * FROM php_consumer WHERE id = ?', [1]);
check(count($rows) === 1 && $rows[0]['id'] === 1 && $rows[0]['note'] === $note && $rows[0]['big'] === 9007199254740993, 'Bound parameters or integer precision lost');
$client->execute('BEGIN');
$client->execute("INSERT INTO php_consumer VALUES (2, 'rolled back', 0)");
$client->execute('ROLLBACK');
check(count($client->query('SELECT * FROM php_consumer')) === 1, 'Rollback failed');
try {
    $client->query('INVALID SQL');
    throw new LogicException('Invalid SQL returned success');
} catch (RuntimeException $expected) {}
try {
    (new Tapirus\Client($endpoint))->query('SELECT 1');
    throw new LogicException('Unauthorized SQL returned success');
} catch (RuntimeException $expected) {}
try {
    $client->query('SELECT ?', [NAN]);
    throw new LogicException('Nonfinite JSON parameter returned success');
} catch (JsonException $expected) {}
$client->execute('CREATE TABLE php_vectors (id INTEGER PRIMARY KEY, embedding VECTOR(2))');
$client->execute('INSERT INTO php_vectors VALUES (?, ?)', [1, [1, 0]]);
$matches = $client->vectorSearch('php_vectors', [1, 0], 1);
check(count($matches) === 1 && $matches[0]['id'] === 1, 'Vector response mismatch');
check($client->graphRag('maintenance')['query'] === 'maintenance', 'GraphRAG response mismatch');
echo 'PHP HTTP: real daemon, bindings, exact integers, rollback, auth, errors, vectors and GraphRAG passed.' . PHP_EOL;
