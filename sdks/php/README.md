<div align="center">

<img src="https://raw.githubusercontent.com/tapiruslab/TapirusDB/main/assets/icons/php.svg" width="64" height="64" alt="PHP Logo" />

# TapirusDB PHP SDK

### Official PHP Client for TapirusDB — Embedded Quad-Model AI Database & Memory Engine
**Relational SQL • HNSW Vector Search • openCypher Knowledge Graph • JSON Documents**  
*Single Encrypted `.tapir` Container • 100% Safe Rust Core • < 4 MB Idle RAM • Zero Cloud Daemons*

<br/>

[![Packagist Version](https://img.shields.io/packagist/v/tapiruslab/tapirusdb.svg?style=flat-square&logo=php)](https://packagist.org/packages/tapiruslab/tapirusdb)
[![PHP Version](https://img.shields.io/packagist/php-v/tapiruslab/tapirusdb.svg?style=flat-square&logo=php)](https://packagist.org/packages/tapiruslab/tapirusdb)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg?style=flat-square)](LICENSE)
[![Docs](https://img.shields.io/badge/docs-tapirusdb.com-2b3a7e.svg?style=flat-square)](https://tapirusdb.com/docs.html)

<br/>

</div>

---

## ⚡ Installation

Install via [Composer](https://packagist.org/packages/tapiruslab/tapirusdb):

```bash
composer require tapiruslab/tapirusdb
```

**Requirements:** PHP 8.0 or later with `ext-json` and `ext-curl`.

---

## 🚀 Quickstart

```php
<?php

require_once __DIR__ . '/vendor/autoload.php';

use Tapirus\Client;

// 1. Initialize client
$db = new Client('http://127.0.0.1:8080');

// 2. Health check
$health = $db->health();
echo "Connected to TapirusDB {$health['version']} ({$health['engine']})\n";

// 3. Relational SQL Queries
$db->execute("
    CREATE TABLE IF NOT EXISTS agents (
        id INT PRIMARY KEY,
        name TEXT NOT NULL,
        model TEXT NOT NULL,
        memory_mb REAL
    );
");

$db->execute("INSERT INTO agents (id, name, model, memory_mb) VALUES (?, ?, ?, ?);", [
    1, 'Echo-Agent', 'Phi-3-Mini', 3.8
]);

$results = $db->query("SELECT * FROM agents WHERE memory_mb < 100;");
print_r($results);

// 4. Vector Similarity Search (HNSW Cosine / L2)
$queryVector = [0.045, -0.128, 0.892, 0.231];
$matches = $db->vectorSearch('agent_embeddings', $queryVector, 5);
print_r($matches);

// 5. Knowledge Graph & GraphRAG Traversal
$rag = $db->graphRAG([
    'query' => 'Explain consensus protocol',
    'seeds' => 3,
    'hops'  => 2,
]);
print_r($rag);
```

---

## 📜 License

The TapirusDB PHP SDK is licensed under the [MIT License](LICENSE).  
The underlying TapirusDB core engine is licensed under [BUSL-1.1](https://github.com/tapiruslab/TapirusDB/blob/main/LICENSE).

For complete documentation, benchmarks, and architectural details, visit **[tapirusdb.com](https://tapirusdb.com)**.
