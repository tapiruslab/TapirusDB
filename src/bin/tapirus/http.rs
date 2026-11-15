use super::*;

pub(super) const HTTP_MAX_CLIENTS: usize = 64;
pub(super) const HTTP_MAX_HEADER_BYTES: usize = 16 * 1024;
pub(super) const HTTP_MAX_BODY_BYTES: usize = 4 * 1024 * 1024;
pub(super) const HTTP_IO_TIMEOUT: Duration = Duration::from_secs(10);

pub(super) struct ActiveHttpClient(Arc<AtomicUsize>);

impl Drop for ActiveHttpClient {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Release);
    }
}

pub(super) fn try_acquire_http_client(active: &Arc<AtomicUsize>) -> Option<ActiveHttpClient> {
    let mut count = active.load(Ordering::Acquire);
    loop {
        if count >= HTTP_MAX_CLIENTS {
            return None;
        }
        match active.compare_exchange_weak(count, count + 1, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return Some(ActiveHttpClient(Arc::clone(active))),
            Err(observed) => count = observed,
        }
    }
}

pub(super) trait HttpTransportStream: Read + Write {
    fn set_io_read_timeout(&mut self, timeout: Duration);
}

impl HttpTransportStream for TcpStream {
    fn set_io_read_timeout(&mut self, timeout: Duration) {
        let _ = self.set_read_timeout(Some(timeout));
    }
}

#[cfg(feature = "tls")]
impl HttpTransportStream for rustls::StreamOwned<rustls::ServerConnection, TcpStream> {
    fn set_io_read_timeout(&mut self, timeout: Duration) {
        let _ = self.sock.set_read_timeout(Some(timeout));
    }
}

#[cfg(feature = "tls")]
pub(super) fn load_tls_server_config(
    cert_path: &Path,
    key_path: &Path,
) -> std::result::Result<Arc<rustls::ServerConfig>, String> {
    use rustls::pki_types::pem::PemObject;
    use rustls::pki_types::{CertificateDer, PrivateKeyDer};

    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_file_iter(cert_path)
        .map_err(|e| {
            format!(
                "Failed to read TLS certificate '{}': {e}",
                cert_path.display()
            )
        })?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| {
            format!(
                "Failed to parse TLS certificate '{}': {e}",
                cert_path.display()
            )
        })?;
    if certs.is_empty() {
        return Err(format!(
            "No certificates found in '{}'",
            cert_path.display()
        ));
    }
    let key = PrivateKeyDer::from_pem_file(key_path).map_err(|e| {
        format!(
            "Failed to read TLS private key '{}': {e}",
            key_path.display()
        )
    })?;
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("TLS protocol configuration error: {e}"))?
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| format!("Invalid TLS certificate/key pair: {e}"))?;
    Ok(Arc::new(config))
}

pub(super) struct HttpServerConfig<'a> {
    pub(super) host: &'a str,
    pub(super) port: u16,
    pub(super) db_path: &'a str,
    pub(super) encrypted: bool,
    pub(super) api_key: Option<String>,
    pub(super) api_key_scopes: Vec<(String, String)>,
    pub(super) api_key_roles: Vec<ApiKeyRoleBinding>,
    pub(super) cors_origins: Vec<String>,
    pub(super) tls_cert: Option<String>,
    pub(super) tls_key: Option<String>,
}

pub(super) fn run_http_server(conn: Connection, cfg: HttpServerConfig<'_>) {
    let HttpServerConfig {
        host,
        port,
        db_path,
        encrypted,
        api_key,
        api_key_scopes,
        api_key_roles,
        cors_origins,
        tls_cert,
        tls_key,
    } = cfg;

    #[cfg(feature = "tls")]
    let tls_config = match (&tls_cert, &tls_key) {
        (Some(cert), Some(key)) => match load_tls_server_config(Path::new(cert), Path::new(key)) {
            Ok(cfg) => Some(cfg),
            Err(err) => {
                eprintln!("{err}");
                std::process::exit(1);
            }
        },
        (None, None) => None,
        _ => {
            eprintln!("Both --tls-cert and --tls-key must be supplied together.");
            std::process::exit(1);
        }
    };
    #[cfg(not(feature = "tls"))]
    if tls_cert.is_some() || tls_key.is_some() {
        eprintln!(
            "Native TLS requested (--tls-cert/--tls-key), but this binary was built without the 'tls' feature."
        );
        std::process::exit(1);
    }

    let addr = format!("{host}:{port}");
    let listener = match TcpListener::bind(&addr) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("Failed to bind server to {addr}: {e}");
            std::process::exit(1);
        }
    };

    #[cfg(feature = "tls")]
    let scheme = if tls_config.is_some() {
        "https"
    } else {
        "http"
    };
    #[cfg(not(feature = "tls"))]
    let scheme = "http";

    println!("{BANNER}");
    println!("TapirusDB HTTP server listening at {scheme}://{addr}");
    println!("Database: {db_path} (Encrypted: {encrypted})");
    if api_key.is_some() || !api_key_scopes.is_empty() || !api_key_roles.is_empty() {
        println!(
            "API key authentication is enabled (Bearer / X-API-Key; {} scoped namespace key(s), {} SQL RBAC key(s)).",
            api_key_scopes.len(),
            api_key_roles.len()
        );
    } else {
        println!("API key authentication is disabled; restrict the bind address accordingly.");
    }
    if cors_origins.is_empty() {
        println!(
            "CORS: Restricted (same-origin only; pass --cors-origin to allow cross-origin callers)."
        );
    } else {
        println!("CORS Allowed Origins: {}", cors_origins.join(", "));
    }
    #[cfg(feature = "tls")]
    if let Some(cert) = tls_cert.as_deref() {
        println!("TLS: Native Rustls HTTPS enabled (certificate: {cert}).");
    } else {
        println!(
            "TLS: Plaintext HTTP only; pass --tls-cert/--tls-key or terminate TLS at a reverse proxy for remote access."
        );
    }
    #[cfg(not(feature = "tls"))]
    println!(
        "TLS: Plaintext HTTP only; rebuild with '--features tls' or terminate TLS at a reverse proxy for remote access."
    );
    println!("📡 Endpoints:");
    println!("   • GET  /health   -> Healthcheck & Version (Public)");
    println!("   • POST /api/sql  -> Execute SQL / Vector Search (Master Key or --api-key-role)");
    println!(
        "   • POST /api/memory[/store|/search|/forget] -> AI Memory API (Protected, Namespace-Scoped)"
    );
    println!("   • GET  /         -> Built-in Web UI Console (Public)");
    println!("   • GET  /chat     -> Chat demo page (Public)");
    println!("   • POST /api/chat -> Chat handler (Master Key)");
    println!("   • GET  /api/chat/logs -> Inspect chat logs (Master Key)");
    println!("\nPress Ctrl+C to stop.\n");

    let db = Arc::new(Mutex::new(conn));
    let auth_key = Arc::new(api_key);
    let auth_scopes = Arc::new(api_key_scopes);
    let auth_roles = Arc::new(api_key_roles);
    let allowed_origins = Arc::new(cors_origins);
    let active_clients = Arc::new(AtomicUsize::new(0));

    for stream in listener.incoming() {
        match stream {
            Ok(mut stream) => {
                let Some(client_guard) = try_acquire_http_client(&active_clients) else {
                    send_http_response(
                        &mut stream,
                        "503 Service Unavailable",
                        "text/plain",
                        "Server busy",
                        None,
                    );
                    continue;
                };
                let _ = stream.set_read_timeout(Some(HTTP_IO_TIMEOUT));
                let _ = stream.set_write_timeout(Some(HTTP_IO_TIMEOUT));
                let db_clone = Arc::clone(&db);
                let auth_clone = Arc::clone(&auth_key);
                let auth_scopes_clone = Arc::clone(&auth_scopes);
                let auth_roles_clone = Arc::clone(&auth_roles);
                let cors_clone = Arc::clone(&allowed_origins);
                #[cfg(feature = "tls")]
                let tls_clone = tls_config.clone();
                thread::spawn(move || {
                    let _client_guard = client_guard;
                    #[cfg(feature = "tls")]
                    if let Some(cfg) = tls_clone {
                        let Ok(tls_conn) = rustls::ServerConnection::new(cfg) else {
                            return;
                        };
                        let tls_stream = rustls::StreamOwned::new(tls_conn, stream);
                        handle_http_client(
                            tls_stream,
                            db_clone,
                            auth_clone,
                            auth_scopes_clone,
                            auth_roles_clone,
                            cors_clone,
                        );
                        return;
                    }
                    handle_http_client(
                        stream,
                        db_clone,
                        auth_clone,
                        auth_scopes_clone,
                        auth_roles_clone,
                        cors_clone,
                    );
                });
            }
            Err(e) => {
                eprintln!("Connection error: {e}");
            }
        }
    }
}

pub(super) fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

pub(super) fn request_content_length(headers: &[u8]) -> std::result::Result<usize, &'static str> {
    let header_text = std::str::from_utf8(headers).map_err(|_| "Invalid HTTP headers")?;
    let mut length = None;
    for line in header_text.lines().skip(1) {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.eq_ignore_ascii_case("transfer-encoding") {
            return Err("Transfer-Encoding is not supported");
        }
        if name.eq_ignore_ascii_case("content-length") {
            if length.is_some() {
                return Err("Duplicate Content-Length");
            }
            length = Some(value.trim().parse().map_err(|_| "Invalid Content-Length")?);
        }
    }
    Ok(length.unwrap_or(0))
}

pub(super) fn constant_time_eq_str(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.as_bytes().ct_eq(b.as_bytes()).into()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum HttpAuthScope {
    Unrestricted,
    Namespace(String),
    SqlRole {
        access: SqlRoleAccess,
        allowed_tables: Option<Vec<String>>,
    },
}

pub(super) fn check_allowed_table(
    table: &str,
    cte_names: &[String],
    allowed_tables: Option<&[String]>,
) -> std::result::Result<(), String> {
    let lower = table.to_ascii_lowercase();
    if cte_names.iter().any(|c| c == &lower) {
        return Ok(());
    }
    if let Some(allowed) = allowed_tables {
        if !allowed.iter().any(|a| a == &lower) {
            return Err(format!(
                "Table '{table}' is outside this API key's RBAC allowlist."
            ));
        }
    } else if lower.starts_with("__sys_") {
        return Err(format!(
            "Internal system table '{table}' requires an unrestricted master API key."
        ));
    }
    Ok(())
}

pub(super) fn authorize_where_subqueries(
    expr: &WhereExpr,
    cte_names: &[String],
    allowed_tables: Option<&[String]>,
) -> std::result::Result<(), String> {
    match expr {
        WhereExpr::Condition(_) | WhereExpr::InList { .. } => Ok(()),
        WhereExpr::And(left, right) | WhereExpr::Or(left, right) => {
            authorize_where_subqueries(left, cte_names, allowed_tables)?;
            authorize_where_subqueries(right, cte_names, allowed_tables)
        }
        WhereExpr::InSubquery { subquery, .. } | WhereExpr::Exists { subquery, .. } => {
            authorize_sql_statement_ast(
                subquery,
                SqlRoleAccess::ReadOnly,
                cte_names,
                allowed_tables,
            )
        }
    }
}

pub(super) fn authorize_sql_statement_ast(
    stmt: &Statement,
    access: SqlRoleAccess,
    cte_names: &[String],
    allowed_tables: Option<&[String]>,
) -> std::result::Result<(), String> {
    match stmt {
        Statement::Select {
            table,
            from_subquery,
            join,
            where_clause,
            having,
            ..
        } => {
            if let Some((sub, _alias)) = from_subquery {
                authorize_sql_statement_ast(
                    sub,
                    SqlRoleAccess::ReadOnly,
                    cte_names,
                    allowed_tables,
                )?;
            } else {
                check_allowed_table(table, cte_names, allowed_tables)?;
            }
            if let Some(j) = join {
                check_allowed_table(&j.table, cte_names, allowed_tables)?;
            }
            if let Some(w) = where_clause {
                authorize_where_subqueries(w, cte_names, allowed_tables)?;
            }
            if let Some(h) = having {
                authorize_where_subqueries(h, cte_names, allowed_tables)?;
            }
            Ok(())
        }
        Statement::VectorSearch {
            table,
            where_clause,
            ..
        } => {
            check_allowed_table(table, cte_names, allowed_tables)?;
            if let Some(w) = where_clause {
                authorize_where_subqueries(w, cte_names, allowed_tables)?;
            }
            Ok(())
        }
        Statement::WithCte { ctes, main_query } => {
            let mut combined_ctes = cte_names.to_vec();
            for cte in ctes {
                if cte.recursive {
                    combined_ctes.push(cte.name.to_ascii_lowercase());
                }
                authorize_sql_statement_ast(
                    &cte.query,
                    SqlRoleAccess::ReadOnly,
                    &combined_ctes,
                    allowed_tables,
                )?;
                if !cte.recursive {
                    combined_ctes.push(cte.name.to_ascii_lowercase());
                }
            }
            authorize_sql_statement_ast(main_query, access, &combined_ctes, allowed_tables)
        }
        Statement::Compound { left, right, .. } => {
            authorize_sql_statement_ast(left, access, cte_names, allowed_tables)?;
            authorize_sql_statement_ast(right, access, cte_names, allowed_tables)
        }
        Statement::Explain { statement, .. } => {
            authorize_sql_statement_ast(statement, access, cte_names, allowed_tables)
        }
        Statement::GraphTraverse { where_clause, .. }
        | Statement::GraphMatch { where_clause, .. } => {
            if allowed_tables.is_some() {
                return Err(
                    "Table-scoped SQL API keys cannot run unscoped graph queries.".to_string(),
                );
            }
            if let Some(w) = where_clause {
                authorize_where_subqueries(w, cte_names, allowed_tables)?;
            }
            Ok(())
        }
        Statement::GraphShortestPath { .. } | Statement::GraphAlgorithm { .. } => {
            if allowed_tables.is_some() {
                return Err(
                    "Table-scoped SQL API keys cannot run unscoped graph queries.".to_string(),
                );
            }
            Ok(())
        }
        Statement::Pragma { value: None, .. } => {
            if allowed_tables.is_some() {
                return Err("Table-scoped SQL API keys cannot run PRAGMA statements.".to_string());
            }
            Ok(())
        }
        Statement::Insert { table, .. } | Statement::InsertMany { table, .. } => {
            if access == SqlRoleAccess::ReadOnly {
                return Err("Read-only API key cannot execute INSERT statements.".to_string());
            }
            check_allowed_table(table, cte_names, allowed_tables)
        }
        Statement::Update {
            table,
            where_clause,
            ..
        } => {
            if access == SqlRoleAccess::ReadOnly {
                return Err("Read-only API key cannot execute UPDATE statements.".to_string());
            }
            check_allowed_table(table, cte_names, allowed_tables)?;
            if let Some(w) = where_clause {
                authorize_where_subqueries(w, cte_names, allowed_tables)?;
            }
            Ok(())
        }
        Statement::Delete {
            table,
            where_clause,
            ..
        } => {
            if access == SqlRoleAccess::ReadOnly {
                return Err("Read-only API key cannot execute DELETE statements.".to_string());
            }
            check_allowed_table(table, cte_names, allowed_tables)?;
            if let Some(w) = where_clause {
                authorize_where_subqueries(w, cte_names, allowed_tables)?;
            }
            Ok(())
        }
        Statement::BeginTransaction
        | Statement::CommitTransaction
        | Statement::RollbackTransaction
        | Statement::Savepoint { .. }
        | Statement::RollbackToSavepoint { .. }
        | Statement::ReleaseSavepoint { .. } => {
            if access == SqlRoleAccess::ReadOnly {
                return Err(
                    "Read-only API key cannot open write transaction control statements."
                        .to_string(),
                );
            }
            Ok(())
        }
        _ => Err(
            "DDL and administrative SQL statements require an unrestricted master API key."
                .to_string(),
        ),
    }
}

pub(super) fn authorize_sql_for_role(
    sql: &str,
    params: &[Value],
    access: SqlRoleAccess,
    allowed_tables: Option<&[String]>,
) -> std::result::Result<(), String> {
    let tokens = tokenize(sql).map_err(|e| format!("Invalid SQL syntax: {e}"))?;
    let mut seen_semicolon = false;
    for tok in &tokens {
        if matches!(tok, Token::Semicolon) {
            seen_semicolon = true;
        } else if seen_semicolon {
            return Err(
                "Scoped SQL API keys only permit one SQL statement per HTTP request.".to_string(),
            );
        }
    }
    let bound =
        bind_parameters(&tokens, params).map_err(|e| format!("Invalid SQL parameters: {e}"))?;
    let stmt = parse_tokens(&bound).map_err(|e| format!("Invalid SQL statement: {e}"))?;
    authorize_sql_statement_ast(&stmt, access, &[], allowed_tables)
}

pub(super) fn extract_request_api_key(header_str: &str) -> Option<&str> {
    let mut provided_key = None;
    for line in header_str.lines() {
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("authorization:") {
            if let Some((_, value)) = line.split_once(':') {
                let value = value.trim();
                provided_key = Some(if value.to_ascii_lowercase().starts_with("bearer ") {
                    value[7..].trim()
                } else {
                    value
                });
            }
        } else if lower.starts_with("x-api-key:")
            && let Some((_, value)) = line.split_once(':')
        {
            provided_key = Some(value.trim());
        }
    }
    provided_key
}

#[cfg(test)]
pub(super) fn request_has_valid_api_key(expected_key: &str, header_str: &str) -> bool {
    extract_request_api_key(header_str).is_some_and(|key| constant_time_eq_str(key, expected_key))
}

pub(super) fn authenticate_http_request(
    master_key: Option<&str>,
    scoped_keys: &[(String, String)],
    role_keys: &[ApiKeyRoleBinding],
    header_str: &str,
) -> Option<HttpAuthScope> {
    if master_key.is_none() && scoped_keys.is_empty() && role_keys.is_empty() {
        return Some(HttpAuthScope::Unrestricted);
    }
    let provided = extract_request_api_key(header_str)?;
    let mut resolved = None;
    if let Some(expected) = master_key
        && constant_time_eq_str(provided, expected)
    {
        resolved = Some(HttpAuthScope::Unrestricted);
    }
    for (key, namespace) in scoped_keys {
        if constant_time_eq_str(provided, key) && resolved.is_none() {
            resolved = Some(HttpAuthScope::Namespace(namespace.clone()));
        }
    }
    for binding in role_keys {
        if constant_time_eq_str(provided, &binding.key) && resolved.is_none() {
            resolved = Some(HttpAuthScope::SqlRole {
                access: binding.access,
                allowed_tables: binding.allowed_tables.clone(),
            });
        }
    }
    resolved
}

pub(super) fn parse_http_sql_payload(
    body: &str,
) -> std::result::Result<(String, Vec<Value>), String> {
    let parsed = serde_json::from_str::<serde_json::Value>(body).ok();
    let (sql, raw_params) = match parsed {
        Some(serde_json::Value::String(sql)) => (sql, None),
        Some(serde_json::Value::Object(object)) => {
            let sql = object
                .get("sql")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| body.to_owned());
            (sql, object.get("params").cloned())
        }
        _ => (body.to_owned(), None),
    };
    let Some(raw_params) = raw_params else {
        return Ok((sql, Vec::new()));
    };
    let params = raw_params
        .as_array()
        .ok_or_else(|| "SQL params must be a JSON array".to_owned())?
        .iter()
        .map(Value::from_json_parameter)
        .collect::<tapirus::Result<Vec<_>>>()
        .map_err(|error| error.to_string())?;
    Ok((sql, params))
}

pub(super) fn is_sql_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    chars
        .next()
        .is_some_and(|first| first == '_' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

pub(super) fn parse_cors_origins(raw: &str) -> std::result::Result<Vec<String>, String> {
    let mut origins = Vec::new();
    for entry in raw.split(',') {
        let trimmed = entry.trim().trim_end_matches('/');
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.chars().any(|c| c.is_control()) {
            return Err("origin contains control characters".to_owned());
        }
        if trimmed != "*" && !trimmed.starts_with("http://") && !trimmed.starts_with("https://") {
            return Err(format!(
                "origin '{trimmed}' must be '*' or start with http:// or https://"
            ));
        }
        if !origins.iter().any(|existing| existing == trimmed) {
            origins.push(trimmed.to_owned());
        }
    }
    Ok(origins)
}

pub(super) fn resolve_cors_origin<'a>(
    header_str: &str,
    allowed_origins: &'a [String],
) -> Option<&'a str> {
    if allowed_origins.is_empty() {
        return None;
    }
    if allowed_origins.iter().any(|o| o == "*") {
        return Some("*");
    }
    let request_origin = header_str.lines().skip(1).find_map(|line| {
        let (name, value) = line.split_once(':')?;
        if name.trim().eq_ignore_ascii_case("origin") {
            Some(value.trim().trim_end_matches('/'))
        } else {
            None
        }
    })?;
    allowed_origins
        .iter()
        .find(|allowed| allowed.eq_ignore_ascii_case(request_origin))
        .map(String::as_str)
}

pub(super) fn map_memory_error_status(err: &tapirus::Error) -> &'static str {
    match err {
        tapirus::Error::PermissionDenied(_) => "403 Forbidden",
        tapirus::Error::ConstraintViolation(_) => "400 Bad Request",
        _ => "500 Internal Server Error",
    }
}

pub(super) fn handle_http_memory_route(
    conn: &Connection,
    route_path: &str,
    body_str: &str,
    auth_scope: &HttpAuthScope,
) -> std::result::Result<String, (&'static str, String)> {
    let effective_conn = match auth_scope {
        HttpAuthScope::Unrestricted => conn.clone(),
        HttpAuthScope::Namespace(ns) => conn
            .scoped_memory(ns, None)
            .map_err(|err| (map_memory_error_status(&err), err.to_string()))?,
        HttpAuthScope::SqlRole { .. } => {
            return Err((
                "403 Forbidden",
                "SQL-role API keys cannot access /api/memory endpoints.".to_string(),
            ));
        }
    };
    match route_path {
        "/api/memory" => {
            let envelope: serde_json::Value = serde_json::from_str(body_str).map_err(|err| {
                (
                    "400 Bad Request",
                    format!("Invalid memory request JSON: {err}"),
                )
            })?;
            let operation = envelope
                .get("operation")
                .or_else(|| envelope.get("action"))
                .and_then(|v| v.as_str())
                .ok_or((
                    "400 Bad Request",
                    "Missing string 'operation' (or 'action') field".to_string(),
                ))?;
            let payload_str = match envelope.get("payload") {
                Some(serde_json::Value::String(raw)) => raw.clone(),
                Some(other) => other.to_string(),
                None => "{}".to_string(),
            };
            effective_conn
                .memory_request_json(operation, &payload_str)
                .map_err(|err| (map_memory_error_status(&err), err.to_string()))
        }
        "/api/memory/store" => {
            #[derive(serde::Deserialize)]
            struct HttpStorePayload {
                #[serde(flatten)]
                document: tapirus::MemoryDocument,
                #[serde(default)]
                upsert: bool,
            }
            let payload: HttpStorePayload = serde_json::from_str(body_str)
                .map_err(|err| ("400 Bad Request", format!("Invalid memory document: {err}")))?;
            let id = if payload.upsert {
                effective_conn.upsert_memory(&payload.document)
            } else {
                effective_conn.store_memory(&payload.document)
            }
            .map_err(|err| (map_memory_error_status(&err), err.to_string()))?;
            Ok(serde_json::json!({
                "status": "success",
                "memory_id": id,
                "namespace": effective_conn.memory_scope().map(|s| s.namespace.as_str()).or(payload.document.namespace.as_deref()),
            })
            .to_string())
        }
        "/api/memory/search" => {
            let search: tapirus::MemorySearch = serde_json::from_str(body_str)
                .map_err(|err| ("400 Bad Request", format!("Invalid memory search: {err}")))?;
            let response = effective_conn
                .search_memories_detailed(&search)
                .map_err(|err| (map_memory_error_status(&err), err.to_string()))?;
            serde_json::to_string(&response)
                .map_err(|err| ("500 Internal Server Error", err.to_string()))
        }
        "/api/memory/forget" => {
            let id = if let Ok(id) = serde_json::from_str::<u64>(body_str) {
                id
            } else {
                let value: serde_json::Value = serde_json::from_str(body_str)
                    .map_err(|err| ("400 Bad Request", format!("Invalid forget payload: {err}")))?;
                value
                    .get("id")
                    .and_then(|v| v.as_u64())
                    .ok_or(("400 Bad Request", "Missing numeric memory 'id'".to_string()))?
            };
            let deleted = effective_conn
                .memory_forget_checked(id)
                .map_err(|err| (map_memory_error_status(&err), err.to_string()))?;
            Ok(serde_json::json!({
                "status": "success",
                "deleted": deleted,
            })
            .to_string())
        }
        _ => Err(("404 Not Found", "Not Found".to_string())),
    }
}

pub(super) fn handle_http_client<S: HttpTransportStream>(
    mut stream: S,
    db: Arc<Mutex<Connection>>,
    api_key: Arc<Option<String>>,
    api_key_scopes: Arc<Vec<(String, String)>>,
    api_key_roles: Arc<Vec<ApiKeyRoleBinding>>,
    allowed_origins: Arc<Vec<String>>,
) {
    let mut request_data = Vec::new();
    let mut buf = [0u8; 4096];
    let mut body_start = None;
    let mut content_length: usize = 0;
    let request_started = Instant::now();

    loop {
        let remaining = HTTP_IO_TIMEOUT.saturating_sub(request_started.elapsed());
        if remaining.is_zero() {
            send_http_response(
                &mut stream,
                "408 Request Timeout",
                "text/plain",
                "Request timed out",
                None,
            );
            return;
        }
        stream.set_io_read_timeout(remaining);
        let n = match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(error)
                if error.kind() == io::ErrorKind::TimedOut
                    || error.kind() == io::ErrorKind::WouldBlock =>
            {
                send_http_response(
                    &mut stream,
                    "408 Request Timeout",
                    "text/plain",
                    "Request timed out",
                    None,
                );
                return;
            }
            Err(_) => return,
        };
        request_data.extend_from_slice(&buf[..n]);

        if body_start.is_none() {
            if let Some(pos) = find_subsequence(&request_data, b"\r\n\r\n") {
                body_start = Some(pos + 4);
            } else if let Some(pos) = find_subsequence(&request_data, b"\n\n") {
                body_start = Some(pos + 2);
            }

            if let Some(start) = body_start {
                if start > HTTP_MAX_HEADER_BYTES {
                    send_http_response(
                        &mut stream,
                        "431 Request Header Fields Too Large",
                        "text/plain",
                        "Request headers too large",
                        None,
                    );
                    return;
                }
                content_length = match request_content_length(&request_data[..start]) {
                    Ok(length) => length,
                    Err(message) => {
                        send_http_response(
                            &mut stream,
                            "400 Bad Request",
                            "text/plain",
                            message,
                            None,
                        );
                        return;
                    }
                };
            }
        }

        if body_start.is_none() && request_data.len() > HTTP_MAX_HEADER_BYTES {
            send_http_response(
                &mut stream,
                "431 Request Header Fields Too Large",
                "text/plain",
                "Request headers too large",
                None,
            );
            return;
        }
        if content_length > HTTP_MAX_BODY_BYTES
            || request_data.len() > HTTP_MAX_HEADER_BYTES + HTTP_MAX_BODY_BYTES
        {
            send_http_response(
                &mut stream,
                "413 Content Too Large",
                "text/plain",
                "Request body too large",
                None,
            );
            return;
        }

        if let Some(start) = body_start
            && request_data.len() >= start + content_length
        {
            break;
        }
    }

    if request_data.is_empty() {
        return;
    }

    let Some(start) = body_start else {
        send_http_response(
            &mut stream,
            "400 Bad Request",
            "text/plain",
            "Incomplete HTTP headers",
            None,
        );
        return;
    };
    if request_data.len() < start + content_length {
        send_http_response(
            &mut stream,
            "400 Bad Request",
            "text/plain",
            "Incomplete request body",
            None,
        );
        return;
    }

    let body_offset = start;
    let header_bytes = &request_data[..body_offset];
    let body_bytes = if body_offset < request_data.len() {
        &request_data
            [body_offset..body_offset + content_length.min(request_data.len() - body_offset)]
    } else {
        b""
    };

    let header_str = match std::str::from_utf8(header_bytes) {
        Ok(s) => s,
        Err(_) => return,
    };

    let mut lines = header_str.lines();
    let first_line = match lines.next() {
        Some(l) => l,
        None => return,
    };

    let mut parts = first_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let full_path = parts.next().unwrap_or("/");
    let route_path = full_path
        .split_once('?')
        .map_or(full_path, |(path, _)| path);
    let cors_origin = resolve_cors_origin(header_str, &allowed_origins);

    // Handle CORS preflight
    if method == "OPTIONS" {
        send_http_response(&mut stream, "204 No Content", "text/plain", "", cors_origin);
        return;
    }

    // Healthcheck endpoint (public by design)
    if method == "GET" && (route_path == "/health" || route_path == "/api/health") {
        let json = serde_json::json!({
            "status": "ok",
            "engine": "TapirusDB",
            "version": VERSION
        });
        send_http_response(
            &mut stream,
            "200 OK",
            "application/json",
            &json.to_string(),
            cors_origin,
        );
        return;
    }

    // Built-in UI page (public by design)
    if method == "GET" && (route_path == "/" || route_path == "/index.html") {
        let html = r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>TapirusDB Server</title>
<style>
body { font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif; background: #0f172a; color: #f8fafc; display: flex; align-items: center; justify-content: center; min-height: 100vh; margin: 0; padding: 20px; box-sizing: border-box; }
.card { background: #1e293b; padding: 2.5rem; border-radius: 14px; border: 1px solid #334155; text-align: center; max-width: 520px; box-shadow: 0 20px 35px rgba(0,0,0,0.35); }
.badge { display: inline-block; background: #0284c7; color: #ffffff; padding: 4px 12px; border-radius: 999px; font-size: 0.8rem; font-weight: 600; margin-bottom: 1.25rem; letter-spacing: 0.04em; }
h1 { margin: 0 0 0.75rem; font-size: 1.65rem; color: #38bdf8; font-weight: 700; }
p { color: #94a3b8; font-size: 0.95rem; line-height: 1.6; margin: 0 0 1.25rem; }
code { background: #0f172a; color: #38bdf8; padding: 2px 6px; border-radius: 4px; font-family: ui-monospace, SFMono-Regular, Menlo, monospace; font-size: 0.88em; }
.btn-group { display: flex; gap: 10px; justify-content: center; margin-top: 1.5rem; }
.btn { display: inline-flex; align-items: center; justify-content: center; background: #0284c7; color: #ffffff; text-decoration: none; padding: 10px 18px; border-radius: 8px; font-weight: 600; font-size: 0.9rem; transition: background 0.2s; }
.btn:hover { background: #0369a1; }
.btn-secondary { background: #334155; color: #f8fafc; }
.btn-secondary:hover { background: #475569; }
</style>
</head>
<body>
<div class="card">
<div class="badge">TAPIRUSDB IN-PROCESS ENGINE</div>
<h1>TapirusDB HTTP Server</h1>
<p>The native engine daemon is active and ready to process Relational SQL, HNSW Vector embeddings, and openCypher Graph queries.</p>
<p>API Endpoints: <code>/sql</code> &bull; <code>/api/sql</code> &bull; <code>/chat</code> &bull; <code>/api/chat</code> &bull; <code>/health</code></p>
<div class="btn-group">
<a href="/chat" class="btn" style="background:#10b981;">AI Cognitive Chatbot</a>
<a href="https://tapirusdb.com" target="_blank" class="btn">Official Website</a>
<a href="https://tapirusdb.com/docs.html" target="_blank" class="btn btn-secondary">Documentation</a>
</div>
</div>
</body>
</html>"#;
        send_http_response(
            &mut stream,
            "200 OK",
            "text/html; charset=utf-8",
            html,
            cors_origin,
        );
        return;
    }

    // Built-in Documentation Portal - Clean Redirect (Public)
    if method == "GET" && (route_path == "/docs" || route_path == "/docs.html") {
        let html = r#"<!DOCTYPE html><html><head><meta http-equiv="refresh" content="0; url=https://tapirusdb.com/docs.html"></head><body>Redirecting to <a href="https://tapirusdb.com/docs.html">TapirusDB Documentation</a>...</body></html>"#;
        send_http_response(
            &mut stream,
            "200 OK",
            "text/html; charset=utf-8",
            html,
            cors_origin,
        );
        return;
    }

    // Chat demo page (public by design)
    if method == "GET" && (route_path == "/chat" || route_path == "/chat.html") {
        send_http_response(
            &mut stream,
            "200 OK",
            "text/html; charset=utf-8",
            CHATBOT_HTML,
            cors_origin,
        );
        return;
    }

    // User chat and persisted chat logs require unrestricted master access when auth is enabled.
    let protected_chat_route = (method == "GET"
        && (route_path == "/api/chat/logs" || route_path == "/chat/logs"))
        || (method == "POST" && (route_path == "/chat" || route_path == "/api/chat"));
    if protected_chat_route {
        match authenticate_http_request(
            api_key.as_deref(),
            &api_key_scopes,
            &api_key_roles,
            header_str,
        ) {
            Some(HttpAuthScope::Unrestricted) => {}
            Some(HttpAuthScope::Namespace(_) | HttpAuthScope::SqlRole { .. }) => {
                let err_json = serde_json::json!({
                    "error": "Forbidden",
                    "message": "Scoped API keys may not access master chat endpoints."
                });
                send_http_response(
                    &mut stream,
                    "403 Forbidden",
                    "application/json",
                    &err_json.to_string(),
                    cors_origin,
                );
                return;
            }
            None => {
                let err_json = serde_json::json!({"error":"Unauthorized","message":"Missing or invalid API key."});
                send_http_unauthorized_response(&mut stream, &err_json.to_string(), cors_origin);
                return;
            }
        }
    }

    // Chat logs endpoint (protected when API key is configured)
    if method == "GET" && (route_path == "/api/chat/logs" || route_path == "/chat/logs") {
        let query_res = {
            let conn = db.lock();
            let _ = ensure_chatbot_schema(&conn);
            let sql = "SELECT id, session_id, user_message, bot_reply, intent, confidence, is_safe, latency_us, created_at FROM tap_chat_logs ORDER BY id DESC LIMIT 25;";
            conn.query(sql)
        };
        match query_res {
            Ok(rows) => {
                let clean_rows: Vec<serde_json::Value> = rows
                    .iter()
                    .map(|r| {
                        let mut map = serde_json::Map::new();
                        for (col, val) in r.columns().iter().zip(r.values().iter()) {
                            map.insert(col.clone(), value_to_json(val));
                        }
                        serde_json::Value::Object(map)
                    })
                    .collect();
                let res = serde_json::json!({ "logs": clean_rows });
                send_http_response(
                    &mut stream,
                    "200 OK",
                    "application/json",
                    &res.to_string(),
                    cors_origin,
                );
            }
            Err(e) => {
                let res = serde_json::json!({ "error": e.to_string() });
                send_http_response(
                    &mut stream,
                    "500 Internal Server Error",
                    "application/json",
                    &res.to_string(),
                    cors_origin,
                );
            }
        }
        return;
    }

    // Chat API (protected when API key is configured)
    if method == "POST" && (route_path == "/chat" || route_path == "/api/chat") {
        let body_str = std::str::from_utf8(body_bytes).unwrap_or("");
        match handle_chatbot_request(Arc::clone(&db), body_str) {
            Ok(json_res) => {
                send_http_response(
                    &mut stream,
                    "200 OK",
                    "application/json",
                    &json_res.to_string(),
                    cors_origin,
                );
            }
            Err(err_msg) => {
                let res = serde_json::json!({ "error": err_msg });
                send_http_response(
                    &mut stream,
                    "400 Bad Request",
                    "application/json",
                    &res.to_string(),
                    cors_origin,
                );
            }
        }
        return;
    }

    // Authentication enforcement for protected endpoints
    let auth_scope = match authenticate_http_request(
        api_key.as_deref(),
        &api_key_scopes,
        &api_key_roles,
        header_str,
    ) {
        Some(scope) => scope,
        None => {
            let err_json = serde_json::json!({
                "error": "Unauthorized",
                "message": "Missing or invalid API key. Provide Authorization: Bearer or X-API-Key."
            });
            send_http_unauthorized_response(&mut stream, &err_json.to_string(), cors_origin);
            return;
        }
    };

    // AI Memory API endpoints (enforcing owner isolation when authenticated via --api-key-namespace)
    if method == "POST"
        && matches!(
            route_path,
            "/api/memory" | "/api/memory/store" | "/api/memory/search" | "/api/memory/forget"
        )
    {
        let body_str = std::str::from_utf8(body_bytes).unwrap_or("").trim();
        let mem_res = {
            let conn = db.lock();
            handle_http_memory_route(&conn, route_path, body_str, &auth_scope)
        };
        match mem_res {
            Ok(json_body) => {
                send_http_response(
                    &mut stream,
                    "200 OK",
                    "application/json",
                    &json_body,
                    cors_origin,
                );
            }
            Err((status, err_msg)) => {
                let label = if status.starts_with("403") {
                    "Forbidden"
                } else if status.starts_with("400") {
                    "Bad Request"
                } else {
                    "Error"
                };
                let res = serde_json::json!({ "error": label, "message": err_msg });
                send_http_response(
                    &mut stream,
                    status,
                    "application/json",
                    &res.to_string(),
                    cors_origin,
                );
            }
        }
        return;
    }

    if matches!(auth_scope, HttpAuthScope::Namespace(_)) {
        let err_json = serde_json::json!({
            "error": "Forbidden",
            "message": "Tenant-scoped API keys may only access /api/memory endpoints."
        });
        send_http_response(
            &mut stream,
            "403 Forbidden",
            "application/json",
            &err_json.to_string(),
            cors_origin,
        );
        return;
    }

    // SQL execution endpoint (Protected)
    if method == "POST" && (route_path == "/sql" || route_path == "/api/sql") {
        let body_str = std::str::from_utf8(body_bytes).unwrap_or("").trim();
        let (sql, params) = match parse_http_sql_payload(body_str) {
            Ok(payload) => payload,
            Err(error) => {
                let res = serde_json::json!({"error": error});
                send_http_response(
                    &mut stream,
                    "400 Bad Request",
                    "application/json",
                    &res.to_string(),
                    cors_origin,
                );
                return;
            }
        };

        let trimmed = sql.trim();
        if let HttpAuthScope::SqlRole {
            access,
            ref allowed_tables,
        } = auth_scope
            && let Err(reason) =
                authorize_sql_for_role(trimmed, &params, access, allowed_tables.as_deref())
        {
            let err_json = serde_json::json!({
                "error": "Forbidden",
                "message": reason
            });
            send_http_response(
                &mut stream,
                "403 Forbidden",
                "application/json",
                &err_json.to_string(),
                cors_origin,
            );
            return;
        }

        if trimmed.to_uppercase().starts_with("SELECT") {
            let query_res = {
                let conn = db.lock();
                conn.query_with_params(trimmed, &params)
            };
            match query_res {
                Ok(rows) => {
                    let clean_rows: Vec<serde_json::Value> = rows
                        .iter()
                        .map(|r| {
                            let mut map = serde_json::Map::new();
                            for (col, val) in r.columns().iter().zip(r.values().iter()) {
                                map.insert(col.clone(), value_to_json(val));
                            }
                            serde_json::Value::Object(map)
                        })
                        .collect();

                    let res = serde_json::json!({ "rows": clean_rows });
                    send_http_response(
                        &mut stream,
                        "200 OK",
                        "application/json",
                        &res.to_string(),
                        cors_origin,
                    );
                }
                Err(e) => {
                    let res = serde_json::json!({ "error": e.to_string() });
                    send_http_response(
                        &mut stream,
                        "400 Bad Request",
                        "application/json",
                        &res.to_string(),
                        cors_origin,
                    );
                }
            }
        } else {
            let exec_res = {
                let conn = db.lock();
                conn.execute_with_params(trimmed, &params)
            };
            match exec_res {
                Ok(affected) => {
                    let res = serde_json::json!({ "affected": affected });
                    send_http_response(
                        &mut stream,
                        "200 OK",
                        "application/json",
                        &res.to_string(),
                        cors_origin,
                    );
                }
                Err(e) => {
                    let res = serde_json::json!({ "error": e.to_string() });
                    send_http_response(
                        &mut stream,
                        "400 Bad Request",
                        "application/json",
                        &res.to_string(),
                        cors_origin,
                    );
                }
            }
        }
        return;
    }

    // Vector search endpoint (Protected)
    if method == "POST" && (route_path == "/api/vector/search" || route_path == "/vector/search") {
        let body_str = std::str::from_utf8(body_bytes).unwrap_or("");
        let parsed: serde_json::Value = match serde_json::from_str(body_str) {
            Ok(v) => v,
            Err(_) => {
                send_http_response(
                    &mut stream,
                    "400 Bad Request",
                    "application/json",
                    r#"{"error":"Invalid JSON payload. Expected {\"collection\": \"...\", \"vector\": [...], \"k\": 5}"}"#,
                    cors_origin,
                );
                return;
            }
        };

        let collection = parsed
            .get("collection")
            .and_then(|s| s.as_str())
            .unwrap_or("embeddings");
        let k = parsed.get("k").and_then(|v| v.as_u64()).unwrap_or(5) as usize;
        let query_vec_result: std::result::Result<Vec<f32>, String> =
            match parsed.get("vector").and_then(|v| v.as_array()) {
                Some(values) => values
                    .iter()
                    .map(|value| {
                        value
                            .as_f64()
                            .map(|number| number as f32)
                            .filter(|number| number.is_finite())
                            .ok_or_else(|| "Vector entries must be finite numbers".to_owned())
                    })
                    .collect::<std::result::Result<_, _>>(),
                None => Err("vector must be a JSON array".to_owned()),
            };
        let query_vec: Vec<f32> = match query_vec_result {
            Ok(vector) => vector,
            Err(error) => {
                send_http_response(
                    &mut stream,
                    "400 Bad Request",
                    "application/json",
                    &serde_json::json!({"error": error}).to_string(),
                    cors_origin,
                );
                return;
            }
        };
        if query_vec.is_empty() || !is_sql_identifier(collection) || k == 0 {
            send_http_response(
                &mut stream,
                "400 Bad Request",
                "application/json",
                r#"{"error":"collection must be an identifier, vector must be nonempty, and k must be positive"}"#,
                cors_origin,
            );
            return;
        }
        if let HttpAuthScope::SqlRole {
            ref allowed_tables, ..
        } = auth_scope
            && let Err(reason) = check_allowed_table(collection, &[], allowed_tables.as_deref())
        {
            let err_json = serde_json::json!({
                "error": "Forbidden",
                "message": reason
            });
            send_http_response(
                &mut stream,
                "403 Forbidden",
                "application/json",
                &err_json.to_string(),
                cors_origin,
            );
            return;
        }

        let search_res = {
            let conn = db.lock();
            vector_search_results(&conn, collection, &query_vec, k)
        };
        let results = match search_res {
            Ok(results) => results,
            Err((status, error)) => {
                send_http_response(
                    &mut stream,
                    status,
                    "application/json",
                    &serde_json::json!({"error": error}).to_string(),
                    cors_origin,
                );
                return;
            }
        };

        let res = serde_json::to_string(&results).unwrap_or_else(|_| "[]".to_string());
        send_http_response(&mut stream, "200 OK", "application/json", &res, cors_origin);
        return;
    }

    // GraphRAG traversal endpoint (Protected — requires Unrestricted master key)
    if method == "POST" && (route_path == "/api/graph/rag" || route_path == "/graph/rag") {
        if !matches!(auth_scope, HttpAuthScope::Unrestricted) {
            let err_json = serde_json::json!({
                "error": "Forbidden",
                "message": "GraphRAG endpoint requires an unrestricted master API key."
            });
            send_http_response(
                &mut stream,
                "403 Forbidden",
                "application/json",
                &err_json.to_string(),
                cors_origin,
            );
            return;
        }
        let body_str = std::str::from_utf8(body_bytes).unwrap_or("");
        let parsed: serde_json::Value = match serde_json::from_str(body_str) {
            Ok(v) => v,
            Err(_) => {
                send_http_response(
                    &mut stream,
                    "400 Bad Request",
                    "application/json",
                    r#"{"error":"Invalid JSON payload. Expected {\"query\": \"...\"}"}"#,
                    cors_origin,
                );
                return;
            }
        };

        let query = parsed.get("query").and_then(|s| s.as_str()).unwrap_or("");
        let seeds = parsed.get("seeds").and_then(|v| v.as_u64()).unwrap_or(3) as usize;
        let hops = parsed.get("hops").and_then(|v| v.as_u64()).unwrap_or(2) as usize;
        let query_vec: Option<Vec<f32>> = parsed
            .get("query_vector")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|val| val.as_f64().map(|f| f as f32))
                    .collect()
            });

        let config = tapirus::GraphRagConfig {
            top_seeds: seeds,
            max_hops: hops,
            limit: seeds.max(5),
            ..Default::default()
        };

        let rag_ctx = {
            let conn = db.lock();
            conn.graph_rag_query(query, query_vec.as_deref(), &config)
        };
        match rag_ctx {
            Ok(ctx) => {
                let nodes: Vec<serde_json::Value> = ctx
                    .results
                    .iter()
                    .map(|r| {
                        let mut m = serde_json::Map::new();
                        m.insert(
                            "id".to_string(),
                            serde_json::Value::Number(r.entity_id.into()),
                        );
                        m.insert(
                            "label".to_string(),
                            serde_json::Value::String(r.label.clone()),
                        );
                        m.insert(
                            "properties".to_string(),
                            serde_json::Value::String(r.properties.clone()),
                        );
                        m.insert("rrf_score".to_string(), serde_json::json!(r.rrf_score));
                        serde_json::Value::Object(m)
                    })
                    .collect();

                let mut all_edges = Vec::new();
                for r in &ctx.results {
                    for e in &r.related_edges {
                        all_edges.push(serde_json::json!({
                            "from_id": e.from_id,
                            "to_id": e.to_id,
                            "label": e.label,
                            "weight": e.weight,
                            "properties": e.properties
                        }));
                    }
                }

                let res = serde_json::json!({
                    "query": ctx.query,
                    "nodes": nodes,
                    "edges": all_edges,
                    "context": ctx.prompt_context,
                    "results": ctx.results
                });
                send_http_response(
                    &mut stream,
                    "200 OK",
                    "application/json",
                    &res.to_string(),
                    cors_origin,
                );
            }
            Err(e) => {
                let res = serde_json::json!({ "error": e.to_string() });
                send_http_response(
                    &mut stream,
                    "500 Internal Server Error",
                    "application/json",
                    &res.to_string(),
                    cors_origin,
                );
            }
        }
        return;
    }

    send_http_response(
        &mut stream,
        "404 Not Found",
        "text/plain",
        "Not Found",
        cors_origin,
    );
}

pub(super) fn format_http_response(
    status: &str,
    content_type: &str,
    body: &str,
    extra_header: Option<&str>,
    cors_origin: Option<&str>,
) -> String {
    let mut headers = format!(
        "HTTP/1.1 {status}\r\n\
         Content-Type: {content_type}\r\n\
         Content-Length: {}\r\n\
         X-Content-Type-Options: nosniff\r\n\
         X-Frame-Options: DENY\r\n\
         Referrer-Policy: no-referrer\r\n",
        body.len()
    );
    if content_type.starts_with("application/json") {
        headers.push_str("Cache-Control: no-store\r\n");
    }
    if let Some(extra) = extra_header {
        headers.push_str(extra);
        headers.push_str("\r\n");
    }
    if let Some(origin) = cors_origin {
        headers.push_str(&format!(
            "Access-Control-Allow-Origin: {origin}\r\n\
             Access-Control-Allow-Methods: GET, POST, OPTIONS\r\n\
             Access-Control-Allow-Headers: Content-Type, Authorization, X-API-Key\r\n"
        ));
        if origin != "*" {
            headers.push_str("Vary: Origin\r\n");
        }
    }
    headers.push_str("Connection: close\r\n\r\n");
    headers.push_str(body);
    headers
}

pub(super) fn send_http_unauthorized_response<W: Write>(
    stream: &mut W,
    body: &str,
    cors_origin: Option<&str>,
) {
    let response = format_http_response(
        "401 Unauthorized",
        "application/json",
        body,
        Some("WWW-Authenticate: Bearer realm=\"TapirusDB\""),
        cors_origin,
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

pub(super) fn send_http_response<W: Write>(
    stream: &mut W,
    status: &str,
    content_type: &str,
    body: &str,
    cors_origin: Option<&str>,
) {
    let response = format_http_response(status, content_type, body, None, cors_origin);
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

pub(super) fn vector_search_results(
    conn: &Connection,
    collection: &str,
    query_vec: &[f32],
    limit: usize,
) -> std::result::Result<Vec<serde_json::Value>, (&'static str, String)> {
    if !is_sql_identifier(collection) || query_vec.is_empty() || limit == 0 {
        return Err((
            "400 Bad Request",
            "Invalid collection, vector, or limit".into(),
        ));
    }
    let table = conn
        .tables()
        .into_iter()
        .find(|table| table.name.eq_ignore_ascii_case(collection))
        .ok_or_else(|| {
            (
                "404 Not Found",
                format!("Table '{collection}' was not found"),
            )
        })?;
    let (vector_index, dimensions) = table.vector_column().ok_or_else(|| {
        (
            "400 Bad Request",
            format!("Table '{collection}' has no vector column"),
        )
    })?;
    if dimensions != query_vec.len() {
        return Err((
            "400 Bad Request",
            format!("Expected a {dimensions}-dimensional query vector"),
        ));
    }
    let vector_col = &table.columns[vector_index].name;
    if !is_sql_identifier(vector_col) {
        return Err((
            "400 Bad Request",
            "Unsupported vector column identifier".into(),
        ));
    }
    let sql =
        format!("SELECT * FROM {collection} VECTOR NEAR {vector_col} = {query_vec:?} TOP {limit};");
    let rows = conn
        .query(&sql)
        .map_err(|error| ("400 Bad Request", error.to_string()))?;
    Ok(rows
        .iter()
        .filter_map(|row| {
            let vector = row
                .columns()
                .iter()
                .zip(row.values())
                .find_map(|(name, value)| {
                    name.eq_ignore_ascii_case(vector_col)
                        .then_some(value)
                        .and_then(|value| match value {
                            Value::Vector(vector) => Some(vector.clone()),
                            _ => None,
                        })
                })?;
            let id = row
                .get::<i64>("id")
                .or_else(|_| row.get::<i64>("doc_id"))
                .ok();
            let mut metadata = serde_json::Map::new();
            for (column, value) in row.columns().iter().zip(row.values()) {
                if !column.eq_ignore_ascii_case(vector_col)
                    && !column.eq_ignore_ascii_case("id")
                    && !column.eq_ignore_ascii_case("doc_id")
                {
                    metadata.insert(column.clone(), value_to_json(value));
                }
            }
            Some(serde_json::json!({
                "id": id,
                "score": tapirus::vector::cosine_similarity(query_vec, &vector),
                "collection": collection,
                "vector": vector,
                "metadata": metadata
            }))
        })
        .collect())
}

pub(super) fn value_to_json(val: &Value) -> serde_json::Value {
    match val {
        Value::Null => serde_json::Value::Null,
        Value::Integer(i) => serde_json::json!(i),
        Value::Real(r) => serde_json::json!(r),
        Value::Text(s) => serde_json::json!(s),
        Value::Blob(b) => serde_json::json!(b),
        Value::Vector(v) => serde_json::json!(v),
    }
}

// ---------------------------------------------------------------------------
// Native AI Cognitive Chatbot Engine & Web UI
// ---------------------------------------------------------------------------

pub(super) fn sql_escape_string(s: &str) -> String {
    s.replace('\'', "''")
}
