package tapirus_test

import (
	"context"
	"encoding/json"
	"fmt"
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
				"engine":  "TapirusDB HTTP test daemon",
			})
		case "/api/sql":
			var request struct {
				SQL    string        `json:"sql"`
				Params []interface{} `json:"params"`
			}
			if err := json.NewDecoder(r.Body).Decode(&request); err != nil {
				t.Errorf("failed to decode SQL request: %v", err)
			}
			if request.SQL != "SELECT * FROM agents WHERE active = ?" || len(request.Params) != 1 || request.Params[0] != true {
				t.Errorf("unexpected parameterized SQL request: %#v", request)
			}
			_ = json.NewEncoder(w).Encode(map[string]interface{}{"rows": []map[string]interface{}{
				{"id": 1, "name": "Agent Alpha", "score": 0.98},
			}})
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

func TestQueryExactIntegersAndInvalidResponses(t *testing.T) {
	for _, test := range []struct {
		body  string
		fails bool
	}{
		{`{"rows":[{"big":9007199254740993}]}`, false},
		{"", true},
		{"invalid JSON", true},
	} {
		t.Run(test.body, func(t *testing.T) {
			server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				var request struct {
					Params []interface{} `json:"params"`
				}
				if err := json.NewDecoder(r.Body).Decode(&request); err != nil || request.Params == nil {
					t.Error("Unbound SQL must send an empty parameter array")
				}
				fmt.Fprint(w, test.body)
			}))
			defer server.Close()
			rows, err := tapirus.NewClient(tapirus.Config{Endpoint: server.URL}).Query(context.Background(), "SELECT 1")
			if (err != nil) != test.fails {
				t.Fatalf("unexpected error: %v", err)
			}
			if !test.fails && rows[0]["big"] != json.Number("9007199254740993") {
				t.Fatalf("Lost integer precision: %#v", rows)
			}
		})
	}
}
