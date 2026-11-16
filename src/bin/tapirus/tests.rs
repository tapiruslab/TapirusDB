use super::*;

#[test]
fn http_client_limit_releases_slots_when_guards_drop() {
    let active = Arc::new(AtomicUsize::new(0));
    let guards: Vec<_> = (0..HTTP_MAX_CLIENTS)
        .map(|_| try_acquire_http_client(&active).expect("available HTTP client slot"))
        .collect();
    assert_eq!(active.load(Ordering::Acquire), HTTP_MAX_CLIENTS);
    assert!(try_acquire_http_client(&active).is_none());

    drop(guards);
    assert_eq!(active.load(Ordering::Acquire), 0);
    assert!(try_acquire_http_client(&active).is_some());
    assert_eq!(active.load(Ordering::Acquire), 0);
}

#[test]
fn http_sql_payload_preserves_bound_parameters() {
    let (sql, params) =
        parse_http_sql_payload(r#"{"sql":"SELECT ? AS value","params":["?"]}"#).unwrap();
    assert_eq!(sql, "SELECT ? AS value");
    assert_eq!(params, vec![Value::Text("?".into())]);
    assert!(parse_http_sql_payload(r#"{"sql":"SELECT ?","params":{}}"#).is_err());
}

#[test]
fn ambiguous_http_body_framing_is_rejected() {
    assert_eq!(
        request_content_length(b"POST /sql HTTP/1.1\r\nContent-Length: 3\r\n\r\n"),
        Ok(3)
    );
    assert!(
        request_content_length(
            b"POST /sql HTTP/1.1\r\nContent-Length: 3\r\nContent-Length: 4\r\n\r\n"
        )
        .is_err()
    );
    assert!(
        request_content_length(b"POST /sql HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n")
            .is_err()
    );
}

#[test]
fn chat_endpoints_require_the_configured_key() {
    assert!(!request_has_valid_api_key(
        "secret",
        "GET /api/chat/logs HTTP/1.1\r\n"
    ));
    assert!(request_has_valid_api_key(
        "secret",
        "GET /api/chat/logs HTTP/1.1\r\nAuthorization: Bearer secret\r\n"
    ));
    assert!(!request_has_valid_api_key(
        "secret",
        "GET /api/chat/logs HTTP/1.1\r\nX-API-Key: wrong\r\n"
    ));
}

#[test]
fn vector_endpoint_reports_record_vector_and_cosine_similarity()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    let db = Connection::open_in_memory()?;
    db.execute("CREATE TABLE vectors (id INTEGER PRIMARY KEY, body TEXT, embedding VECTOR(2));")?;
    db.execute("INSERT INTO vectors VALUES (1, 'orthogonal', [1.0, 0.0]);")?;
    db.execute("INSERT INTO vectors VALUES (2, 'same', [0.0, 1.0]);")?;
    let results = vector_search_results(&db, "vectors", &[0.0, 1.0], 2).unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0]["id"], 2);
    assert_eq!(results[0]["score"].as_f64().unwrap(), 1.0);
    assert_eq!(results[0]["vector"], serde_json::json!([0.0, 1.0]));
    assert_eq!(results[1]["score"].as_f64().unwrap().abs(), 0.0);
    assert_eq!(results[1]["vector"], serde_json::json!([1.0, 0.0]));
    Ok(())
}

#[test]
fn cors_policy_defaults_to_restricted_and_validates_origins() {
    let default_origins = parse_cors_origins("").unwrap();
    assert!(default_origins.is_empty());
    assert_eq!(
        resolve_cors_origin(
            "POST /api/sql HTTP/1.1\r\nOrigin: https://evil.example\r\n",
            &default_origins
        ),
        None
    );

    let restrictive_resp = format_http_response("200 OK", "application/json", "{}", None, None);
    assert!(!restrictive_resp.contains("Access-Control-Allow-Origin"));

    let allowlist = parse_cors_origins("https://app.example.com/, http://localhost:5173").unwrap();
    assert_eq!(
        resolve_cors_origin(
            "POST /api/sql HTTP/1.1\r\nOrigin: https://app.example.com\r\n",
            &allowlist
        ),
        Some("https://app.example.com")
    );
    assert_eq!(
        resolve_cors_origin(
            "POST /api/sql HTTP/1.1\r\nOrigin: https://evil.example\r\n",
            &allowlist
        ),
        None
    );

    let allowed_resp = format_http_response(
        "200 OK",
        "application/json",
        "{}",
        None,
        Some("https://app.example.com"),
    );
    assert!(allowed_resp.contains("Access-Control-Allow-Origin: https://app.example.com\r\n"));
    assert!(allowed_resp.contains("Vary: Origin\r\n"));

    assert!(parse_cors_origins("https://good.example\r\nEvil: 1").is_err());
    assert!(parse_cors_origins("file:///etc/passwd").is_err());
}

#[test]
fn mcp_supports_caller_and_external_http_neural_embeddings()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    let db = Connection::open_in_memory()?;

    // 1. Caller-supplied explicit embedding works without external server or keyword overlap
    let store_resp: serde_json::Value = serde_json::from_str(&handle_mcp_tool_call(
        &db,
        "tapirus_remember",
        &serde_json::json!({
            "content": "Kubernetes pod evicted due to OOMKilled limit",
            "namespace": "infra",
            "embedding": [1.0, 0.0, 0.0, 0.0],
            "embedding_model": "bge-small"
        }),
    ))?;
    assert_eq!(store_resp["status"], "success");
    assert_eq!(store_resp["embedding_mode"], "caller-vector");
    assert_eq!(store_resp["embedding_dims"], 4);

    let recall_resp: serde_json::Value = serde_json::from_str(&handle_mcp_tool_call(
        &db,
        "tapirus_recall",
        &serde_json::json!({
            "query": "container out of memory crash",
            "namespace": "infra",
            "embedding": [0.99, 0.1, 0.0, 0.0],
            "embedding_model": "bge-small"
        }),
    ))?;
    assert_eq!(recall_resp["status"], "success");
    assert_eq!(recall_resp["embedding_mode"], "caller-vector");
    assert_eq!(recall_resp["count"], 1);

    // 2. Local HTTP Ollama-compatible /api/embed server provides neural embeddings
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let addr = listener.local_addr()?;
    let server_thread = thread::spawn(move || {
        for stream in listener.incoming().take(3) {
            let Ok(mut stream) = stream else { break };
            let mut req_buf = Vec::new();
            let mut chunk = [0u8; 4096];
            while let Ok(n) = stream.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                req_buf.extend_from_slice(&chunk[..n]);
                if let Some(end) = find_subsequence(&req_buf, b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&req_buf[..end]);
                    let content_len = headers
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .and_then(|v| v.trim().parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    if req_buf.len() >= end + 4 + content_len {
                        break;
                    }
                }
            }
            let req_text = String::from_utf8_lossy(&req_buf);
            let body = req_text.split("\r\n\r\n").nth(1).unwrap_or("{}");
            let vec = if body.contains("laptop battery") || body.contains("notebook power") {
                serde_json::json!([0.0, 1.0, 0.0, 0.0])
            } else {
                serde_json::json!([0.0, 0.0, 1.0, 0.0])
            };
            let payload = serde_json::json!({
                "model": "nomic-embed-text",
                "embeddings": [vec]
            })
            .to_string();
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                payload.len(),
                payload
            );
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.flush();
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
    });

    let cfg = McpServerConfig {
        embed_endpoint: Some(format!("http://{addr}/api/embed")),
        embed_model: Some("nomic-embed-text".to_string()),
        embed_dims: Some(4),
        embed_api_key: Some("embed-secret".to_string()),
    };

    // Semantic hit (no keyword overlap with query "notebook power drain")
    let s1: serde_json::Value = serde_json::from_str(&handle_mcp_tool_call_with_config(
        &db,
        "tapirus_remember",
        &serde_json::json!({
            "content": "Replace laptop battery cell when runtime drops below 20 minutes",
            "namespace": "support"
        }),
        &cfg,
    ))?;
    assert_eq!(s1["status"], "success");
    assert_eq!(s1["embedding_mode"], "neural-http");
    assert_eq!(s1["embedding_dims"], 4);

    // Unrelated lexical distractor
    let _s2 = handle_mcp_tool_call_with_config(
        &db,
        "tapirus_remember",
        &serde_json::json!({
            "content": "Firewall power adapter rack notebook inventory",
            "namespace": "support"
        }),
        &cfg,
    );

    let r_neural: serde_json::Value = serde_json::from_str(&handle_mcp_tool_call_with_config(
        &db,
        "tapirus_recall",
        &serde_json::json!({
            "query": "notebook power drain",
            "namespace": "support",
            "limit": 2
        }),
        &cfg,
    ))?;
    assert_eq!(r_neural["status"], "success");
    assert_eq!(r_neural["embedding_mode"], "neural-http");
    assert_eq!(
        r_neural["memories"][0]["content"],
        "Replace laptop battery cell when runtime drops below 20 minutes"
    );
    let _ = server_thread.join();

    // 3. Configured endpoint failures fail loudly instead of silently downgrading to hash
    let err_resp: serde_json::Value = serde_json::from_str(&handle_mcp_tool_call_with_config(
        &db,
        "tapirus_remember",
        &serde_json::json!({ "content": "should fail loudly" }),
        &cfg,
    ))?;
    assert_eq!(err_resp["status"], "error");
    Ok(())
}

#[test]
fn mcp_namespace_scope_isolates_tools_and_blocks_sys_memory_sql()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    let db = Connection::open_in_memory()?;
    db.store_memory(&tapirus::MemoryDocument {
        content: "Secret beta memory".into(),
        namespace: Some("tenant_beta".into()),
        ..Default::default()
    })?;

    let scoped_alpha = db.scoped_memory("tenant_alpha", Some("sess_1"))?;
    let ok_remember: serde_json::Value = serde_json::from_str(&handle_mcp_tool_call(
        &scoped_alpha,
        "tapirus_remember",
        &serde_json::json!({ "content": "Alpha note" }),
    ))?;
    assert_eq!(ok_remember["status"], "success");
    assert_eq!(ok_remember["namespace"], "tenant_alpha");
    assert_eq!(ok_remember["session_id"], "sess_1");

    let denied_remember: serde_json::Value = serde_json::from_str(&handle_mcp_tool_call(
        &scoped_alpha,
        "tapirus_remember",
        &serde_json::json!({ "content": "Escape attempt", "namespace": "tenant_beta" }),
    ))?;
    assert_eq!(denied_remember["status"], "error");

    let recall_alpha: serde_json::Value = serde_json::from_str(&handle_mcp_tool_call(
        &scoped_alpha,
        "tapirus_recall",
        &serde_json::json!({ "query": "Secret beta memory" }),
    ))?;
    let memories = recall_alpha["memories"].as_array().unwrap();
    assert!(
        memories
            .iter()
            .all(|m| { m["namespace"] == "tenant_alpha" && m["content"] != "Secret beta memory" }),
        "Scoped MCP recall leaked cross-tenant data: {recall_alpha}"
    );

    let denied_sql: serde_json::Value = serde_json::from_str(&handle_mcp_tool_call(
        &scoped_alpha,
        "tapirus_sql",
        &serde_json::json!({ "sql": "SELECT * FROM __sys_memory;" }),
    ))?;
    assert_eq!(denied_sql["status"], "error");
    assert!(
        denied_sql["message"]
            .as_str()
            .unwrap_or("")
            .contains("Permission denied")
    );
    Ok(())
}

#[test]
fn http_api_key_namespace_enforces_tenant_isolation()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    let bindings = parse_api_key_namespace_bindings(&[
        "key-alpha=tenant_alpha, key-beta=tenant_beta".to_string(),
    ])
    .unwrap();
    assert_eq!(
        authenticate_http_request(
            Some("master-key"),
            &bindings,
            &[],
            "POST /api/memory/store HTTP/1.1\r\nAuthorization: Bearer key-alpha\r\n"
        ),
        Some(HttpAuthScope::Namespace("tenant_alpha".to_string()))
    );
    assert_eq!(
        authenticate_http_request(
            Some("master-key"),
            &bindings,
            &[],
            "POST /api/sql HTTP/1.1\r\nX-API-Key: master-key\r\n"
        ),
        Some(HttpAuthScope::Unrestricted)
    );
    assert_eq!(
        authenticate_http_request(
            Some("master-key"),
            &bindings,
            &[],
            "POST /api/memory/store HTTP/1.1\r\nX-API-Key: invalid\r\n"
        ),
        None
    );

    let conn = Connection::open_in_memory()?;
    let store_b = handle_http_memory_route(
        &conn,
        "/api/memory/store",
        r#"{"content":"Beta confidential roadmap","embedding":[0.0,1.0]}"#,
        &HttpAuthScope::Namespace("tenant_beta".to_string()),
    )
    .unwrap();
    let store_b_json: serde_json::Value = serde_json::from_str(&store_b)?;
    let beta_id = store_b_json["memory_id"].as_u64().unwrap();

    let err_cross_ns = handle_http_memory_route(
        &conn,
        "/api/memory/store",
        r#"{"content":"Write into beta","namespace":"tenant_beta"}"#,
        &HttpAuthScope::Namespace("tenant_alpha".to_string()),
    )
    .unwrap_err();
    assert_eq!(err_cross_ns.0, "403 Forbidden");

    let search_alpha: serde_json::Value = serde_json::from_str(
        &handle_http_memory_route(
            &conn,
            "/api/memory/search",
            r#"{"query":"Beta confidential","embedding":[0.0,1.0]}"#,
            &HttpAuthScope::Namespace("tenant_alpha".to_string()),
        )
        .unwrap(),
    )?;
    assert_eq!(search_alpha["results"].as_array().unwrap().len(), 0);

    let forget_err = handle_http_memory_route(
        &conn,
        "/api/memory/forget",
        &serde_json::json!({ "id": beta_id }).to_string(),
        &HttpAuthScope::Namespace("tenant_alpha".to_string()),
    )
    .unwrap_err();
    assert_eq!(forget_err.0, "403 Forbidden");
    assert!(conn.memory_get(beta_id).is_some());

    // End-to-end HTTP check: tenant key is rejected on /api/sql (403 Forbidden)
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let addr = listener.local_addr()?;
    let db_arc = Arc::new(Mutex::new(conn));
    let db_srv = Arc::clone(&db_arc);
    let master = Arc::new(Some("master-key".to_string()));
    let scopes = Arc::new(bindings);
    let cors = Arc::new(Vec::new());
    let srv = thread::spawn(move || {
        if let Ok((stream, _)) = listener.accept() {
            handle_http_client(stream, db_srv, master, scopes, Arc::new(Vec::new()), cors);
        }
    });
    let mut client = TcpStream::connect(addr)?;
    let req_body = r#"{"sql":"SELECT * FROM __sys_memory;"}"#;
    let raw_req = format!(
        "POST /api/sql HTTP/1.1\r\nHost: localhost\r\nX-API-Key: key-alpha\r\nContent-Length: {}\r\n\r\n{}",
        req_body.len(),
        req_body
    );
    client.write_all(raw_req.as_bytes())?;
    let mut resp = String::new();
    client.read_to_string(&mut resp)?;
    assert!(resp.starts_with("HTTP/1.1 403 Forbidden"));
    assert!(resp.contains("X-Content-Type-Options: nosniff"));
    assert!(resp.contains("Cache-Control: no-store"));
    let _ = srv.join();
    Ok(())
}

#[test]
fn http_api_key_sql_rbac_enforces_roles_and_table_allowlists()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    let role_bindings = parse_api_key_role_bindings(&[
        "ro-public=readonly:public_items+orders, rw-orders=readwrite:orders".to_string(),
    ])
    .unwrap();

    let conn = Connection::open_in_memory()?;
    conn.execute("CREATE TABLE public_items (id INTEGER PRIMARY KEY, label TEXT);")?;
    conn.execute("CREATE TABLE orders (id INTEGER PRIMARY KEY, total INTEGER);")?;
    conn.execute("CREATE TABLE secrets (id INTEGER PRIMARY KEY, secret TEXT);")?;
    conn.execute("INSERT INTO public_items VALUES (1, 'tapir-book');")?;
    conn.execute("INSERT INTO secrets VALUES (1, 'master-secret');")?;

    let db_arc = Arc::new(Mutex::new(conn));
    let master = Arc::new(Some("master-key".to_string()));
    let scopes = Arc::new(Vec::new());
    let roles = Arc::new(role_bindings);
    let cors = Arc::new(Vec::new());

    let send_sql = |key: &str, sql: &str| -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let db_srv = Arc::clone(&db_arc);
        let master_srv = Arc::clone(&master);
        let scopes_srv = Arc::clone(&scopes);
        let roles_srv = Arc::clone(&roles);
        let cors_srv = Arc::clone(&cors);
        let srv = thread::spawn(move || {
            if let Ok((stream, _)) = listener.accept() {
                handle_http_client(stream, db_srv, master_srv, scopes_srv, roles_srv, cors_srv);
            }
        });
        let mut client = TcpStream::connect(addr).unwrap();
        let req_body = serde_json::json!({ "sql": sql }).to_string();
        let raw_req = format!(
            "POST /api/sql HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {key}\r\nContent-Length: {}\r\n\r\n{}",
            req_body.len(),
            req_body
        );
        client.write_all(raw_req.as_bytes()).unwrap();
        let mut resp = String::new();
        client.read_to_string(&mut resp).unwrap();
        let _ = srv.join();
        resp
    };

    // 1. Read-only key can SELECT from allowlisted table (including parameterized queries)
    let ok_select = send_sql("ro-public", "SELECT id, label FROM public_items;");
    assert!(ok_select.starts_with("HTTP/1.1 200 OK"), "{ok_select}");
    assert!(ok_select.contains("tapir-book"));
    assert!(ok_select.contains("X-Frame-Options: DENY"));

    assert!(
        authorize_sql_for_role(
            "SELECT id FROM public_items WHERE id = ?;",
            &[Value::Integer(1)],
            SqlRoleAccess::ReadOnly,
            Some(&["public_items".to_string()])
        )
        .is_ok()
    );

    // 2. Read-only key cannot access non-allowlisted table directly, via JOIN, or via subqueries (EXISTS / scalar)
    assert!(send_sql("ro-public", "SELECT * FROM secrets;").starts_with("HTTP/1.1 403 Forbidden"));
    assert!(
        send_sql(
            "ro-public",
            "SELECT p.id FROM public_items p WHERE EXISTS (SELECT id FROM secrets);"
        )
        .starts_with("HTTP/1.1 403 Forbidden")
    );
    assert!(
        send_sql(
            "ro-public",
            "SELECT p.id, (SELECT COUNT(*) FROM secrets) AS leak FROM public_items p;"
        )
        .starts_with("HTTP/1.1 403 Forbidden")
    );
    assert!(
        send_sql(
            "ro-public",
            "SELECT p.id FROM public_items p WHERE p.id = (SELECT id FROM secrets LIMIT 1);"
        )
        .starts_with("HTTP/1.1 403 Forbidden")
    );

    // 3. Read-only key cannot write or smuggle multiple statements
    assert!(
        send_sql("ro-public", "INSERT INTO public_items VALUES (2, 'bad');")
            .starts_with("HTTP/1.1 403 Forbidden")
    );
    assert!(
        send_sql(
            "ro-public",
            "SELECT id FROM public_items; DROP TABLE public_items;"
        )
        .starts_with("HTTP/1.1 403 Forbidden")
    );

    // 4. ReadWrite key can mutate allowlisted table `orders`, cannot touch `secrets`, cannot DROP
    assert!(
        send_sql("rw-orders", "INSERT INTO orders VALUES (10, 500);")
            .starts_with("HTTP/1.1 200 OK")
    );
    assert!(
        send_sql("rw-orders", "INSERT INTO public_items VALUES (2, 'nope');")
            .starts_with("HTTP/1.1 403 Forbidden")
    );
    assert!(
        send_sql(
            "rw-orders",
            "UPDATE orders SET total = (SELECT id FROM secrets LIMIT 1) WHERE id = 10;"
        )
        .starts_with("HTTP/1.1 403 Forbidden")
    );
    assert!(send_sql("rw-orders", "DROP TABLE orders;").starts_with("HTTP/1.1 403 Forbidden"));

    Ok(())
}

#[cfg(feature = "tls")]
#[test]
fn tls_config_loader_rejects_missing_files() {
    let missing = load_tls_server_config(
        Path::new("nonexistent-cert.pem"),
        Path::new("nonexistent-key.pem"),
    );
    assert!(missing.is_err());
}
