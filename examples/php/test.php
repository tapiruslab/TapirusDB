<?php
declare(strict_types=1);

set_exception_handler(static function (\Throwable $error): void {
    $message = sprintf('%s: %s at %s:%d', get_class($error), $error->getMessage(), $error->getFile(), $error->getLine());
    fwrite(STDERR, $message . PHP_EOL);
    if (getenv('GITHUB_ACTIONS') === 'true') {
        $annotation = str_replace(["%", "\r", "\n"], ["%25", "%0D", "%0A"], $message);
        fwrite(STDERR, "::error file=examples/php/test.php,line=1::{$annotation}" . PHP_EOL);
    }
    exit(255);
});

require __DIR__ . '/Tapirus.php';

function check(bool $condition, string $message): void {
    if (!$condition) throw new RuntimeException($message);
}

$directory = sys_get_temp_dir() . '/tapirus-php-ffi-' . bin2hex(random_bytes(8));
if (!mkdir($directory, 0700)) throw new RuntimeException('Cannot create test directory');
$file = $directory . '/consumer.tapir';
try {
    $db = new Tapirus\Tapirus($file);
    $expected = json_decode(file_get_contents(__DIR__ . '/../../sdks/nodejs/package.json'), true)['version'];
    check(Tapirus\Tapirus::version() === $expected, 'Native version mismatch');
    $db->execute('CREATE TABLE records (id INTEGER PRIMARY KEY, note TEXT)');
    $db->execute("INSERT INTO records VALUES (1, 'O''Reilly')");
    $db->execute('INSERT INTO records VALUES (2, NULL)');
    check($db->query('SELECT * FROM records ORDER BY id') === [['id' => 1, 'note' => "O'Reilly"], ['id' => 2, 'note' => null]], 'SQL/null decoding mismatch');
    check($db->query("SELECT 'Null' AS literal") === [['literal' => 'Null']], 'Literal text must not become SQL NULL');
    try {
        $db->query('INVALID SQL');
        throw new LogicException('Invalid SQL returned success');
    } catch (RuntimeException $expected) {}
    $db->close();
    $db->close();
    try {
        $db->query('SELECT 1');
        throw new LogicException('Closed database returned success');
    } catch (RuntimeException $expected) {}
    $db = new Tapirus\Tapirus($file);
    check(count($db->query('SELECT * FROM records')) === 2, 'File reopen failed');
    $db->close();
    echo 'PHP C FFI: real library, SQL, nulls, errors, close and file reopen passed.' . PHP_EOL;
} finally {
    foreach ([$file, $file . '-wal', $file . '-shm'] as $ownedFile) {
        if (file_exists($ownedFile)) unlink($ownedFile);
    }
    rmdir($directory);
}
