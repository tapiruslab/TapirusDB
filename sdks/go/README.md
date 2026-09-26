<div align="center">

<img src="https://raw.githubusercontent.com/tapiruslab/TapirusDB/main/assets/icons/go.svg" width="64" height="64" alt="Go Logo" />

# TapirusDB Go SDK

### Official Go Client for TapirusDB — Embedded Quad-Model AI Database & Memory Engine
**Sub-Microsecond Agent Memory • openCypher Knowledge Graphs • HNSW Vector Search • Relational SQL**  
*Single Encrypted `.tapir` Container • 100% Safe Rust Core • < 4 MB Idle RAM • Zero Cloud Daemons*

<br/>

[![Go Reference](https://pkg.go.dev/badge/github.com/tapiruslab/TapirusDB/sdks/go.svg)](https://pkg.go.dev/github.com/tapiruslab/TapirusDB/sdks/go)
[![Go Report Card](https://goreportcard.com/badge/github.com/tapiruslab/TapirusDB/sdks/go)](https://goreportcard.com/report/github.com/tapiruslab/TapirusDB/sdks/go)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg?style=flat-square)](LICENSE)
[![Release: v1.0.0](https://img.shields.io/badge/release-v1.0.0-059669.svg?style=flat-square)](https://github.com/tapiruslab/TapirusDB/releases)
[![Documentation](https://img.shields.io/badge/docs-tapirusdb.com-2b3a7e.svg?style=flat-square)](https://tapirusdb.com/docs.html)

<br/>

</div>

---

## ⚡ Installation

Install via standard Go toolchain:

```bash
go get github.com/tapiruslab/TapirusDB/sdks/go
```

**Requirements:** Go 1.20 or later. Built using pure Go standard library with **zero external dependencies**.

---

## 🚀 Quickstart

```go
package main

import (
	"context"
	"fmt"
	"log"
	"time"

	tapirus "github.com/tapiruslab/TapirusDB/sdks/go"
)

func main() {
	ctx := context.Background()

	// 1. Initialize client
	client := tapirus.NewClient(tapirus.Config{
		Endpoint: "http://127.0.0.1:8080",
		Timeout:  15 * time.Second,
	})

	// 2. Engine health verification
	health, err := client.Health(ctx)
	if err != nil {
		log.Fatalf("Engine offline: %v", err)
	}
	fmt.Printf("Connected: %s (%s, Uptime: %ds)\n", health.Engine, health.Version, health.Uptime)

	// 3. Relational SQL DDL & DML
	_, err = client.Execute(ctx, `
		CREATE TABLE IF NOT EXISTS agents (
			id INTEGER PRIMARY KEY,
			name TEXT NOT NULL,
			model TEXT NOT NULL,
			status TEXT DEFAULT 'idle'
		);
	`)
	if err != nil {
		log.Fatal(err)
	}

	_, err = client.Execute(ctx, "INSERT INTO agents (id, name, model) VALUES (?, ?, ?);", 1, "Agent-Echo", "Phi-3-Mini")
	if err != nil {
		log.Fatal(err)
	}

	// 4. Query Relational Data
	rows, err := client.Query(ctx, "SELECT id, name, model FROM agents WHERE id = ?;", 1)
	if err != nil {
		log.Fatal(err)
	}
	fmt.Println("Agents retrieved:", rows)

	// 5. HNSW Vector Similarity Search (SIMD Cosine / L2)
	queryVector := []float32{0.024, 0.451, -0.892, 0.114}
	matches, err := client.VectorSearch(ctx, "agent_embeddings", queryVector, 3)
	if err != nil {
		log.Fatal(err)
	}
	for _, m := range matches {
		fmt.Printf("Top Match #%d (Score: %.4f)\n", m.ID, m.Score)
	}

	// 6. Knowledge Graph & GraphRAG Traversal
	rag, err := client.GraphRAG(ctx, tapirus.GraphRAGRequest{
		Query: "Explain autonomous consensus algorithm",
		Seeds: 3,
		Hops:  2,
	})
	if err != nil {
		log.Fatal(err)
	}
	fmt.Printf("GraphRAG Extracted: %d knowledge nodes, %d relations\n", len(rag.Nodes), len(rag.Edges))
}
```

---

## 🛠️ API Reference

| Method | Signature | Description |
| :--- | :--- | :--- |
| `NewClient` | `NewClient(cfg ...Config) *Client` | Initializes client with endpoint, token authentication, and timeout. |
| `Health` | `(c *Client) Health(ctx) (*HealthStatus, error)` | Queries server engine version, health state, and uptime. |
| `Query` | `(c *Client) Query(ctx, sql, params...) ([]map[string]interface{}, error)` | Executes SQL queries returning rows as slice of maps. |
| `Execute` | `(c *Client) Execute(ctx, sql, params...) (int64, error)` | Executes DDL/DML statements returning affected row count. |
| `VectorSearch`| `(c *Client) VectorSearch(ctx, collection, vector, k) ([]VectorMatch, error)` | Performs nearest-neighbor vector search using Cosine / L2 distance. |
| `GraphRAG` | `(c *Client) GraphRAG(ctx, req GraphRAGRequest) (*GraphRAGResponse, error)` | Traverses knowledge graph with multi-hop seed extraction. |

---

## 🛡️ License

The TapirusDB Go SDK is open-source software licensed under the [MIT License](LICENSE).  
The underlying TapirusDB core engine is licensed under [BUSL-1.1](https://github.com/tapiruslab/TapirusDB/blob/main/LICENSE).

For complete documentation, benchmarks, and architectural details, visit **[tapirusdb.com](https://tapirusdb.com)**.
