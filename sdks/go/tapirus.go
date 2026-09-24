// Package tapirus provides the official Go client for TapirusDB —
// the 100% Safe-Rust Embedded Quad-Model AI Database & Agent Memory Engine.
package tapirus

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"strings"
	"time"
)

// Config configures the TapirusDB client.
type Config struct {
	Endpoint string
	Token    string
	Timeout  time.Duration
}

// Client is the official TapirusDB Go client.
type Client struct {
	endpoint   string
	token      string
	httpClient *http.Client
}

// VectorMatch represents a single vector similarity search result.
type VectorMatch struct {
	ID         int64                  `json:"id"`
	Score      float32                `json:"score"`
	Collection string                 `json:"collection,omitempty"`
	Vector     []float32              `json:"vector,omitempty"`
	Metadata   map[string]interface{} `json:"metadata,omitempty"`
}

// GraphRAGRequest defines the parameters for a GraphRAG query.
type GraphRAGRequest struct {
	Query       string    `json:"query"`
	QueryVector []float32 `json:"query_vector,omitempty"`
	Seeds       int       `json:"seeds,omitempty"`
	Hops        int       `json:"hops,omitempty"`
}

// GraphRAGResponse represents the result of a GraphRAG traversal.
type GraphRAGResponse struct {
	Query   string                   `json:"query"`
	Nodes   []map[string]interface{} `json:"nodes"`
	Edges   []map[string]interface{} `json:"edges"`
	Context string                   `json:"context,omitempty"`
}

// HealthStatus reports daemon operational health.
type HealthStatus struct {
	Status  string `json:"status"`
	Version string `json:"version"`
	Engine  string `json:"engine"`
	Uptime  int64  `json:"uptime,omitempty"`
}

// NewClient initializes a new TapirusDB client.
func NewClient(cfg ...Config) *Client {
	endpoint := "http://127.0.0.1:8080"
	token := ""
	timeout := 30 * time.Second

	if len(cfg) > 0 {
		if cfg[0].Endpoint != "" {
			endpoint = strings.TrimRight(cfg[0].Endpoint, "/")
		}
		if cfg[0].Token != "" {
			token = cfg[0].Token
		}
		if cfg[0].Timeout > 0 {
			timeout = cfg[0].Timeout
		}
	}

	return &Client{
		endpoint: endpoint,
		token:    token,
		httpClient: &http.Client{
			Timeout: timeout,
		},
	}
}

// Query executes a SQL query returning rows as a slice of maps.
func (c *Client) Query(ctx context.Context, sql string, params ...interface{}) ([]map[string]interface{}, error) {
	payload := map[string]interface{}{
		"sql":    sql,
		"params": params,
	}

	var result []map[string]interface{}
	err := c.request(ctx, http.MethodPost, "/api/sql", payload, &result)
	if err != nil {
		return nil, err
	}
	return result, nil
}

// Execute executes a non-query SQL command and returns the affected rows count.
func (c *Client) Execute(ctx context.Context, sql string, params ...interface{}) (int64, error) {
	payload := map[string]interface{}{
		"sql":    sql,
		"params": params,
	}

	var resp struct {
		Affected int64 `json:"affected"`
	}
	err := c.request(ctx, http.MethodPost, "/api/sql", payload, &resp)
	if err != nil {
		return 0, err
	}
	return resp.Affected, nil
}

// VectorSearch performs high-performance similarity search on vector embeddings.
func (c *Client) VectorSearch(ctx context.Context, collection string, vector []float32, k int) ([]VectorMatch, error) {
	payload := map[string]interface{}{
		"collection": collection,
		"vector":     vector,
		"k":          k,
	}

	var results []VectorMatch
	err := c.request(ctx, http.MethodPost, "/api/vector/search", payload, &results)
	if err != nil {
		return nil, err
	}
	return results, nil
}

// GraphRAG executes a seeded GraphRAG knowledge traversal.
func (c *Client) GraphRAG(ctx context.Context, req GraphRAGRequest) (*GraphRAGResponse, error) {
	if req.Seeds <= 0 {
		req.Seeds = 3
	}
	if req.Hops <= 0 {
		req.Hops = 2
	}

	var resp GraphRAGResponse
	err := c.request(ctx, http.MethodPost, "/api/graph/rag", req, &resp)
	if err != nil {
		return nil, err
	}
	return &resp, nil
}

// Health checks the status of the TapirusDB engine instance.
func (c *Client) Health(ctx context.Context) (*HealthStatus, error) {
	var status HealthStatus
	err := c.request(ctx, http.MethodGet, "/api/health", nil, &status)
	if err != nil {
		return nil, err
	}
	return &status, nil
}

func (c *Client) request(ctx context.Context, method, path string, body interface{}, dest interface{}) error {
	var bodyReader io.Reader
	if body != nil {
		buf, err := json.Marshal(body)
		if err != nil {
			return fmt.Errorf("failed to marshal request: %w", err)
		}
		bodyReader = bytes.NewReader(buf)
	}

	url := c.endpoint + path
	req, err := http.NewRequestWithContext(ctx, method, url, bodyReader)
	if err != nil {
		return fmt.Errorf("failed to create request: %w", err)
	}

	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("Accept", "application/json")
	req.Header.Set("User-Agent", "TapirusDB-Go/1.0.0")

	if c.token != "" {
		req.Header.Set("Authorization", "Bearer "+c.token)
	}

	res, err := c.httpClient.Do(req)
	if err != nil {
		return fmt.Errorf("connection to TapirusDB failed: %w", err)
	}
	defer res.Body.Close()

	if res.StatusCode >= 400 {
		errBytes, _ := io.ReadAll(res.Body)
		return fmt.Errorf("tapirus error HTTP %d: %s", res.StatusCode, string(errBytes))
	}

	if dest != nil {
		if err := json.NewDecoder(res.Body).Decode(dest); err != nil {
			if !errors.Is(err, io.EOF) {
				return fmt.Errorf("failed to decode response: %w", err)
			}
		}
	}

	return nil
}
