package tapirus_test

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"

	tapirus "github.com/tapiruslab/TapirusDB/sdks/go"
)

func TestTapirusClient(t *testing.T) {
	// Create mock TapirusDB daemon server
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/api/health":
			_ = json.NewEncoder(w).Encode(map[string]interface{}{
				"status":  "ok",
				"version": "1.0.0",
				"engine":  "Safe-Rust Quad-Model",
			})
		case "/api/sql":
			_ = json.NewEncoder(w).Encode([]map[string]interface{}{
				{"id": 1, "name": "Agent Alpha", "score": 0.98},
			})
		case "/api/vector/search":
			_ = json.NewEncoder(w).Encode([]tapirus.VectorMatch{
				{ID: 1, Score: 0.99, Collection: "memories"},
			})
		case "/api/graph/rag":
			_ = json.NewEncoder(w).Encode(tapirus.GraphRAGResponse{
				Query: "test query",
				Nodes: []map[string]interface{}{{"id": "n1"}},
				Edges: []map[string]interface{}{{"from": "n1", "to": "n2"}},
			})
		default:
			http.NotFound(w, r)
		}
	}))
	defer server.Close()

	client := tapirus.NewClient(tapirus.Config{
		Endpoint: server.URL,
		Timeout:  5 * time.Second,
	})

	ctx := context.Background()

	// 1. Health
	health, err := client.Health(ctx)
	if err != nil {
		t.Fatalf("Health check failed: %v", err)
	}
	if health.Status != "ok" {
		t.Errorf("Expected status 'ok', got %q", health.Status)
	}

	// 2. SQL Query
	rows, err := client.Query(ctx, "SELECT * FROM agents WHERE active = ?", true)
	if err != nil {
		t.Fatalf("Query failed: %v", err)
	}
	if len(rows) != 1 || rows[0]["name"] != "Agent Alpha" {
		t.Errorf("Unexpected rows: %v", rows)
	}

	// 3. Vector Search
	matches, err := client.VectorSearch(ctx, "memories", []float32{0.1, 0.2, 0.3}, 3)
	if err != nil {
		t.Fatalf("VectorSearch failed: %v", err)
	}
	if len(matches) != 1 || matches[0].Score < 0.9 {
		t.Errorf("Unexpected vector matches: %v", matches)
	}

	// 4. GraphRAG
	rag, err := client.GraphRAG(ctx, tapirus.GraphRAGRequest{
		Query: "Who is agent alpha?",
		Seeds: 2,
		Hops:  1,
	})
	if err != nil {
		t.Fatalf("GraphRAG failed: %v", err)
	}
	if len(rag.Nodes) != 1 {
		t.Errorf("Expected 1 node, got %d", len(rag.Nodes))
	}
}
