package tapirus_test

import (
	"context"
	"fmt"
	"log"
	"net/http"
	"net/http/httptest"
	"time"

	tapirus "github.com/tapiruslab/TapirusDB/sdks/go"
)

func ExampleNewClient() {
	// Initialize client pointing to local or remote TapirusDB instance
	client := tapirus.NewClient(tapirus.Config{
		Endpoint: "http://127.0.0.1:8080",
		Timeout:  10 * time.Second,
	})

	_ = client
	fmt.Println("Client initialized")
	// Output:
	// Client initialized
}

func ExampleClient_Query() {
	// Mock server for standalone godoc example
	ts := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		fmt.Fprintln(w, `[{"id": 1, "name": "Agent-01", "role": "Orchestrator"}]`)
	}))
	defer ts.Close()

	ctx := context.Background()
	client := tapirus.NewClient(tapirus.Config{Endpoint: ts.URL})

	rows, err := client.Query(ctx, "SELECT id, name, role FROM agents WHERE id = ?", 1)
	if err != nil {
		log.Fatal(err)
	}

	for _, row := range rows {
		fmt.Printf("Agent #%v: %v (%v)\n", row["id"], row["name"], row["role"])
	}
	// Output:
	// Agent #1: Agent-01 (Orchestrator)
}

func ExampleClient_VectorSearch() {
	ts := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		fmt.Fprintln(w, `[{"id": 101, "score": 0.985, "collection": "docs"}]`)
	}))
	defer ts.Close()

	ctx := context.Background()
	client := tapirus.NewClient(tapirus.Config{Endpoint: ts.URL})

	queryVector := []float32{0.024, 0.451, -0.892, 0.114}
	matches, err := client.VectorSearch(ctx, "docs", queryVector, 1)
	if err != nil {
		log.Fatal(err)
	}

	for _, m := range matches {
		fmt.Printf("Nearest Vector ID: %d (Score: %.3f)\n", m.ID, m.Score)
	}
	// Output:
	// Nearest Vector ID: 101 (Score: 0.985)
}

func ExampleClient_GraphRAG() {
	ts := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		fmt.Fprintln(w, `{"query": "consensus", "nodes": [{"id": "n1"}], "edges": [{"from": "n1", "to": "n2"}]}`)
	}))
	defer ts.Close()

	ctx := context.Background()
	client := tapirus.NewClient(tapirus.Config{Endpoint: ts.URL})

	res, err := client.GraphRAG(ctx, tapirus.GraphRAGRequest{
		Query: "consensus",
		Seeds: 2,
		Hops:  1,
	})
	if err != nil {
		log.Fatal(err)
	}

	fmt.Printf("GraphRAG returned %d nodes and %d edges\n", len(res.Nodes), len(res.Edges))
	// Output:
	// GraphRAG returned 1 nodes and 1 edges
}
