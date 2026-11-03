package tapirus_test

import (
	"context"
	"encoding/json"
	"os"
	"testing"

	tapirus "github.com/tapiruslab/TapirusDB/sdks/go"
)

// scripts/test-http-clients.py owns the real daemon and verifies process reopen.
func TestLiveDaemon(t *testing.T) {
	endpoint := os.Getenv("TAPIRUS_TEST_ENDPOINT")
	if endpoint == "" {
		t.Skip("Run scripts/test-http-clients.py for real daemon integration")
	}
	ctx := context.Background()
	client := tapirus.NewClient(tapirus.Config{Endpoint: endpoint, Token: os.Getenv("TAPIRUS_TEST_TOKEN")})
	health, err := client.Health(ctx)
	if err != nil || health.Version != os.Getenv("TAPIRUS_TEST_VERSION") {
		t.Fatalf("health: %#v, %v", health, err)
	}
	if _, err := client.Execute(ctx, "CREATE TABLE go_consumer (id INTEGER PRIMARY KEY, note TEXT, big INTEGER)"); err != nil {
		t.Fatal(err)
	}
	const note = "quote ' ? ; -- source"
	if affected, err := client.Execute(ctx, "INSERT INTO go_consumer VALUES (?, ?, ?)", 1, note, int64(9007199254740993)); err != nil || affected != 1 {
		t.Fatalf("insert: %d, %v", affected, err)
	}
	rows, err := client.Query(ctx, "SELECT * FROM go_consumer WHERE id = ?", 1)
	if err != nil || len(rows) != 1 || rows[0]["note"] != note || rows[0]["big"] != json.Number("9007199254740993") {
		t.Fatalf("query lost bound values or integer precision: %#v, %v", rows, err)
	}
	for _, sql := range []string{"BEGIN", "INSERT INTO go_consumer VALUES (2, 'rolled back', 0)", "ROLLBACK"} {
		if _, err := client.Execute(ctx, sql); err != nil {
			t.Fatal(err)
		}
	}
	rows, err = client.Query(ctx, "SELECT * FROM go_consumer")
	if err != nil || len(rows) != 1 {
		t.Fatalf("rollback: %#v, %v", rows, err)
	}
	if _, err := client.Query(ctx, "INVALID SQL"); err == nil {
		t.Fatal("invalid SQL returned success")
	}
	unauthorized := tapirus.NewClient(tapirus.Config{Endpoint: endpoint})
	if _, err := unauthorized.Query(ctx, "SELECT 1"); err == nil {
		t.Fatal("unauthorized SQL returned success")
	}
	for _, sql := range []string{"CREATE TABLE go_vectors (id INTEGER PRIMARY KEY, embedding VECTOR(2))", "INSERT INTO go_vectors VALUES (1, [1, 0])"} {
		if _, err := client.Execute(ctx, sql); err != nil {
			t.Fatal(err)
		}
	}
	matches, err := client.VectorSearch(ctx, "go_vectors", []float32{1, 0}, 1)
	if err != nil || len(matches) != 1 || matches[0].ID != 1 {
		t.Fatalf("vector response: %#v, %v", matches, err)
	}
	if response, err := client.GraphRAG(ctx, tapirus.GraphRAGRequest{Query: "maintenance"}); err != nil || response.Query != "maintenance" {
		t.Fatalf("graph response: %#v, %v", response, err)
	}
}
