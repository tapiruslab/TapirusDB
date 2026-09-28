//! CLI Authentication and Data Importer Integration Tests for TapirusDB
//!
//! Validates:
//! - Langkah A: `tapirus serve --api-key <KEY>` token authentication (Bearer / X-API-Key / query param)
//! - Langkah B: `tapirus import` for CSV (relational SQL tables & type inference),
//!   JSONL (Document collections), and Markdown (AI Agent Memory & Knowledge Graph)

use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::Command;
use std::thread;
use std::time::Duration;
use tapirus::Connection;
use tempfile::tempdir;

const TAPIRUS_BIN: &str = env!("CARGO_BIN_EXE_tapirus");

#[test]
fn test_cli_import_csv() {
    let dir = tempdir().expect("Create temp dir");
    let csv_file = dir.path().join("customers.csv");
    let db_file = dir.path().join("crm.tapir");

    let csv_data = "id,name,credit_score,embedding\n\
                    1,Alice Johnson,750.5,[0.1, 0.2, 0.3]\n\
                    2,Bob Smith,680.0,[0.4, 0.5, 0.6]\n\
                    3,Charlie Davis,810.2,[0.7, 0.8, 0.9]\n";
    std::fs::write(&csv_file, csv_data).expect("Write CSV");

    // Execute tapirus import csv
    let status = Command::new(TAPIRUS_BIN)
        .arg("import")
        .arg("csv")
        .arg(&csv_file)
        .arg("--db")
        .arg(&db_file)
        .arg("--table")
        .arg("customers")
        .status()
        .expect("Execute tapirus import csv");

    assert!(status.success(), "tapirus import csv must succeed");

    // Verify database contents
    let conn = Connection::open(&db_file).expect("Open imported database");
    let rows = conn
        .query("SELECT id, name, credit_score FROM customers ORDER BY id ASC;")
        .expect("Query customers");

    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].get::<i64>("id").unwrap(), 1);
    assert_eq!(rows[0].get::<String>("name").unwrap(), "Alice Johnson");
    assert_eq!(rows[1].get::<f64>("credit_score").unwrap(), 680.0);

    // Verify vector search works on imported vector column
    let vec_rows = conn
        .query("SELECT id, name FROM customers VECTOR NEAR embedding = [0.1, 0.2, 0.3] TOP 1;")
        .expect("Vector search on imported table");
    assert_eq!(vec_rows.len(), 1);
    assert_eq!(vec_rows[0].get::<String>("name").unwrap(), "Alice Johnson");
}

#[test]
fn test_cli_import_jsonl() {
    let dir = tempdir().expect("Create temp dir");
    let jsonl_file = dir.path().join("telemetry.jsonl");
    let db_file = dir.path().join("metrics.tapir");

    let jsonl_data = "{\"node\": \"worker-1\", \"cpu\": 22.4, \"status\": \"online\"}\n\
                      {\"node\": \"worker-2\", \"cpu\": 65.1, \"status\": \"busy\"}\n\
                      {\"node\": \"worker-3\", \"cpu\": 12.0, \"status\": \"idle\"}\n";
    std::fs::write(&jsonl_file, jsonl_data).expect("Write JSONL");

    // Execute tapirus import jsonl
    let status = Command::new(TAPIRUS_BIN)
        .arg("import")
        .arg("jsonl")
        .arg(&jsonl_file)
        .arg("--db")
        .arg(&db_file)
        .arg("--collection")
        .arg("telemetry")
        .status()
        .expect("Execute tapirus import jsonl");

    assert!(status.success(), "tapirus import jsonl must succeed");

    // Verify document collection
    let conn = Connection::open(&db_file).expect("Open imported database");
    let col = conn.collection("telemetry").expect("Open telemetry collection");
    let all_docs = col.find_all().expect("Find all documents");

    assert_eq!(all_docs.len(), 3);
    assert_eq!(
        all_docs[0].1.get("node").and_then(|v| v.as_str()),
        Some("worker-1")
    );
    assert_eq!(
        all_docs[1].1.get("status").and_then(|v| v.as_str()),
        Some("busy")
    );
}

#[test]
fn test_cli_import_markdown_knowledge_graph_and_memory() {
    let dir = tempdir().expect("Create temp dir");
    let md_file = dir.path().join("runbook.md");
    let db_file = dir.path().join("knowledge.tapir");

    let md_data = "# Autonomous Incident Response\n\
                   This runbook defines emergency procedures for sovereign AI clusters.\n\n\
                   ## Failover Protocol\n\
                   When a primary node loses heartbeat for more than 500ms, initiate standby election.\n\n\
                   ## Quorum Rebalance\n\
                   After failover completes, WAL replication pointers are updated across all replicas.\n";
    std::fs::write(&md_file, md_data).expect("Write Markdown");

    // Execute tapirus import markdown
    let status = Command::new(TAPIRUS_BIN)
        .arg("import")
        .arg("md")
        .arg(&md_file)
        .arg("--db")
        .arg(&db_file)
        .arg("--namespace")
        .arg("sre_docs")
        .arg("--tags")
        .arg("incident,failover,quorum")
        .status()
        .expect("Execute tapirus import markdown");

    assert!(status.success(), "tapirus import md must succeed");

    // Verify AI Agent Memory and Knowledge Graph
    let conn = Connection::open(&db_file).expect("Open imported database");
    assert!(conn.memory_count() >= 3, "Expected at least 3 remembered sections");

    // Test hybrid memory recall
    let recalled = conn.memory_recall_text("failover heartbeat election", 3);
    assert!(!recalled.is_empty(), "Memory recall should find matching section");
    assert!(
        recalled[0].entry.content.contains("Failover Protocol")
            || recalled[0].entry.content.contains("heartbeat"),
        "Top recalled memory should match query semantics: {}",
        recalled[0].entry.content
    );

    // Test knowledge graph topology
    let (node_count, edge_count) = conn.graph_stats();
    assert_eq!(node_count, 4, "1 Document node + 3 Section nodes = 4 nodes");
    assert_eq!(edge_count, 5, "3 CONTAINS edges + 2 PRECEDES edges = 5 edges");

    // Verify graph node labels
    let nodes = conn.graph_nodes();
    assert!(nodes.iter().any(|n| n.label == "Document"));
    assert_eq!(nodes.iter().filter(|n| n.label == "Section").count(), 3);
}

fn http_exchange(host: &str, port: u16, request: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(format!("{host}:{port}")).expect("Connect to HTTP server");
    stream.write_all(request.as_bytes()).expect("Write HTTP request");

    let mut response_bytes = Vec::new();
    let mut buf = [0u8; 1024];
    loop {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => response_bytes.extend_from_slice(&buf[..n]),
            Err(_) => break,
        }
    }

    let response_str = String::from_utf8_lossy(&response_bytes).to_string();
    let status_code = response_str
        .lines()
        .next()
        .and_then(|line| {
            let mut parts = line.split_whitespace();
            parts.next(); // HTTP/1.1
            parts.next()?.parse::<u16>().ok()
        })
        .unwrap_or(0);

    (status_code, response_str)
}

#[test]
fn test_serve_api_key_authentication() {
    let dir = tempdir().expect("Create temp dir");
    let db_file = dir.path().join("server_auth.tapir");
    let conn = Connection::open(&db_file).expect("Create db");
    conn.execute("CREATE TABLE status (id INTEGER PRIMARY KEY, msg TEXT);").unwrap();
    conn.execute("INSERT INTO status (id, msg) VALUES (1, 'Server Online');").unwrap();
    drop(conn);

    let port: u16 = 18492;
    let api_key = "tapirus_live_secret_key_99";

    // Spawn server process with --api-key
    let mut child = Command::new(TAPIRUS_BIN)
        .arg("serve")
        .arg("--port")
        .arg(port.to_string())
        .arg("--host")
        .arg("127.0.0.1")
        .arg("--api-key")
        .arg(api_key)
        .arg(&db_file)
        .spawn()
        .expect("Spawn tapirus serve");

    // Wait briefly for server to bind
    thread::sleep(Duration::from_millis(600));

    // 1. Public endpoint /health should return 200 without any API key
    let health_req = "GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n";
    let (code, resp) = http_exchange("127.0.0.1", port, health_req);
    assert_eq!(code, 200, "GET /health must be public (200 OK): {resp}");
    assert!(resp.contains("\"status\":\"ok\""));

    // 2. Protected endpoint /api/sql without credentials must return 401 Unauthorized
    let payload = "{\"sql\":\"SELECT * FROM status;\"}";
    let plen = payload.len();

    let unauth_sql = format!(
        "POST /api/sql HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {plen}\r\nConnection: close\r\n\r\n{payload}"
    );
    let (code, resp) = http_exchange("127.0.0.1", port, &unauth_sql);
    assert_eq!(code, 401, "Protected route without auth must return 401: {resp}");
    assert!(resp.contains("WWW-Authenticate: Bearer"));
    assert!(resp.contains("Unauthorized"));

    // 3. Protected endpoint with invalid credentials must return 401 Unauthorized
    let wrong_sql = format!(
        "POST /api/sql HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer wrong_token\r\nContent-Type: application/json\r\nContent-Length: {plen}\r\nConnection: close\r\n\r\n{payload}"
    );
    let (code, resp) = http_exchange("127.0.0.1", port, &wrong_sql);
    assert_eq!(code, 401, "Protected route with wrong token must return 401: {resp}");

    // 4. Protected endpoint with valid Authorization: Bearer <KEY> must succeed (200 OK)
    let bearer_sql = format!(
        "POST /api/sql HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {api_key}\r\nContent-Type: application/json\r\nContent-Length: {plen}\r\nConnection: close\r\n\r\n{payload}"
    );
    let (code, resp) = http_exchange("127.0.0.1", port, &bearer_sql);
    assert_eq!(code, 200, "Protected route with Bearer key must succeed: {resp}");
    assert!(resp.contains("Server Online"));

    // 5. Protected endpoint with valid X-API-Key: <KEY> must succeed (200 OK)
    let x_api_sql = format!(
        "POST /api/sql HTTP/1.1\r\nHost: 127.0.0.1\r\nX-API-Key: {api_key}\r\nContent-Type: application/json\r\nContent-Length: {plen}\r\nConnection: close\r\n\r\n{payload}"
    );
    let (code, resp) = http_exchange("127.0.0.1", port, &x_api_sql);
    assert_eq!(code, 200, "Protected route with X-API-Key must succeed: {resp}");
    assert!(resp.contains("Server Online"));

    // 6. Protected endpoint with valid ?api_key=<KEY> parameter must succeed (200 OK)
    let query_sql = format!(
        "POST /api/sql?api_key={api_key} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {plen}\r\nConnection: close\r\n\r\n{payload}"
    );
    let (code, resp) = http_exchange("127.0.0.1", port, &query_sql);
    assert_eq!(code, 200, "Protected route with ?api_key= query parameter must succeed: {resp}");
    assert!(resp.contains("Server Online"));

    // Clean up server process
    let _ = child.kill();
}
