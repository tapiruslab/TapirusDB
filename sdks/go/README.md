# TapirusDB Go SDK

Official Go client for **TapirusDB** — the 100% Safe-Rust Embedded Quad-Model AI Database Engine (Relational SQL, Vectors, GraphRAG, and Document Store).

---

## ⚡ Installation

Install via standard Go toolchain:

```bash
go get github.com/tapiruslab/TapirusDB/sdks/go
```

Requires Go 1.20 or later. Zero external third-party dependencies.

---

## 🚀 Quickstart

```go
package main

import (
	"context"
	"fmt"
	"log"

	tapirus "github.com/tapiruslab/TapirusDB/sdks/go"
)

func main() {
	ctx := context.Background()

	// 1. Connect to local or remote TapirusDB instance
	client := tapirus.NewClient(tapirus.Config{
		Endpoint: "http://127.0.0.1:8080",
	})

	// 2. Health check
	health, err := client.Health(ctx)
	if err != nil {
		log.Fatalf("Engine offline: %v", err)
	}
	fmt.Printf("Connected to TapirusDB %s (%s)\n", health.Version, health.Engine)

	// 3. Execute SQL DDL
	_, err = client.Execute(ctx, "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT);")
	if err != nil {
		log.Fatal(err)
	}

	// 4. Query
	rows, err := client.Query(ctx, "SELECT * FROM users;")
	if err != nil {
		log.Fatal(err)
	}
	fmt.Println("Users:", rows)

	// 5. Perform Vector Similarity Search
	matches, err := client.VectorSearch(ctx, "agent_embeddings", []float32{0.12, 0.45, -0.67}, 5)
	if err != nil {
		log.Fatal(err)
	}
	for _, m := range matches {
		fmt.Printf("Match ID %d - Score: %f\n", m.ID, m.Score)
	}

	// 6. GraphRAG Traversal
	rag, err := client.GraphRAG(ctx, tapirus.GraphRAGRequest{
		Query: "Explain autonomous consensus algorithm",
		Seeds: 3,
		Hops:  2,
	})
	if err != nil {
		log.Fatal(err)
	}
	fmt.Printf("Extracted %d knowledge nodes and %d relations\n", len(rag.Nodes), len(rag.Edges))
}
```

---

## 📜 License

Licensed under the [Business Source License 1.1 (BUSL-1.1)](https://github.com/tapiruslab/TapirusDB/blob/main/LICENSE).
