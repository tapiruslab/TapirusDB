<?php

declare(strict_types=1);

namespace Tapirus;

/**
 * TapirusDB Official PHP Client.
 *
 * Connects seamlessly to in-process or local `tapirus serve` instances
 * executing Relational SQL, Vector Search, and GraphRAG traversals.
 */
class Client
{
    private string $endpoint;
    private ?string $token;
    private int $timeoutSeconds;

    public function __construct(string $endpoint = 'http://127.0.0.1:8080', ?string $token = null, int $timeoutSeconds = 30)
    {
        $this->endpoint = rtrim($endpoint, '/');
        $this->token = $token;
        $this->timeoutSeconds = $timeoutSeconds;
    }

    /**
     * Execute a SQL query or command.
     *
     * @param string $sql SQL query string (e.g. "SELECT * FROM items WHERE price > ?;")
     * @param array $params Optional bind parameters
     * @return array Decoded response rows or metadata
     */
    public function query(string $sql, array $params = []): array
    {
        return $this->request('/api/sql', [
            'sql' => $sql,
            'params' => $params
        ]);
    }

    /**
     * Execute a SQL command (alias for query).
     */
    public function execute(string $sql): array
    {
        return $this->query($sql);
    }

    /**
     * Perform HNSW Vector Similarity search.
     *
     * @param string $collection Collection or index name
     * @param float[] $vector Float array query embedding
     * @param int $k Top-k results limit
     * @return array List of matched vectors with similarity scores
     */
    public function vectorSearch(string $collection, array $vector, int $k = 5): array
    {
        return $this->request('/api/vector/search', [
            'collection' => $collection,
            'vector' => $vector,
            'k' => $k
        ]);
    }

    /**
     * Execute a GraphRAG seed-and-traverse query.
     */
    public function graphRag(string $query, ?array $queryVector = null, int $seeds = 3, int $hops = 2): array
    {
        return $this->request('/api/graph/rag', [
            'query' => $query,
            'query_vector' => $queryVector,
            'seeds' => $seeds,
            'hops' => $hops
        ]);
    }

    /**
     * Health check endpoint.
     */
    public function health(): array
    {
        return $this->request('/api/health', null, 'GET');
    }

    /**
     * Send HTTP request to TapirusDB daemon.
     */
    private function request(string $path, ?array $body = null, string $method = 'POST'): array
    {
        $url = $this->endpoint . $path;
        $ch = curl_init($url);

        curl_setopt($ch, CURLOPT_RETURNTRANSFER, true);
        curl_setopt($ch, CURLOPT_CUSTOMREQUEST, $method);
        curl_setopt($ch, CURLOPT_TIMEOUT, $this->timeoutSeconds);

        $headers = [
            'Content-Type: application/json',
            'Accept: application/json',
            'User-Agent: TapirusDB-PHP/1.0.0'
        ];

        if ($this->token !== null && $this->token !== '') {
            $headers[] = 'Authorization: Bearer ' . $this->token;
        }

        curl_setopt($ch, CURLOPT_HTTPHEADER, $headers);

        if ($body !== null) {
            $encoded = json_encode($body, JSON_UNESCAPED_SLASHES);
            curl_setopt($ch, CURLOPT_POSTFIELDS, $encoded);
        }

        $response = curl_exec($ch);
        $httpCode = curl_getinfo($ch, CURLINFO_HTTP_CODE);
        $error = curl_error($ch);
        curl_close($ch);

        if ($response === false) {
            throw new \RuntimeException("TapirusDB connection failed: {$error}");
        }

        $decoded = json_decode($response, true);
        if ($httpCode >= 400) {
            $msg = $decoded['error'] ?? "TapirusDB returned HTTP {$httpCode}";
            throw new \RuntimeException($msg, $httpCode);
        }

        return is_array($decoded) ? $decoded : ['raw' => $response];
    }
}
