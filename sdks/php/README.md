# TapirusDB PHP SDK

Official PHP Client for [TapirusDB](https://tapirusdb.com) — The Safe-Rust Quad-Model Embedded AI Database.

## Installation

Install via [Composer](https://packagist.org/packages/tapiruslab/tapirusdb):

```bash
composer require tapiruslab/tapirusdb
```

## Quickstart

```php
<?php

require_once 'vendor/autoload.php';

use Tapirus\Client;

$db = new Client('http://127.0.0.1:8080');

// 1. Relational SQL Queries
$db->execute("CREATE TABLE IF NOT EXISTS users (id INT PRIMARY KEY, name TEXT, balance REAL);");
$db->execute("INSERT INTO users VALUES (1, 'Alice', 1450.50);");

$results = $db->query("SELECT * FROM users WHERE balance > 1000;");
print_r($results);

// 2. Vector Search (HNSW)
$matches = $db->vectorSearch('embeddings', [0.045, -0.128, 0.892, 0.231], 5);
print_r($matches);
```

## License

Business Source License 1.1 (BSL-1.1), converting to Apache 2.0.
