use super::*;

#[derive(Clone, Debug, Default)]
pub(super) struct McpServerConfig {
    pub(super) embed_endpoint: Option<String>,
    pub(super) embed_model: Option<String>,
    pub(super) embed_dims: Option<usize>,
    pub(super) embed_api_key: Option<String>,
}

pub(super) fn print_mcp_help() {
    println!("Usage: tapirus mcp [OPTIONS] [DATABASE_FILE]");
    println!();
    println!("Launch Model Context Protocol (MCP) JSON-RPC 2.0 stdio server for AI agents.");
    println!("Compatible with Claude Desktop, Cursor, Gemini, and any MCP client.");
    println!();
    println!("Options:");
    println!("  --config-claude            Print ready-to-paste Claude Desktop JSON configuration");
    println!("  --passphrase <KEY>         Passphrase for encrypted database (ChaCha20-Poly1305)");
    println!(
        "  --embed-endpoint <URL>     External neural embedding endpoint (Ollama /api/embed or OpenAI /v1/embeddings)"
    );
    println!(
        "  --embed-model <NAME>       Embedding model identifier (default: nomic-embed-text when endpoint set)"
    );
    println!("  --embed-dims <N>           Expected embedding dimension for validation");
    println!(
        "  --embed-api-key <KEY>      Optional Bearer API key for external embedding endpoint"
    );
    println!(
        "  --namespace <NS>           Enforce strict owner namespace isolation across MCP calls"
    );
    println!("  --session-id <ID>          Bind an optional owner session within --namespace");
    println!("  -h, --help                 Print this help message");
    println!();
    println!("Environment Variables:");
    println!("  TAPIRUS_EMBED_ENDPOINT     Default --embed-endpoint");
    println!("  TAPIRUS_EMBED_MODEL        Default --embed-model");
    println!("  TAPIRUS_EMBED_DIMS         Default --embed-dims");
    println!("  TAPIRUS_EMBED_API_KEY      Default --embed-api-key");
    println!("  TAPIRUS_MCP_NAMESPACE      Default --namespace");
    println!("  TAPIRUS_MCP_SESSION_ID     Default --session-id");
    println!();
    println!("Arguments:");
    println!(
        "  [DATABASE_FILE]            Path to database file (defaults to: agent_memory.tapir)"
    );
}

pub(super) fn print_claude_config(db_path: &str) {
    let current_exe = std::env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "tapirus".to_string());
    let abs_db = std::fs::canonicalize(db_path)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| db_path.to_string());

    let config = serde_json::json!({
        "mcpServers": {
            "tapirus": {
                "command": current_exe,
                "args": ["mcp", abs_db]
            }
        }
    });

    println!("{}", serde_json::to_string_pretty(&config).unwrap());
}

pub(super) fn run_mcp_command(args: &[String]) {
    let mut db_path = "agent_memory.tapir".to_string();
    let mut passphrase: Option<String> = None;
    let mut show_claude_config = false;
    let mut mcp_cfg = McpServerConfig {
        embed_endpoint: env::var("TAPIRUS_EMBED_ENDPOINT")
            .ok()
            .filter(|s| !s.trim().is_empty()),
        embed_model: env::var("TAPIRUS_EMBED_MODEL")
            .ok()
            .filter(|s| !s.trim().is_empty()),
        embed_dims: env::var("TAPIRUS_EMBED_DIMS")
            .ok()
            .and_then(|s| s.trim().parse::<usize>().ok()),
        embed_api_key: env::var("TAPIRUS_EMBED_API_KEY")
            .ok()
            .filter(|s| !s.trim().is_empty()),
    };
    let mut mcp_namespace: Option<String> = env::var("TAPIRUS_MCP_NAMESPACE")
        .ok()
        .filter(|s| !s.trim().is_empty());
    let mut mcp_session_id: Option<String> = env::var("TAPIRUS_MCP_SESSION_ID")
        .ok()
        .filter(|s| !s.trim().is_empty());

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" => {
                print_mcp_help();
                return;
            }
            "--config-claude" => {
                show_claude_config = true;
            }
            "--passphrase" => {
                if i + 1 < args.len() {
                    passphrase = Some(args[i + 1].clone());
                    i += 1;
                }
            }
            "--embed-endpoint" => {
                if i + 1 < args.len() {
                    mcp_cfg.embed_endpoint = Some(args[i + 1].clone());
                    i += 1;
                }
            }
            "--embed-model" => {
                if i + 1 < args.len() {
                    mcp_cfg.embed_model = Some(args[i + 1].clone());
                    i += 1;
                }
            }
            "--embed-dims" => {
                if i + 1 < args.len() {
                    let parsed = args[i + 1].parse::<usize>().unwrap_or_else(|_| {
                        eprintln!("Invalid --embed-dims value '{}'", args[i + 1]);
                        std::process::exit(1);
                    });
                    if parsed == 0 || parsed > 4096 {
                        eprintln!("--embed-dims must be between 1 and 4096");
                        std::process::exit(1);
                    }
                    mcp_cfg.embed_dims = Some(parsed);
                    i += 1;
                }
            }
            "--embed-api-key" => {
                if i + 1 < args.len() {
                    mcp_cfg.embed_api_key = Some(args[i + 1].clone());
                    i += 1;
                }
            }
            "--namespace" => {
                if i + 1 < args.len() {
                    mcp_namespace = Some(args[i + 1].clone());
                    i += 1;
                }
            }
            "--session-id" => {
                if i + 1 < args.len() {
                    mcp_session_id = Some(args[i + 1].clone());
                    i += 1;
                }
            }
            "-d" | "--database" => {
                if i + 1 < args.len() {
                    db_path = args[i + 1].clone();
                    i += 1;
                }
            }
            arg if !arg.starts_with('-') => {
                db_path = arg.to_string();
            }
            _ => {}
        }
        i += 1;
    }

    if show_claude_config {
        print_claude_config(&db_path);
        return;
    }
    if mcp_session_id.is_some() && mcp_namespace.is_none() {
        eprintln!(
            "--session-id / TAPIRUS_MCP_SESSION_ID requires --namespace / TAPIRUS_MCP_NAMESPACE"
        );
        std::process::exit(1);
    }

    let base_conn = if let Some(pass) = passphrase {
        match Connection::open_encrypted(Path::new(&db_path), &pass) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("Error opening encrypted database at '{db_path}': {e}");
                std::process::exit(1);
            }
        }
    } else {
        match Connection::open(Path::new(&db_path)) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("Error opening database at '{db_path}': {e}");
                std::process::exit(1);
            }
        }
    };
    let conn = if let Some(ref ns) = mcp_namespace {
        match base_conn.scoped_memory(ns, mcp_session_id.as_deref()) {
            Ok(scoped) => scoped,
            Err(e) => {
                eprintln!("Invalid MCP memory scope: {e}");
                std::process::exit(1);
            }
        }
    } else {
        base_conn
    };

    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let req: serde_json::Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let id = req.get("id").cloned();
        let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");

        match method {
            "initialize" => {
                let resp = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "protocolVersion": "2024-11-05",
                        "serverInfo": {
                            "name": "tapirus-mcp",
                            "version": VERSION
                        },
                        "capabilities": {
                            "tools": {}
                        }
                    }
                });
                let _ = writeln!(stdout, "{}", resp);
                let _ = stdout.flush();
            }
            "notifications/initialized" => {}
            "ping" => {
                let resp = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {}
                });
                let _ = writeln!(stdout, "{}", resp);
                let _ = stdout.flush();
            }
            "tools/list" => {
                let tools = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "tools": [
                            {
                                "name": "tapirus_remember",
                                "description": "Store a new memory, observation, or fact into TapirusDB AI Agent Memory.",
                                "inputSchema": {
                                    "type": "object",
                                    "properties": {
                                        "content": { "type": "string", "description": "The memory content or fact to remember" },
                                        "importance": { "type": "number", "description": "Priority score between 0.0 and 1.0 (default: 0.5)" },
                                        "tags": { "type": "array", "items": { "type": "string" }, "description": "Optional category tags" },
                                        "namespace": { "type": "string", "description": "Optional owner namespace filter/scope" },
                                        "session_id": { "type": "string", "description": "Optional session/thread identifier" },
                                        "external_id": { "type": "string", "description": "Optional idempotency key for upsert within namespace+session" },
                                        "embedding": { "type": "array", "items": { "type": "number" }, "description": "Optional dense neural embedding supplied directly by the caller" },
                                        "embedding_model": { "type": "string", "description": "Optional embedding model identifier to isolate vector spaces" }
                                    },
                                    "required": ["content"]
                                }
                            },
                            {
                                "name": "tapirus_recall",
                                "description": "Retrieve relevant memories using hybrid BM25 full-text keyword matching, dense neural vectors (when configured or supplied), and temporal recency decay.",
                                "inputSchema": {
                                    "type": "object",
                                    "properties": {
                                        "query": { "type": "string", "description": "Search query or topic to recall" },
                                        "limit": { "type": "integer", "description": "Maximum number of memories to return (default: 5)" },
                                        "tags": { "type": "array", "items": { "type": "string" }, "description": "Filter by tags" },
                                        "namespace": { "type": "string", "description": "Optional owner namespace filter/scope" },
                                        "session_id": { "type": "string", "description": "Optional session/thread identifier" },
                                        "embedding": { "type": "array", "items": { "type": "number" }, "description": "Optional dense neural query embedding supplied directly by the caller" },
                                        "embedding_model": { "type": "string", "description": "Optional embedding model identifier matching the stored vector space" }
                                    }
                                }
                            },
                            {
                                "name": "tapirus_sql",
                                "description": "Execute a SQL query against the TapirusDB single-file database.",
                                "inputSchema": {
                                    "type": "object",
                                    "properties": {
                                        "sql": { "type": "string", "description": "SQL statement (SELECT, INSERT, UPDATE, DELETE, CREATE TABLE)" }
                                    },
                                    "required": ["sql"]
                                }
                            },
                            {
                                "name": "tapirus_graph_neighbors",
                                "description": "Traverse entity connections in the TapirusDB embedded knowledge graph.",
                                "inputSchema": {
                                    "type": "object",
                                    "properties": {
                                        "node_id": { "type": "integer", "description": "ID of node to explore" },
                                        "direction": { "type": "string", "enum": ["outgoing", "incoming", "both"], "description": "Direction of edges" }
                                    },
                                    "required": ["node_id"]
                                }
                            },
                            {
                                "name": "tapirus_status",
                                "description": "Get database statistics, memory count, and table schema definitions.",
                                "inputSchema": {
                                    "type": "object",
                                    "properties": {}
                                }
                            }
                        ]
                    }
                });
                let _ = writeln!(stdout, "{}", tools);
                let _ = stdout.flush();
            }
            "tools/call" => {
                let params = req.get("params").cloned().unwrap_or(serde_json::json!({}));
                let tool_name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
                let tool_args = params
                    .get("arguments")
                    .cloned()
                    .unwrap_or(serde_json::json!({}));

                let tool_result =
                    handle_mcp_tool_call_with_config(&conn, tool_name, &tool_args, &mcp_cfg);
                let resp = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "content": [
                            {
                                "type": "text",
                                "text": tool_result
                            }
                        ]
                    }
                });
                let _ = writeln!(stdout, "{}", resp);
                let _ = stdout.flush();
            }
            _ => {
                if let Some(id_val) = id {
                    let err_resp = serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": id_val,
                        "error": {
                            "code": -32601,
                            "message": format!("Method '{method}' not found")
                        }
                    });
                    let _ = writeln!(stdout, "{}", err_resp);
                    let _ = stdout.flush();
                }
            }
        }
    }
}

pub(super) struct ResolvedMcpEmbedding {
    vector: Vec<f32>,
    model: String,
    mode: &'static str,
}

pub(super) fn parse_embedding_array_arg(
    raw: &serde_json::Value,
    expected_dims: Option<usize>,
) -> std::result::Result<Vec<f32>, String> {
    let arr = raw
        .as_array()
        .ok_or_else(|| "embedding must be a JSON array of finite numbers".to_string())?;
    if arr.is_empty() || arr.len() > 4096 {
        return Err("embedding length must be between 1 and 4096".to_string());
    }
    let mut vec = Vec::with_capacity(arr.len());
    for item in arr {
        let val = item
            .as_f64()
            .ok_or_else(|| "embedding elements must be numbers".to_string())?;
        let f = val as f32;
        if !f.is_finite() {
            return Err("embedding contains non-finite values".to_string());
        }
        vec.push(f);
    }
    if let Some(dims) = expected_dims
        && vec.len() != dims
    {
        return Err(format!(
            "embedding dimension mismatch: got {}, expected {}",
            vec.len(),
            dims
        ));
    }
    Ok(vec)
}

pub(super) fn decode_http_chunked_body(raw: &[u8]) -> std::result::Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut cursor = 0usize;
    while cursor < raw.len() {
        let Some(rel_end) = find_subsequence(&raw[cursor..], b"\r\n") else {
            return Err("Malformed chunked HTTP response from embedding server".to_string());
        };
        let size_line = std::str::from_utf8(&raw[cursor..cursor + rel_end])
            .map_err(|_| "Invalid UTF-8 in chunked embedding response".to_string())?
            .split(';')
            .next()
            .unwrap_or("")
            .trim();
        let chunk_size = usize::from_str_radix(size_line, 16)
            .map_err(|_| "Invalid chunk size from embedding server".to_string())?;
        cursor += rel_end + 2;
        if chunk_size == 0 {
            return Ok(out);
        }
        if cursor + chunk_size + 2 > raw.len() {
            return Err("Truncated chunked HTTP body from embedding server".to_string());
        }
        out.extend_from_slice(&raw[cursor..cursor + chunk_size]);
        cursor += chunk_size + 2;
    }
    Ok(out)
}

pub(super) fn parse_http_endpoint_authority_and_path(
    endpoint: &str,
) -> std::result::Result<(String, String), String> {
    let rest = endpoint.strip_prefix("http://").ok_or_else(|| {
        "Only plain http:// endpoints are handled by built-in TCP client".to_string()
    })?;
    let (authority, path) = match rest.split_once('/') {
        Some((auth, tail)) => (auth, format!("/{}", tail.trim_start_matches('/'))),
        None => (rest, "/api/embed".to_string()),
    };
    let authority = authority.trim();
    if authority.is_empty() {
        return Err("Embedding endpoint host must not be empty".to_string());
    }
    let normalized_path = if path == "/" {
        "/api/embed".to_string()
    } else {
        path
    };
    let connect_addr = if authority.starts_with('[') {
        if authority.contains("]:") {
            authority.to_string()
        } else {
            format!("{authority}:80")
        }
    } else if authority.contains(':') {
        authority.to_string()
    } else {
        format!("{authority}:80")
    };
    Ok((connect_addr, normalized_path))
}

pub(super) fn fetch_http_embedding_plain(
    endpoint: &str,
    model: &str,
    text: &str,
    expected_dims: Option<usize>,
    api_key: Option<&str>,
) -> std::result::Result<Vec<f32>, String> {
    let (connect_addr, path) = parse_http_endpoint_authority_and_path(endpoint)?;
    let payload = if path.ends_with("/api/embeddings") {
        serde_json::json!({ "model": model, "prompt": text })
    } else {
        serde_json::json!({ "model": model, "input": text })
    };
    let body_bytes = serde_json::to_vec(&payload).map_err(|e| e.to_string())?;
    let mut req = format!(
        "POST {path} HTTP/1.1\r\nHost: {connect_addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
        body_bytes.len()
    );
    if let Some(key) = api_key
        && !key.trim().is_empty()
    {
        req.push_str(&format!("Authorization: Bearer {}\r\n", key.trim()));
    }
    req.push_str("\r\n");

    let mut full_req = req.into_bytes();
    full_req.extend_from_slice(&body_bytes);

    let mut stream = TcpStream::connect(&connect_addr)
        .map_err(|e| format!("Failed connecting to embedding endpoint '{endpoint}': {e}"))?;
    let _ = stream.set_read_timeout(Some(HTTP_IO_TIMEOUT));
    let _ = stream.set_write_timeout(Some(HTTP_IO_TIMEOUT));
    stream
        .write_all(&full_req)
        .and_then(|_| stream.flush())
        .map_err(|e| format!("Failed sending embedding request: {e}"))?;

    let mut raw_resp = Vec::new();
    let mut header_end_opt = None;
    let mut expected_len = None;
    let mut is_chunked = false;
    let mut buf = [0u8; 4096];
    loop {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                raw_resp.extend_from_slice(&buf[..n]);
                if header_end_opt.is_none()
                    && let Some(end) = find_subsequence(&raw_resp, b"\r\n\r\n")
                {
                    header_end_opt = Some(end);
                    let headers_str = String::from_utf8_lossy(&raw_resp[..end]);
                    for line in headers_str.lines() {
                        let lower = line.to_ascii_lowercase();
                        if let Some((_, v)) = lower.split_once("content-length:") {
                            expected_len = v.trim().parse::<usize>().ok();
                        } else if lower.starts_with("transfer-encoding:")
                            && lower.contains("chunked")
                        {
                            is_chunked = true;
                        }
                    }
                }
                if let Some(end) = header_end_opt {
                    let body_len = raw_resp.len().saturating_sub(end + 4);
                    if let Some(len) = expected_len {
                        if body_len >= len {
                            break;
                        }
                    } else if is_chunked
                        && find_subsequence(&raw_resp[end + 4..], b"0\r\n\r\n").is_some()
                    {
                        break;
                    }
                }
            }
            Err(e) if !raw_resp.is_empty() && e.kind() == io::ErrorKind::ConnectionReset => {
                break;
            }
            Err(e) => return Err(format!("Failed reading embedding response: {e}")),
        }
    }
    let header_end = header_end_opt
        .ok_or_else(|| "Invalid HTTP response from embedding endpoint".to_string())?;
    let headers = std::str::from_utf8(&raw_resp[..header_end])
        .map_err(|_| "Non-UTF-8 HTTP headers from embedding endpoint".to_string())?;
    let status_line = headers.lines().next().unwrap_or("");
    let status_code = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(0);
    let body_slice = &raw_resp[header_end + 4..];
    let body = if is_chunked {
        decode_http_chunked_body(body_slice)?
    } else if let Some(len) = expected_len {
        body_slice[..len.min(body_slice.len())].to_vec()
    } else {
        body_slice.to_vec()
    };
    if !(200..300).contains(&status_code) {
        let preview = String::from_utf8_lossy(&body);
        return Err(format!(
            "Embedding endpoint returned HTTP {status_code}: {}",
            preview.trim()
        ));
    }
    let json: serde_json::Value = serde_json::from_slice(&body)
        .map_err(|e| format!("Invalid JSON from embedding endpoint: {e}"))?;
    let vec_val = json
        .get("embeddings")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .or_else(|| {
            json.get("data")
                .and_then(|v| v.as_array())
                .and_then(|arr| arr.first())
                .and_then(|item| item.get("embedding"))
        })
        .or_else(|| json.get("embedding"))
        .ok_or_else(|| {
            "Embedding response missing 'embeddings[0]', 'data[0].embedding', or 'embedding'"
                .to_string()
        })?;
    parse_embedding_array_arg(vec_val, expected_dims)
}

#[cfg(feature = "http-embeddings")]
pub(super) fn fetch_http_embedding(
    endpoint: &str,
    model: &str,
    text: &str,
    expected_dims: Option<usize>,
    api_key: Option<&str>,
) -> std::result::Result<Vec<f32>, String> {
    if endpoint.starts_with("http://") {
        return fetch_http_embedding_plain(endpoint, model, text, expected_dims, api_key);
    }
    if endpoint.starts_with("https://") {
        let dims = expected_dims.ok_or_else(|| {
            "HTTPS embedding endpoints require --embed-dims / TAPIRUS_EMBED_DIMS".to_string()
        })?;
        let engine = tapirus::HttpEmbeddingEngine::new(endpoint, model, dims);
        return engine
            .try_embed_text(text)
            .map_err(|e| format!("HTTPS embedding request failed: {e}"));
    }
    Err(format!(
        "Unsupported embedding endpoint URL scheme '{endpoint}'; expected http:// or https://"
    ))
}

#[cfg(not(feature = "http-embeddings"))]
pub(super) fn fetch_http_embedding(
    endpoint: &str,
    model: &str,
    text: &str,
    expected_dims: Option<usize>,
    api_key: Option<&str>,
) -> std::result::Result<Vec<f32>, String> {
    if endpoint.starts_with("http://") {
        return fetch_http_embedding_plain(endpoint, model, text, expected_dims, api_key);
    }
    if endpoint.starts_with("https://") {
        return Err(
            "HTTPS embedding endpoints require building TapirusDB with '--features http-embeddings' (or point --embed-endpoint at a local http:// Ollama/TEI proxy)".to_string(),
        );
    }
    Err(format!(
        "Unsupported embedding endpoint URL scheme '{endpoint}'; expected http:// or https://"
    ))
}

pub(super) fn resolve_mcp_embedding(
    text: &str,
    args: &serde_json::Value,
    cfg: &McpServerConfig,
) -> std::result::Result<ResolvedMcpEmbedding, String> {
    let explicit_model = args
        .get("embedding_model")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);

    if let Some(raw_vec) = args.get("embedding")
        && !raw_vec.is_null()
    {
        let vector = parse_embedding_array_arg(raw_vec, cfg.embed_dims)?;
        let model = explicit_model
            .or_else(|| cfg.embed_model.clone())
            .unwrap_or_else(|| format!("caller-{}d", vector.len()));
        return Ok(ResolvedMcpEmbedding {
            vector,
            model,
            mode: "caller-vector",
        });
    }

    if let Some(ref endpoint) = cfg.embed_endpoint {
        let model = explicit_model
            .or_else(|| cfg.embed_model.clone())
            .unwrap_or_else(|| "nomic-embed-text".to_string());
        let vector = fetch_http_embedding(
            endpoint,
            &model,
            text,
            cfg.embed_dims,
            cfg.embed_api_key.as_deref(),
        )?;
        return Ok(ResolvedMcpEmbedding {
            vector,
            model,
            mode: "neural-http",
        });
    }

    let embedder = tapirus::DeterministicHashEmbedder::default();
    Ok(ResolvedMcpEmbedding {
        vector: embedder.embed_text(text),
        model: explicit_model.unwrap_or_else(|| "deterministic-hash-128d".to_string()),
        mode: "deterministic-hash-fallback",
    })
}

#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn handle_mcp_tool_call(
    conn: &Connection,
    tool_name: &str,
    args: &serde_json::Value,
) -> String {
    handle_mcp_tool_call_with_config(conn, tool_name, args, &McpServerConfig::default())
}

pub(super) fn handle_mcp_tool_call_with_config(
    conn: &Connection,
    tool_name: &str,
    args: &serde_json::Value,
    cfg: &McpServerConfig,
) -> String {
    match tool_name {
        "tapirus_remember" => {
            let content = args.get("content").and_then(|v| v.as_str()).unwrap_or("");
            if content.trim().is_empty() {
                return serde_json::json!({
                    "status": "error",
                    "message": "Memory content must not be empty"
                })
                .to_string();
            }
            let importance = args
                .get("importance")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.5) as f32;
            let namespace = args
                .get("namespace")
                .and_then(|v| v.as_str())
                .map(str::to_owned);
            let session_id = args
                .get("session_id")
                .and_then(|v| v.as_str())
                .map(str::to_owned);
            let external_id = args
                .get("external_id")
                .and_then(|v| v.as_str())
                .map(str::to_owned);
            let tags: Vec<String> = args
                .get("tags")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|t| t.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();

            let resolved = match resolve_mcp_embedding(content, args, cfg) {
                Ok(resolved) => resolved,
                Err(err) => {
                    return serde_json::json!({ "status": "error", "message": err }).to_string();
                }
            };
            let dims = resolved.vector.len();
            let doc = tapirus::MemoryDocument {
                content: content.to_string(),
                embedding: Some(resolved.vector),
                importance,
                tags: tags.clone(),
                namespace,
                session_id,
                external_id: external_id.clone(),
                embedding_model: Some(resolved.model.clone()),
                ..Default::default()
            };
            let write_res = if external_id.is_some() {
                conn.upsert_memory(&doc)
            } else {
                conn.store_memory(&doc)
            };
            match write_res {
                Ok(id) => {
                    let stored = conn.memory_get(id);
                    serde_json::json!({
                        "status": "success",
                        "memory_id": id,
                        "stored_content": content,
                        "importance": importance,
                        "tags": tags,
                        "namespace": stored.as_ref().and_then(|e| e.namespace.clone()),
                        "session_id": stored.as_ref().and_then(|e| e.session_id.clone()),
                        "embedding_model": resolved.model,
                        "embedding_dims": dims,
                        "embedding_mode": resolved.mode,
                    })
                    .to_string()
                }
                Err(e) => {
                    serde_json::json!({ "status": "error", "message": format!("{e}") }).to_string()
                }
            }
        }
        "tapirus_recall" => {
            let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
            let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(5) as usize;
            let namespace = args
                .get("namespace")
                .and_then(|v| v.as_str())
                .map(str::to_owned);
            let session_id = args
                .get("session_id")
                .and_then(|v| v.as_str())
                .map(str::to_owned);
            let tags: Vec<String> = args
                .get("tags")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|t| t.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();

            let resolved = match resolve_mcp_embedding(query, args, cfg) {
                Ok(resolved) => resolved,
                Err(err) => {
                    return serde_json::json!({ "status": "error", "message": err }).to_string();
                }
            };
            let dims = resolved.vector.len();
            let search = tapirus::MemorySearch {
                query: (!query.trim().is_empty()).then(|| query.to_string()),
                embedding: Some(resolved.vector),
                limit,
                namespace,
                session_id,
                tags,
                embedding_model: Some(resolved.model.clone()),
                ..Default::default()
            };
            match conn.search_memories_detailed(&search) {
                Ok(response) => {
                    let formatted: Vec<serde_json::Value> = response
                        .results
                        .into_iter()
                        .map(|r| {
                            serde_json::json!({
                                "id": r.entry.id,
                                "content": r.entry.content,
                                "combined_score": r.combined_score,
                                "semantic_score": r.semantic_score,
                                "lexical_score": r.lexical_score,
                                "recency_score": r.recency_score,
                                "timestamp": r.entry.timestamp,
                                "tags": r.entry.tags,
                                "namespace": r.entry.namespace,
                                "session_id": r.entry.session_id,
                                "is_associative": r.is_associative,
                            })
                        })
                        .collect();

                    serde_json::json!({
                        "status": "success",
                        "count": formatted.len(),
                        "embedding_model": resolved.model,
                        "embedding_dims": dims,
                        "embedding_mode": resolved.mode,
                        "vector_candidates": response.diagnostics.vector_candidates,
                        "lexical_candidates": response.diagnostics.lexical_candidates,
                        "memories": formatted
                    })
                    .to_string()
                }
                Err(e) => {
                    serde_json::json!({ "status": "error", "message": format!("{e}") }).to_string()
                }
            }
        }
        "tapirus_sql" => {
            let sql = args.get("sql").and_then(|v| v.as_str()).unwrap_or("");
            if conn.memory_scope().is_some()
                && sql.to_ascii_lowercase().contains("__sys_memory")
            {
                return serde_json::json!({
                    "status": "error",
                    "message": "Permission denied: direct SQL access to __sys_memory is forbidden on a namespace-scoped MCP server"
                })
                .to_string();
            }
            if sql.trim_start().to_uppercase().starts_with("SELECT") {
                match conn.query(sql) {
                    Ok(rows) => {
                        let json_rows: Vec<serde_json::Value> = rows.iter().map(|r| {
                            let mut map = serde_json::Map::new();
                            for (col, val) in r.columns().iter().zip(r.values().iter()) {
                                map.insert(col.clone(), serde_json::json!(format!("{val}")));
                            }
                            serde_json::Value::Object(map)
                        }).collect();
                        serde_json::json!({
                            "status": "success",
                            "row_count": json_rows.len(),
                            "rows": json_rows
                        }).to_string()
                    }
                    Err(e) => serde_json::json!({ "status": "error", "message": format!("{e}") }).to_string(),
                }
            } else {
                match conn.execute(sql) {
                    Ok(affected) => serde_json::json!({
                        "status": "success",
                        "rows_affected": affected
                    }).to_string(),
                    Err(e) => serde_json::json!({ "status": "error", "message": format!("{e}") }).to_string(),
                }
            }
        }
        "tapirus_graph_neighbors" => {
            let node_id = args.get("node_id").and_then(|v| v.as_u64()).unwrap_or(0);
            let dir_str = args.get("direction").and_then(|v| v.as_str()).unwrap_or("outgoing");
            let dir = match dir_str {
                "incoming" => tapirus::Direction::Incoming,
                "both" => tapirus::Direction::Both,
                _ => tapirus::Direction::Outgoing,
            };

            let neighbors = conn.graph_neighbors(node_id, dir, None);
            let json_neighbors: Vec<serde_json::Value> = neighbors.into_iter().map(|(n, e)| {
                serde_json::json!({
                    "neighbor_id": n.id,
                    "label": n.label,
                    "edge_id": e.id,
                    "edge_label": e.label,
                    "weight": e.weight,
                })
            }).collect();

            serde_json::json!({
                "status": "success",
                "center_node": node_id,
                "neighbor_count": json_neighbors.len(),
                "neighbors": json_neighbors
            }).to_string()
        }
        "tapirus_status" => {
            let tables = conn.tables();
            let mem_count = conn.memory_count();
            serde_json::json!({
                "status": "operational",
                "engine": "TapirusDB",
                "version": VERSION,
                "memory_safety": "Core crate forbids unsafe code; C FFI crate contains unsafe code",
                "page_size_bytes": 4096,
                "stored_memories_count": mem_count,
                "tables_count": tables.len(),
                "tables": tables.into_iter().map(|t| t.name).collect::<Vec<String>>()
            }).to_string()
        }
        _ => serde_json::json!({ "status": "error", "message": format!("Unknown tool '{tool_name}'") }).to_string(),
    }
}
