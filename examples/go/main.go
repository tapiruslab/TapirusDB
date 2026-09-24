package main

import (
	"fmt"
	"log"

	"./tapirus"
)

func main() {
	fmt.Printf("🚀 TapirusDB Go Client (Engine v%s)\n", tapirus.Version())

	// Open in-memory database
	db, err := tapirus.OpenInMemory()
	if err != nil {
		log.Fatalf("Failed to open database: %v", err)
	}
	defer db.Close()

	// 1. Create Table with Vector column
	_, err = db.Execute("CREATE TABLE documents (id INTEGER PRIMARY KEY, title TEXT, embedding VECTOR(3));")
	if err != nil {
		log.Fatalf("Create table failed: %v", err)
	}

	// 2. Insert Records
	_, err = db.Execute("INSERT INTO documents (id, title, embedding) VALUES (1, 'Safe Rust Concurrency', [0.1, 0.9, 0.2]);")
	if err != nil {
		log.Fatalf("Insert failed: %v", err)
	}

	// 3. Transactions
	_, _ = db.Execute("BEGIN;")
	_, _ = db.Execute("INSERT INTO documents (id, title, embedding) VALUES (2, 'AI Vector Indexing', [0.8, 0.1, 0.1]);")
	_, _ = db.Execute("COMMIT;")

	// 4. Query All Rows
	rows, err := db.Query("SELECT id, title FROM documents;")
	if err != nil {
		log.Fatalf("Query failed: %v", err)
	}

	fmt.Println("\n📦 Retrieved Documents:")
	for _, row := range rows {
		fmt.Printf(" - ID: %v | Title: %v\n", row["id"], row["title"])
	}

	// 5. Vector ANN Search
	results, err := db.Query("SELECT id, title FROM documents VECTOR NEAR embedding = [0.15, 0.85, 0.2] TOP 1;")
	if err != nil {
		log.Fatalf("Vector search failed: %v", err)
	}

	fmt.Println("\n⚡ Top Vector Match:")
	for _, match := range results {
		fmt.Printf(" - Nearest: %v (ID: %v)\n", match["title"], match["id"])
	}
}
