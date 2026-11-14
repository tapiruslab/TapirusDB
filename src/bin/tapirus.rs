//! # TapirusDB Interactive Terminal Shell & HTTP Server (`tapirus`)
//!
//! An interactive REPL and lightweight HTTP server for TapirusDB providing unified SQL,
//! Native AI Vector Search, MongoDB-style Documents, and Knowledge Graph inspection.

#![forbid(unsafe_code)]

use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use std::env;
use std::io::{self, BufRead, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;
use subtle::ConstantTimeEq;
use tapirus::sql::catalog::DataType;
use tapirus::sql::lexer::{Token, tokenize};
use tapirus::sql::parser::WhereExpr;
use tapirus::{Connection, EmbeddingEngine, Row, Statement, Value, bind_parameters, parse_tokens};
use web_time::Instant;
use zeroize::Zeroize;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const HTTP_MAX_CLIENTS: usize = 64;
const HTTP_MAX_HEADER_BYTES: usize = 16 * 1024;
const HTTP_MAX_BODY_BYTES: usize = 4 * 1024 * 1024;
const HTTP_IO_TIMEOUT: Duration = Duration::from_secs(10);

struct ActiveHttpClient(Arc<AtomicUsize>);

impl Drop for ActiveHttpClient {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Release);
    }
}

fn try_acquire_http_client(active: &Arc<AtomicUsize>) -> Option<ActiveHttpClient> {
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

const BANNER: &str = r#"
  ___________           .__                     ________  __________
  \__    ___/____  ____ |__|______ __ __  ______\______ \ \______   \
    |    |  \__  \ \____ \|  \_  __ \  |  \/  ___/ |    |  \ |    |  _/
    |    |   / __ \|  |_> >  ||  | \/  |  /\___ \  |    `   \|    |   \
    |____|  (____  /   __/|__||__|  |____//____  >/_______  /|______  /
                 \/|__|                        \/         \/        \/
"#;

fn main() {
    let args: Vec<String> = env::args().collect();

    // Check if sub-command is "serve"
    if args.len() > 1 && args[1] == "serve" {
        run_serve_command(&args[2..]);
        return;
    }

    // Check if sub-command is "studio"
    if args.len() > 1 && args[1] == "studio" {
        run_studio_command(&args[2..]);
        return;
    }

    // Check if sub-command is "backup"
    if args.len() > 1 && args[1] == "backup" {
        run_backup_command(&args[2..]);
        return;
    }

    // Check if sub-command is "restore"
    if args.len() > 1 && args[1] == "restore" {
        run_restore_command(&args[2..]);
        return;
    }

    // Check if sub-command is "verify"
    if args.len() > 1 && args[1] == "verify" {
        run_verify_command(&args[2..]);
        return;
    }

    // Check if sub-command is "import"
    if args.len() > 1 && args[1] == "import" {
        run_import_command(&args[2..]);
        return;
    }

    // Check if sub-command is "mcp"
    if args.len() > 1 && args[1] == "mcp" {
        run_mcp_command(&args[2..]);
        return;
    }

    // Check if sub-command is "grep" or "tg"
    if args.len() > 1 && (args[1] == "grep" || args[1] == "tg") {
        run_grep_command(&args[2..]);
        return;
    }

    // Check if sub-command is "bitnet" or "tap-deep"
    if args.len() > 1 && (args[1] == "bitnet" || args[1] == "tap-deep") {
        run_bitnet_command(&args[2..]);
        return;
    }

    // Parse global flags
    if args.len() > 1 {
        match args[1].as_str() {
            "-h" | "--help" => {
                print_help();
                return;
            }
            "-v" | "--version" => {
                println!("TapirusDB version {VERSION}");
                return;
            }
            _ => {}
        }
    }

    let target = if args.len() > 1 && !args[1].starts_with('-') {
        &args[1]
    } else {
        ":memory:"
    };

    let is_memory = target == ":memory:";
    let conn = if is_memory {
        match Connection::open_in_memory() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("Error opening in-memory database: {e}");
                std::process::exit(1);
            }
        }
    } else {
        match Connection::open(Path::new(target)) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("Error opening database at '{target}': {e}");
                std::process::exit(1);
            }
        }
    };

    // Non-interactive execution: tapirus [DB] [-c SQL | --sql SQL | --json SQL | "SELECT ..."]
    if args.len() > 2 {
        let (sql_cmd, is_json) = if args[2] == "-c" || args[2] == "--sql" {
            (args.get(3).cloned().unwrap_or_default(), false)
        } else if args[2] == "--json" {
            (args.get(3).cloned().unwrap_or_default(), true)
        } else if !args[2].starts_with('-') {
            (args[2..].join(" "), false)
        } else {
            (String::new(), false)
        };

        if !sql_cmd.is_empty() {
            if is_json {
                match conn.query(&sql_cmd) {
                    Ok(rows) => {
                        let json_rows: Vec<serde_json::Value> = rows
                            .iter()
                            .map(|r| {
                                let mut map = serde_json::Map::new();
                                for (c, v) in r.columns().iter().zip(r.values().iter()) {
                                    map.insert(
                                        c.clone(),
                                        serde_json::to_value(v).unwrap_or(serde_json::Value::Null),
                                    );
                                }
                                serde_json::Value::Object(map)
                            })
                            .collect();
                        println!(
                            "{}",
                            serde_json::to_string(&json_rows).unwrap_or_else(|_| "[]".to_string())
                        );
                    }
                    Err(e) => {
                        eprintln!("Error: {e}");
                        std::process::exit(1);
                    }
                }
            } else {
                let stmts: Vec<&str> = sql_cmd.split(';').collect();
                for stmt in stmts {
                    let s = stmt.trim();
                    if !s.is_empty() && !execute_statement(&conn, s) {
                        std::process::exit(1);
                    }
                }
            }
            return;
        }
    }

    println!("{BANNER}");
    println!("TapirusDB v{VERSION} - Embedded Rust database");
    println!("Connected to: {target} (Page Size: 4,096 B)");
    println!("Enter SQL, Vector, or dot-commands. End SQL statements with ';'.");
    println!("Type '.help' for instructions, '.exit' to quit.\n");

    run_repl(&conn, target);
}

fn print_help() {
    println!("Usage:");
    println!("  tapirus [OPTIONS] [DATABASE_FILE]                    Launch interactive REPL");
    println!(
        "  tapirus backup [OPTIONS] <SOURCE_DB> <DEST_BACKUP>   Create a database snapshot backup"
    );
    println!(
        "  tapirus restore [OPTIONS] <BACKUP_FILE> <TARGET_DB>  Restore a database from a backup"
    );
    println!(
        "  tapirus verify [OPTIONS] <DATABASE_FILE>             Check database page and encryption integrity"
    );
    println!(
        "  tapirus import <FORMAT> <FILE> [OPTIONS]             Streaming importer (CSV, JSONL, Markdown)"
    );
    println!("  tapirus serve [OPTIONS] [DATABASE_FILE]              Launch the HTTP REST server");
    println!(
        "  tapirus studio [OPTIONS] [DATABASE_FILE]             Launch Tapirus Studio companion bridge server"
    );
    println!(
        "  tapirus mcp [OPTIONS] [DATABASE_FILE]                Launch Model Context Protocol (MCP) server"
    );
    println!(
        "  tapirus grep [OPTIONS] <PATTERN> [PATH]              Accelerated hybrid workspace search (tg)"
    );
    println!(
        "  tapirus bitnet [OPTIONS] <INPUT> [CANDIDATES...]     BitNet b1.58 ternary tensor demo"
    );
    println!();
    println!("Options:");
    println!("  -h, --help                               Print this help message");
    println!("  -v, --version                            Print TapirusDB version");
    println!();
    println!("Query Options:");
    println!("  -c, --sql <SQL>                          Execute non-interactive SQL statement(s)");
    println!(
        "  --json <SQL>                             Execute query and format output as JSON array"
    );
    println!();
    println!("Backup Options (for 'tapirus backup'):");
    println!(
        "  --passphrase <KEY>                       Passphrase for encrypted database (ChaCha20-Poly1305)"
    );
    println!(
        "  --vacuum, --compact                      Perform VACUUM and page compaction during backup"
    );
    println!();
    println!("Restore Options (for 'tapirus restore'):");
    println!(
        "  --passphrase <KEY>                       Passphrase for encrypted backup (ChaCha20-Poly1305)"
    );
    println!(
        "  -f, --force                              Overwrite target database if it already exists"
    );
    println!();
    println!("Verify Options (for 'tapirus verify'):");
    println!("  --passphrase <KEY>                       Passphrase for encrypted database audit");
    println!(
        "  --deep, --full                           Deep slotted page scan & full checksum verification"
    );
    println!();
    println!("Import Options (for 'tapirus import'):");
    println!(
        "  --db <PATH>                              Path to target database file (default: production.tapir)"
    );
    println!("  --table <NAME>                           Target table name (CSV)");
    println!("  --collection <NAME>                      Target collection name (JSON / JSONL)");
    println!("  --namespace <NAME>                       Target AI memory namespace (Markdown)");
    println!("  --session-id <ID>                        Session ID for agent memory (Markdown)");
    println!(
        "  --tags <TAG1,TAG2,...>                   Indexing tags for memory recall (Markdown)"
    );
    println!(
        "  --batch <N>                              Batch transaction commit size (default: 500)"
    );
    println!(
        "  --passphrase <KEY>                       Passphrase for encrypted database (ChaCha20-Poly1305)"
    );
    println!();
    println!("Server Options (for 'tapirus serve'):");
    println!("  -p, --port <PORT>                        Port to listen on (default: 3005)");
    println!("  -b, --host <HOST>                        Host to bind to (default: 127.0.0.1)");
    println!("  -k, --api-key <KEY>                      Enforce API key token authentication");
    println!(
        "  --cors-origin <ORIGIN>                   Allowed CORS origin (default: none/same-origin)"
    );
    println!(
        "  --passphrase <KEY>                       Encryption passphrase (ChaCha20-Poly1305)"
    );
    println!();
    println!("Environment Variables:");
    println!(
        "  TAPIRUS_API_KEY                          Fallback API key for 'tapirus serve' if --api-key omitted"
    );
    println!(
        "  TAPIRUS_CORS_ORIGIN                      Comma-separated CORS origins (default: restricted)"
    );
    println!();
    println!("Transport Security:");
    println!(
        "  'tapirus serve' serves plaintext HTTP. Bind to 127.0.0.1 behind a TLS reverse proxy"
    );
    println!("  (Nginx, Caddy, Envoy, or Cloudflare Tunnel) before exposing over any network.");
    println!();
    println!("Arguments:");
    println!("  [DATABASE_FILE]                          Path to .tapir database file");
}

fn print_serve_help() {
    println!("Usage: tapirus serve [OPTIONS] [DATABASE_FILE]");
    println!();
    println!("Options:");
    println!("  -p, --port <PORT>                Port to listen on (default: 3005)");
    println!("  -b, --host <HOST>                Host to bind to (default: 127.0.0.1)");
    println!(
        "  -k, --api-key <KEY>              Enforce master API key authentication (Bearer / X-API-Key)"
    );
    println!(
        "  --api-key-namespace <KEY>=<NS>   Bind an API key to a single isolated memory namespace (repeatable)"
    );
    println!(
        "  --api-key-role <KEY>=<RULE>      Bind a scoped SQL RBAC key: readonly|readwrite[:t1+t2] (repeatable)"
    );
    println!(
        "  --tls-cert <CERT_PEM>            Enable native Rustls HTTPS with PEM certificate chain"
    );
    println!("  --tls-key <KEY_PEM>              Enable native Rustls HTTPS with PEM private key");
    println!(
        "  --cors-origin <ORIGIN>           Allow CORS origin (comma-separated or repeatable; '*' opts in)"
    );
    println!(
        "  --passphrase <KEY>               Passphrase for encrypted database (ChaCha20-Poly1305)"
    );
    println!("  -h, --help                       Print this help message");
    println!();
    println!("Environment Variables:");
    println!(
        "  TAPIRUS_API_KEY                  Fallback master API key if --api-key is not specified"
    );
    println!(
        "  TAPIRUS_API_KEY_NAMESPACES       Comma-separated KEY=NAMESPACE bindings for tenant isolation"
    );
    println!(
        "  TAPIRUS_API_KEY_ROLES            Comma-separated KEY=readonly|readwrite[:t1+t2] SQL RBAC bindings"
    );
    println!(
        "  TAPIRUS_TLS_CERT                 Fallback PEM certificate chain path for native TLS"
    );
    println!("  TAPIRUS_TLS_KEY                  Fallback PEM private key path for native TLS");
    println!(
        "  TAPIRUS_CORS_ORIGIN              Comma-separated allowed CORS origins (default: disabled)"
    );
    println!();
    println!("Transport Security:");
    println!("  Pass --tls-cert and --tls-key to serve native HTTPS via Rustls, or bind to");
    println!("  127.0.0.1 behind a TLS-terminating reverse proxy (Nginx, Caddy, or tunnel).");
    println!();
    println!("Arguments:");
    println!(
        "  [DATABASE_FILE]                  Path to database file (defaults to: production.tapir)"
    );
}

fn run_studio_command(args: &[String]) {
    println!("===============================================================");
    println!("  Tapirus Studio Companion Server (v{})", VERSION);
    println!("===============================================================");
    println!("  -> Web Studio UI:  https://tapirusdb.com/studio/");
    println!("  -> Local API:      http://127.0.0.1:3005");
    println!("  -> Press Ctrl+C to stop server cleanly.");
    println!("===============================================================\n");
    run_serve_command(args);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SqlRoleAccess {
    ReadOnly,
    ReadWrite,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ApiKeyRoleBinding {
    key: String,
    access: SqlRoleAccess,
    allowed_tables: Option<Vec<String>>,
}

fn parse_api_key_role_bindings(
    raw_entries: &[String],
) -> std::result::Result<Vec<ApiKeyRoleBinding>, String> {
    let mut bindings = Vec::new();
    for entry in raw_entries {
        for item in entry.split(',') {
            let trimmed = item.trim();
            if trimmed.is_empty() {
                continue;
            }
            let Some((key, rule)) = trimmed.split_once('=') else {
                return Err(format!(
                    "Invalid API key role binding '{trimmed}'; expected KEY=readonly|readwrite[:table1+table2]"
                ));
            };
            let key = key.trim();
            let rule = rule.trim();
            if key.is_empty() {
                return Err("API key in KEY=ROLE binding must not be empty".to_string());
            }
            let (role_part, tables_part) = match rule.split_once(':') {
                Some((r, t)) => (r.trim(), Some(t.trim())),
                None => (rule, None),
            };
            let access = match role_part.to_ascii_lowercase().as_str() {
                "readonly" | "read" | "ro" => SqlRoleAccess::ReadOnly,
                "readwrite" | "rw" => SqlRoleAccess::ReadWrite,
                other => {
                    return Err(format!(
                        "Unknown SQL role '{other}'; expected 'readonly' or 'readwrite'"
                    ));
                }
            };
            let allowed_tables = if let Some(tables_raw) = tables_part {
                if tables_raw == "*" {
                    None
                } else {
                    let mut tables = Vec::new();
                    for tbl in tables_raw
                        .split('+')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                    {
                        if !is_sql_identifier(tbl) {
                            return Err(format!(
                                "Invalid SQL table identifier '{tbl}' in RBAC rule"
                            ));
                        }
                        let lower = tbl.to_ascii_lowercase();
                        if !tables.contains(&lower) {
                            tables.push(lower);
                        }
                    }
                    if tables.is_empty() {
                        return Err(format!(
                            "Role binding '{trimmed}' specified ':' with an empty table allowlist"
                        ));
                    }
                    Some(tables)
                }
            } else {
                None
            };
            if bindings.iter().any(|b: &ApiKeyRoleBinding| b.key == key) {
                return Err(format!("Duplicate API key in role bindings: '{key}'"));
            }
            bindings.push(ApiKeyRoleBinding {
                key: key.to_string(),
                access,
                allowed_tables,
            });
        }
    }
    Ok(bindings)
}

fn parse_api_key_namespace_bindings(
    raw_entries: &[String],
) -> std::result::Result<Vec<(String, String)>, String> {
    let mut bindings = Vec::new();
    for entry in raw_entries {
        for item in entry.split(',') {
            let trimmed = item.trim();
            if trimmed.is_empty() {
                continue;
            }
            let Some((key, namespace)) = trimmed.split_once('=') else {
                return Err(format!(
                    "Invalid API key namespace binding '{trimmed}'; expected KEY=NAMESPACE"
                ));
            };
            let key = key.trim();
            let namespace = namespace.trim();
            if key.is_empty() {
                return Err("API key in KEY=NAMESPACE binding must not be empty".to_string());
            }
            tapirus::MemoryAccessScope::new(namespace, None)
                .map_err(|err| format!("Invalid namespace '{namespace}': {err}"))?;
            if bindings.iter().any(|(existing_key, _)| existing_key == key) {
                return Err(format!("Duplicate API key in namespace bindings: '{key}'"));
            }
            bindings.push((key.to_string(), namespace.to_string()));
        }
    }
    Ok(bindings)
}

fn run_serve_command(args: &[String]) {
    let mut port: u16 = 3005;
    let mut host = "127.0.0.1".to_string();
    let mut api_key: Option<String> = env::var("TAPIRUS_API_KEY").ok();
    let mut api_key_ns_raw: Vec<String> = env::var("TAPIRUS_API_KEY_NAMESPACES")
        .ok()
        .into_iter()
        .collect();
    let mut api_key_roles_raw: Vec<String> =
        env::var("TAPIRUS_API_KEY_ROLES").ok().into_iter().collect();
    let mut tls_cert: Option<String> = env::var("TAPIRUS_TLS_CERT")
        .ok()
        .filter(|s| !s.trim().is_empty());
    let mut tls_key: Option<String> = env::var("TAPIRUS_TLS_KEY")
        .ok()
        .filter(|s| !s.trim().is_empty());
    let mut cors_origins_raw: Vec<String> =
        env::var("TAPIRUS_CORS_ORIGIN").ok().into_iter().collect();
    let mut passphrase: Option<String> = None;
    let mut db_path = "production.tapir".to_string();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-p" | "--port" => {
                if i + 1 < args.len() {
                    if let Ok(p) = args[i + 1].parse() {
                        port = p;
                    }
                    i += 1;
                }
            }
            "-b" | "--host" | "--bind" => {
                if i + 1 < args.len() {
                    let val = args[i + 1].clone();
                    if let Some((h, p_str)) = val.split_once(':') {
                        host = h.to_string();
                        if let Ok(p) = p_str.parse::<u16>() {
                            port = p;
                        }
                    } else {
                        host = val;
                    }
                    i += 1;
                }
            }
            "-d" | "--database" | "--db" => {
                if i + 1 < args.len() {
                    db_path = args[i + 1].clone();
                    i += 1;
                }
            }
            "-k" | "--api-key" => {
                if i + 1 < args.len() {
                    api_key = Some(args[i + 1].clone());
                    i += 1;
                }
            }
            "--api-key-namespace" => {
                if i + 1 < args.len() {
                    api_key_ns_raw.push(args[i + 1].clone());
                    i += 1;
                }
            }
            "--api-key-role" => {
                if i + 1 < args.len() {
                    api_key_roles_raw.push(args[i + 1].clone());
                    i += 1;
                }
            }
            "--tls-cert" => {
                if i + 1 < args.len() {
                    tls_cert = Some(args[i + 1].clone());
                    i += 1;
                }
            }
            "--tls-key" => {
                if i + 1 < args.len() {
                    tls_key = Some(args[i + 1].clone());
                    i += 1;
                }
            }
            "--cors-origin" => {
                if i + 1 < args.len() {
                    cors_origins_raw.push(args[i + 1].clone());
                    i += 1;
                }
            }
            "--passphrase" => {
                if i + 1 < args.len() {
                    passphrase = Some(args[i + 1].clone());
                    i += 1;
                }
            }
            "-h" | "--help" => {
                print_serve_help();
                return;
            }
            arg if !arg.starts_with('-') => {
                db_path = arg.to_string();
            }
            _ => {}
        }
        i += 1;
    }

    if api_key.as_deref().is_some_and(str::is_empty) {
        eprintln!("TAPIRUS_API_KEY/--api-key must not be empty");
        std::process::exit(1);
    }
    let api_key_scopes = match parse_api_key_namespace_bindings(&api_key_ns_raw) {
        Ok(bindings) => bindings,
        Err(err) => {
            eprintln!("Invalid --api-key-namespace / TAPIRUS_API_KEY_NAMESPACES: {err}");
            std::process::exit(1);
        }
    };
    let api_key_roles = match parse_api_key_role_bindings(&api_key_roles_raw) {
        Ok(bindings) => bindings,
        Err(err) => {
            eprintln!("Invalid --api-key-role / TAPIRUS_API_KEY_ROLES: {err}");
            std::process::exit(1);
        }
    };
    if let Some(ref master) = api_key {
        if api_key_scopes.iter().any(|(k, _)| k == master) {
            eprintln!("Master --api-key cannot also be bound in --api-key-namespace");
            std::process::exit(1);
        }
        if api_key_roles.iter().any(|r| &r.key == master) {
            eprintln!("Master --api-key cannot also be bound in --api-key-role");
            std::process::exit(1);
        }
    }
    for role in &api_key_roles {
        if api_key_scopes.iter().any(|(k, _)| k == &role.key) {
            eprintln!(
                "API key '{}' cannot appear in both --api-key-namespace and --api-key-role",
                role.key
            );
            std::process::exit(1);
        }
    }
    if tls_cert.is_some() != tls_key.is_some() {
        eprintln!(
            "Both --tls-cert and --tls-key (or TAPIRUS_TLS_CERT and TAPIRUS_TLS_KEY) must be provided together."
        );
        std::process::exit(1);
    }
    if !matches!(host.as_str(), "127.0.0.1" | "localhost" | "::1")
        && api_key.is_none()
        && api_key_scopes.is_empty()
        && api_key_roles.is_empty()
    {
        eprintln!(
            "Non-loopback HTTP bind requires TAPIRUS_API_KEY/--api-key, --api-key-namespace, or --api-key-role"
        );
        std::process::exit(1);
    }

    let cors_origins = match parse_cors_origins(&cors_origins_raw.join(",")) {
        Ok(origins) => origins,
        Err(err) => {
            eprintln!("Invalid --cors-origin / TAPIRUS_CORS_ORIGIN: {err}");
            std::process::exit(1);
        }
    };

    let is_memory = db_path == ":memory:";
    let is_encrypted = passphrase.is_some();
    let conn = if let Some(ref pass) = passphrase {
        match if is_memory {
            Connection::open_encrypted_in_memory(pass)
        } else {
            Connection::open_encrypted(Path::new(&db_path), pass)
        } {
            Ok(c) => c,
            Err(e) => {
                eprintln!("Error opening encrypted database at '{db_path}': {e}");
                std::process::exit(1);
            }
        }
    } else if is_memory {
        match Connection::open_in_memory() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("Error opening in-memory database: {e}");
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
    if let Some(ref mut pass) = passphrase {
        pass.zeroize();
    }

    run_http_server(
        conn,
        HttpServerConfig {
            host: &host,
            port,
            db_path: &db_path,
            encrypted: is_encrypted,
            api_key,
            api_key_scopes,
            api_key_roles,
            cors_origins,
            tls_cert,
            tls_key,
        },
    );
}

fn print_backup_help() {
    println!("Usage: tapirus backup [OPTIONS] <SOURCE_DATABASE> <DEST_BACKUP>");
    println!();
    println!("Perform an atomic, hot point-in-time snapshot backup of a TapirusDB database.");
    println!();
    println!("Options:");
    println!("  --passphrase <KEY>   Passphrase for encrypted database (ChaCha20-Poly1305)");
    println!("  --vacuum, --compact  Vacuum and compact database while creating backup");
    println!("  -h, --help           Print this help message");
    println!();
    println!("Arguments:");
    println!("  <SOURCE_DATABASE>    Path to existing source database (.tapir file)");
    println!("  <DEST_BACKUP>        Path to target backup file destination");
}

fn run_backup_command(args: &[String]) {
    let mut passphrase: Option<String> = None;
    let mut compact = false;
    let mut positional = Vec::new();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--passphrase" => {
                if i + 1 < args.len() {
                    passphrase = Some(args[i + 1].clone());
                    i += 1;
                }
            }
            "--vacuum" | "--compact" => {
                compact = true;
            }
            "-h" | "--help" => {
                print_backup_help();
                return;
            }
            arg if !arg.starts_with('-') => {
                positional.push(arg.to_string());
            }
            _ => {}
        }
        i += 1;
    }

    if positional.len() < 2 {
        eprintln!("Error: Both <SOURCE_DATABASE> and <DEST_BACKUP> arguments are required.");
        println!();
        print_backup_help();
        std::process::exit(1);
    }

    let src_path = &positional[0];
    let dest_path = &positional[1];

    if !Path::new(src_path).exists() {
        eprintln!("Error: Source database file '{src_path}' does not exist.");
        std::process::exit(1);
    }

    println!("Starting TapirusDB hot backup...");
    println!("  Source:      {src_path}");
    println!("  Destination: {dest_path}");
    if compact {
        println!("  Mode:        VACUUM & Compact Snapshot");
    } else {
        println!("  Mode:        Point-in-Time Online Hot Snapshot");
    }

    let start = Instant::now();

    let conn = if let Some(ref pass) = passphrase {
        match Connection::open_encrypted(Path::new(src_path), pass) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("Error opening encrypted database at '{src_path}': {e}");
                std::process::exit(1);
            }
        }
    } else {
        match Connection::open(Path::new(src_path)) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("Error opening database at '{src_path}': {e}");
                std::process::exit(1);
            }
        }
    };

    let pages_result = if compact {
        conn.vacuum_into(Path::new(dest_path))
    } else {
        conn.backup(Path::new(dest_path))
    };

    match pages_result {
        Ok(pages) => {
            let elapsed = start.elapsed();
            let dest_size = std::fs::metadata(dest_path).map(|m| m.len()).unwrap_or(0);

            // Calculate SHA-256 of created backup for cryptographic verification
            let sha256_hex = match std::fs::read(dest_path) {
                Ok(bytes) => {
                    let mut hasher = Sha256::new();
                    hasher.update(&bytes);
                    use std::fmt::Write as _;
                    let mut digest_hex = String::with_capacity(64);
                    for byte in hasher.finalize() {
                        write!(&mut digest_hex, "{byte:02x}")
                            .expect("writing to String cannot fail");
                    }
                    digest_hex
                }
                Err(_) => "unavailable".to_string(),
            };

            println!();
            println!("Backup completed successfully!");
            println!("  Pages Written:  {pages}");
            println!(
                "  Backup Size:    {} bytes ({:.2} KB)",
                dest_size,
                dest_size as f64 / 1024.0
            );
            println!("  SHA-256 Hash:   {sha256_hex}");
            println!("  Duration:       {:.2?}", elapsed);
            println!("  Status:         VERIFIED & COMMITTED");
        }
        Err(e) => {
            eprintln!("Backup failed: {e}");
            std::process::exit(1);
        }
    }
}

fn print_restore_help() {
    println!("Usage: tapirus restore [OPTIONS] <BACKUP_FILE> <RESTORE_TARGET>");
    println!();
    println!("Verify and safely restore a TapirusDB backup file to a target database destination.");
    println!();
    println!("Options:");
    println!("  --passphrase <KEY>   Passphrase for encrypted backup (ChaCha20-Poly1305)");
    println!("  -f, --force          Overwrite restore target if it already exists");
    println!("  -h, --help           Print this help message");
    println!();
    println!("Arguments:");
    println!("  <BACKUP_FILE>        Path to source backup file (.tapir)");
    println!("  <RESTORE_TARGET>     Path to target database destination");
}

fn run_restore_command(args: &[String]) {
    let mut passphrase: Option<String> = None;
    let mut force = false;
    let mut positional = Vec::new();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--passphrase" => {
                if i + 1 < args.len() {
                    passphrase = Some(args[i + 1].clone());
                    i += 1;
                }
            }
            "-f" | "--force" => {
                force = true;
            }
            "-h" | "--help" => {
                print_restore_help();
                return;
            }
            arg if !arg.starts_with('-') => {
                positional.push(arg.to_string());
            }
            _ => {}
        }
        i += 1;
    }

    if positional.len() < 2 {
        eprintln!("Error: Both <BACKUP_FILE> and <RESTORE_TARGET> arguments are required.");
        println!();
        print_restore_help();
        std::process::exit(1);
    }

    let backup_path = &positional[0];
    let target_path = &positional[1];

    if !Path::new(backup_path).exists() {
        eprintln!("Error: Backup file '{backup_path}' does not exist.");
        std::process::exit(1);
    }

    if Path::new(target_path).exists() && !force {
        eprintln!(
            "Error: Restore target '{target_path}' already exists. Use -f or --force to overwrite."
        );
        std::process::exit(1);
    }

    println!("Starting TapirusDB database restoration...");
    println!("  Backup Source:  {backup_path}");
    println!("  Restore Target: {target_path}");

    let start = Instant::now();

    // 1. Verify backup file header integrity before proceeding
    let mut backup_file = match std::fs::File::open(backup_path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("Error opening backup file '{backup_path}': {e}");
            std::process::exit(1);
        }
    };

    let mut header_buf = [0u8; 100];
    if let Err(e) = backup_file.read_exact(&mut header_buf) {
        eprintln!("Error reading header from backup file: {e}");
        std::process::exit(1);
    }

    let header = match tapirus::pager::DatabaseHeader::from_bytes(&header_buf) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("Corrupted backup file: Header validation failed: {e}");
            std::process::exit(1);
        }
    };

    // Check encryption
    if header.encryption_flags == 1 {
        if let Some(ref pass) = passphrase {
            let cipher = tapirus::crypto::DatabaseCipher::from_passphrase(pass, header.salt);
            if !cipher.verify_kcv(&header.kcv) {
                eprintln!("Error: Invalid encryption passphrase for encrypted backup.");
                std::process::exit(1);
            }
        } else {
            eprintln!(
                "Error: Backup file is encrypted (ChaCha20-Poly1305). Please provide --passphrase <KEY>."
            );
            std::process::exit(1);
        }
    }

    // Validate the full source and preserve target locks, WAL and prior backup.
    let config = tapirus::Config {
        passphrase: passphrase.clone(),
        ..Default::default()
    };
    if let Err(e) = Connection::restore_backup(backup_path, target_path, config, force) {
        eprintln!("Error validating/installing backup: {e}");
        std::process::exit(1);
    }

    // 3. Verify target database opens and initializes correctly
    let conn_test = if let Some(ref pass) = passphrase {
        Connection::open_encrypted(Path::new(target_path), pass)
    } else {
        Connection::open(Path::new(target_path))
    };

    match conn_test {
        Ok(c) => {
            let elapsed = start.elapsed();
            let table_count = c.tables().len();
            let (nodes, edges) = c.graph_stats();
            let target_size = std::fs::metadata(target_path).map(|m| m.len()).unwrap_or(0);

            println!();
            println!("Database restored successfully!");
            println!("  Total Pages:    {}", header.total_pages);
            println!(
                "  Database Size:  {} bytes ({:.2} KB)",
                target_size,
                target_size as f64 / 1024.0
            );
            println!("  Format Version: v{}", header.version);
            println!(
                "  Encryption:     {}",
                if header.encryption_flags == 1 {
                    "ChaCha20-Poly1305 (Verified ✓)"
                } else {
                    "Plaintext"
                }
            );
            println!(
                "  Catalog Stats:  {} tables, {} graph nodes, {} graph edges",
                table_count, nodes, edges
            );
            println!("  Duration:       {:.2?}", elapsed);
            println!("  Status:         HEALTHY & READY");
        }
        Err(e) => {
            eprintln!(
                "Restoration validation failed: Could not initialize database at '{target_path}': {e}"
            );
            std::process::exit(1);
        }
    }
}

fn print_verify_help() {
    println!("Usage: tapirus verify [OPTIONS] <DATABASE_FILE>");
    println!();
    println!("Audit physical page integrity, cryptographic KCV signatures, CRC32 checksums,");
    println!("and schema catalogs for a TapirusDB database file.");
    println!();
    println!("Options:");
    println!("  --passphrase <KEY>   Passphrase for encrypted database (ChaCha20-Poly1305)");
    println!("  --deep, --full       Deep slotted page scan & full checksum verification");
    println!("  -h, --help           Print this help message");
    println!();
    println!("Arguments:");
    println!("  <DATABASE_FILE>      Path to .tapir database file to audit");
}

fn run_verify_command(args: &[String]) {
    let mut passphrase: Option<String> = None;
    let mut deep = false;
    let mut db_path = String::new();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--passphrase" => {
                if i + 1 < args.len() {
                    passphrase = Some(args[i + 1].clone());
                    i += 1;
                }
            }
            "--deep" | "--full" => {
                deep = true;
            }
            "-h" | "--help" => {
                print_verify_help();
                return;
            }
            arg if !arg.starts_with('-') => {
                db_path = arg.to_string();
            }
            _ => {}
        }
        i += 1;
    }

    if db_path.is_empty() {
        eprintln!("Error: <DATABASE_FILE> argument is required.");
        println!();
        print_verify_help();
        std::process::exit(1);
    }

    let p = Path::new(&db_path);
    if !p.exists() {
        eprintln!("Error: Database file '{db_path}' does not exist.");
        std::process::exit(1);
    }

    let file_metadata = match std::fs::metadata(p) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("Error reading file metadata: {e}");
            std::process::exit(1);
        }
    };

    let file_size = file_metadata.len();
    if file_size < 100 {
        eprintln!(
            "Integrity Error: File size ({} bytes) is too small to contain a valid TapirusDB header.",
            file_size
        );
        std::process::exit(1);
    }

    let mut f = match std::fs::File::open(p) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("Error opening file: {e}");
            std::process::exit(1);
        }
    };

    let mut header_bytes = [0u8; 100];
    if let Err(e) = f.read_exact(&mut header_bytes) {
        eprintln!("Error reading header: {e}");
        std::process::exit(1);
    }

    let header = match tapirus::pager::DatabaseHeader::from_bytes(&header_bytes) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("Integrity Audit FAILED: Corrupted database header: {e}");
            std::process::exit(1);
        }
    };

    let page_size = header.page_size as u64;
    let expected_file_size = header.total_pages as u64 * page_size;
    let is_size_matching = file_size >= expected_file_size;

    let is_encrypted = header.encryption_flags == 1;
    let kcv_status = if is_encrypted {
        if let Some(ref pass) = passphrase {
            let cipher = tapirus::crypto::DatabaseCipher::from_passphrase(pass, header.salt);
            if cipher.verify_kcv(&header.kcv) {
                "Valid (Constant-time KCV Match ✓)"
            } else {
                "INVALID (Passphrase does not match KCV ✗)"
            }
        } else {
            "Passphrase required for KCV validation (--passphrase <KEY>)"
        }
    } else {
        "Plaintext (No encryption)"
    };

    println!("===============================================================");
    println!("       TapirusDB Physical & Cryptographic Integrity Audit      ");
    println!("===============================================================");
    println!("Database File:     {db_path}");
    println!(
        "File Size:         {file_size} bytes ({:.2} KB)",
        file_size as f64 / 1024.0
    );
    println!("Page Size:         {} bytes", header.page_size);
    println!(
        "Total Pages:       {} pages {}",
        header.total_pages,
        if is_size_matching {
            "(Size verified ✓)"
        } else {
            "(Size mismatch warning ⚠️)"
        }
    );
    println!("Format Version:    v{}", header.version);
    println!("Header CRC32:      0x{:08X} (Valid ✓)", header.header_crc32);
    println!(
        "Encryption:        {} [{kcv_status}]",
        if is_encrypted {
            "ChaCha20-Poly1305 AEAD"
        } else {
            "None"
        }
    );
    println!(
        "Compression:       {}",
        if header.compression_flags == 1 {
            "Transparent LZ4 Block"
        } else {
            "None"
        }
    );
    println!(
        "WAL Sequence:      {} (Committed changes)",
        header.wal_sequence
    );
    println!("Change Counter:    {} transactions", header.change_counter);
    println!(
        "Freelist:          Trunk Page {}, Free Pages {}",
        header.freelist_trunk, header.freelist_count
    );
    println!("Vector Directory:  Root Page {}", header.vector_index_page);

    // Deep page scan or catalog inspection if accessible
    let can_inspect = !is_encrypted || (passphrase.is_some() && kcv_status.contains("Match"));

    if can_inspect {
        println!("---------------------------------------------------------------");
        println!("Inspecting Catalog & Structural Subsystems...");

        let conn_res = if let Some(ref pass) = passphrase {
            Connection::open_encrypted(p, pass)
        } else {
            Connection::open(p)
        };

        match conn_res {
            Ok(conn) => match conn.check_integrity() {
                Ok(report) => {
                    println!(
                        "  Relational Tables:     {} user tables",
                        report.tables_count
                    );
                    for t in conn.tables() {
                        println!("    - Table '{}' ({} columns)", t.name, t.columns.len());
                    }
                    println!(
                        "  Document Collections:  {} collections",
                        report.collections_count
                    );
                    for c in conn.collections() {
                        println!("    - Collection '{}'", c);
                    }
                    println!(
                        "  Knowledge Graph:       {} nodes, {} edges",
                        report.graph_nodes_count, report.graph_edges_count
                    );

                    if deep {
                        println!(
                            "  Deep Page Scan:        Scanned {} pages — {} verified",
                            report.total_pages, report.pages_verified
                        );
                    }

                    if report.is_ok() {
                        println!("---------------------------------------------------------------");
                        println!("Audit Result: PASSED (100% Integrity Verified, 0 Errors)");
                        println!("===============================================================");
                    } else {
                        println!("---------------------------------------------------------------");
                        println!(
                            "Audit Result: ANOMALIES DETECTED ({} issues):",
                            report.errors.len()
                        );
                        for err in &report.errors {
                            println!("  ✗ {err}");
                        }
                        println!("===============================================================");
                        std::process::exit(1);
                    }
                }
                Err(e) => {
                    println!("---------------------------------------------------------------");
                    println!("Audit Warning: Integrity check failed: {e}");
                    println!("===============================================================");
                }
            },
            Err(e) => {
                println!("---------------------------------------------------------------");
                println!("Audit Warning: Catalog boot error: {e}");
                println!("===============================================================");
            }
        }
    } else {
        println!("---------------------------------------------------------------");
        if is_encrypted {
            println!("Note: Provide valid --passphrase <KEY> to run deep catalog & page checks.");
        }
        println!("Audit Result: HEADER PASSED (Cryptographic & Physical Header Valid)");
        println!("===============================================================");
    }
}

trait HttpTransportStream: Read + Write {
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
fn load_tls_server_config(
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

struct HttpServerConfig<'a> {
    host: &'a str,
    port: u16,
    db_path: &'a str,
    encrypted: bool,
    api_key: Option<String>,
    api_key_scopes: Vec<(String, String)>,
    api_key_roles: Vec<ApiKeyRoleBinding>,
    cors_origins: Vec<String>,
    tls_cert: Option<String>,
    tls_key: Option<String>,
}

fn run_http_server(conn: Connection, cfg: HttpServerConfig<'_>) {
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

fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn request_content_length(headers: &[u8]) -> std::result::Result<usize, &'static str> {
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

fn constant_time_eq_str(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.as_bytes().ct_eq(b.as_bytes()).into()
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum HttpAuthScope {
    Unrestricted,
    Namespace(String),
    SqlRole {
        access: SqlRoleAccess,
        allowed_tables: Option<Vec<String>>,
    },
}

fn check_allowed_table(
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

fn authorize_where_subqueries(
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

fn authorize_sql_statement_ast(
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
                authorize_sql_statement_ast(
                    &cte.query,
                    SqlRoleAccess::ReadOnly,
                    &combined_ctes,
                    allowed_tables,
                )?;
                combined_ctes.push(cte.name.to_ascii_lowercase());
            }
            authorize_sql_statement_ast(main_query, access, &combined_ctes, allowed_tables)
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

fn authorize_sql_for_role(
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

fn extract_request_api_key(header_str: &str) -> Option<&str> {
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
        } else if lower.starts_with("x-api-key:") {
            if let Some((_, value)) = line.split_once(':') {
                provided_key = Some(value.trim());
            }
        }
    }
    provided_key
}

#[cfg(test)]
fn request_has_valid_api_key(expected_key: &str, header_str: &str) -> bool {
    extract_request_api_key(header_str).is_some_and(|key| constant_time_eq_str(key, expected_key))
}

fn authenticate_http_request(
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
    if let Some(expected) = master_key {
        if constant_time_eq_str(provided, expected) {
            resolved = Some(HttpAuthScope::Unrestricted);
        }
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

fn parse_http_sql_payload(body: &str) -> std::result::Result<(String, Vec<Value>), String> {
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

fn is_sql_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    chars
        .next()
        .is_some_and(|first| first == '_' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn parse_cors_origins(raw: &str) -> std::result::Result<Vec<String>, String> {
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

fn resolve_cors_origin<'a>(header_str: &str, allowed_origins: &'a [String]) -> Option<&'a str> {
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

fn map_memory_error_status(err: &tapirus::Error) -> &'static str {
    match err {
        tapirus::Error::PermissionDenied(_) => "403 Forbidden",
        tapirus::Error::ConstraintViolation(_) => "400 Bad Request",
        _ => "500 Internal Server Error",
    }
}

fn handle_http_memory_route(
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

fn handle_http_client<S: HttpTransportStream>(
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

        if let Some(start) = body_start {
            if request_data.len() >= start + content_length {
                break;
            }
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
        let conn = db.lock();
        let _ = ensure_chatbot_schema(&conn);
        let sql = "SELECT id, session_id, user_message, bot_reply, intent, confidence, is_safe, latency_us, created_at FROM tap_chat_logs ORDER BY id DESC LIMIT 25;";
        match conn.query(sql) {
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
        let conn = db.lock();
        match handle_http_memory_route(&conn, route_path, body_str, &auth_scope) {
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
        {
            if let Err(reason) =
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
        }

        let conn = db.lock();

        if trimmed.to_uppercase().starts_with("SELECT") {
            match conn.query_with_params(trimmed, &params) {
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
            match conn.execute_with_params(trimmed, &params) {
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
        {
            if let Err(reason) = check_allowed_table(collection, &[], allowed_tables.as_deref()) {
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
        }

        let conn = db.lock();
        let results = match vector_search_results(&conn, collection, &query_vec, k) {
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

        let conn = db.lock();
        let config = tapirus::GraphRagConfig {
            top_seeds: seeds,
            max_hops: hops,
            limit: seeds.max(5),
            ..Default::default()
        };

        let rag_ctx = conn.graph_rag_query(query, query_vec.as_deref(), &config);
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

fn format_http_response(
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

fn send_http_unauthorized_response<W: Write>(
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

fn send_http_response<W: Write>(
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

fn vector_search_results(
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

fn value_to_json(val: &Value) -> serde_json::Value {
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

fn sql_escape_string(s: &str) -> String {
    s.replace('\'', "''")
}

fn detect_language(text: &str) -> &'static str {
    let lower = text.to_lowercase();
    let words: Vec<&str> = lower.split_whitespace().collect();

    let ms_markers = [
        "saya",
        "sy",
        "nak",
        "nk",
        "boleh",
        "bagaimana",
        "macam",
        "cmne",
        "apa",
        "cara",
        "pelan",
        "harga",
        "tukar",
        "tolong",
        "ada",
        "ini",
        "tu",
        "dan",
        "ke",
        "di",
        "hai",
        "khabar",
        "guna",
        "buat",
        "pangkalan",
        "data",
        "terbenam",
        "vektor",
        "graf",
        "enjin",
    ];
    let fr_markers = [
        "bonjour",
        "comment",
        "merci",
        "avec",
        "pour",
        "votre",
        "base",
        "donnees",
        "aidez",
        "mot",
        "passe",
        "securite",
        "chiffrement",
    ];
    let de_markers = [
        "hallo",
        "wie",
        "danke",
        "bitte",
        "datenbank",
        "kann",
        "ich",
        "brauche",
        "hilfe",
        "unterstutzung",
        "sicherheit",
        "speicher",
    ];
    let es_markers = [
        "hola",
        "como",
        "gracias",
        "por",
        "favor",
        "para",
        "cuenta",
        "base",
        "datos",
        "ayuda",
        "reembolso",
        "seguridad",
        "cifrado",
    ];

    let count_matches =
        |markers: &[&str]| -> usize { words.iter().filter(|w| markers.contains(w)).count() };

    let ms_score = count_matches(&ms_markers);
    let fr_score = count_matches(&fr_markers);
    let de_score = count_matches(&de_markers);
    let es_score = count_matches(&es_markers);

    if ms_score > 0 && ms_score >= fr_score && ms_score >= de_score && ms_score >= es_score {
        "ms"
    } else if fr_score > 0 && fr_score >= de_score && fr_score >= es_score {
        "fr"
    } else if de_score > 0 && de_score >= es_score {
        "de"
    } else if es_score > 0 {
        "es"
    } else {
        "en"
    }
}

fn ensure_chatbot_schema(conn: &Connection) -> Result<(), String> {
    // 1. Create table for real-time chat dialogue persistence
    let sql_logs = "CREATE TABLE IF NOT EXISTS tap_chat_logs (
        id INTEGER PRIMARY KEY,
        session_id TEXT,
        user_message TEXT,
        bot_reply TEXT,
        intent TEXT,
        confidence REAL,
        is_safe INTEGER,
        latency_us INTEGER,
        created_at TEXT
    );";
    conn.execute(sql_logs).map_err(|e| e.to_string())?;

    // 2. Create knowledge base table for grounded factual responses
    let sql_kb = "CREATE TABLE IF NOT EXISTS tap_knowledge_base (
        id INTEGER PRIMARY KEY,
        category TEXT,
        keywords TEXT,
        title TEXT,
        content TEXT,
        language TEXT
    );";
    conn.execute(sql_kb).map_err(|e| e.to_string())?;

    // 3. Populate the built-in demo knowledge base if empty
    let count_rows = conn
        .query("SELECT COUNT(*) FROM tap_knowledge_base;")
        .unwrap_or_default();
    let mut count = 0i64;
    if let Some(row) = count_rows.first() {
        if let Some(Value::Integer(i)) = row.values().first() {
            count = *i;
        }
    }

    if count == 0 {
        seed_knowledge_base(conn)?;
    }

    Ok(())
}

fn seed_knowledge_base(conn: &Connection) -> Result<(), String> {
    let entries = [
        (
            1,
            "tap_cognitive_engine",
            "tap perception heuristic intent classify policy route no latency guarantee",
            "TAP heuristic routing helpers",
            "TAP provides heuristic intent classification and routing helpers. Its scores are not calibrated probabilities, and it does not independently verify facts or guarantee safety.",
            "en",
        ),
        (
            2,
            "database_architecture_and_rag",
            "architecture quad-model sql vector hnsw graph opencypher json rag embedded acid",
            "TapirusDB Quad-Model Architecture",
            "TapirusDB provides embedded SQL, vector, graph, and JSON APIs. Query coverage and persistence behavior vary by API; optional dependencies and encryption depend on the build.",
            "en",
        ),
        (
            3,
            "database_architecture_and_rag",
            "hnsw vector search grounding cosine similarity top-k tap_classify_grounded tap_verify_grounded",
            "HNSW Vector Grounding in TapirusDB",
            "TapirusDB includes HNSW vector index code and SQL vector operations. Measure retrieval latency and recall on representative data; TAP helper scores are heuristic.",
            "en",
        ),
        (
            4,
            "tap_cognitive_engine",
            "tap-deep deep transformer onnx candle neural models multilingual",
            "Tap-Deep Transformer Runtime",
            "Tap-Deep (TapDeepEngine) enables in-process transformer execution in Rust. It executes ONNX and Candle deep learning models directly in memory with zero Python dependencies for heavy multilingual understanding.",
            "en",
        ),
        (
            5,
            "python_and_developer_sdk",
            "python sdk pip connect execute query tap_classify tap_verify api",
            "TapirusDB Python SDK & Integration",
            "Developers can use TapirusDB in Python via 'import tapirus'. It provides connect(), execute(), query(), tap_classify(), and tap_verify() with native C/PyO3 bindings for AI agent memory and database workflows.",
            "en",
        ),
        (
            6,
            "security_and_encryption",
            "security chacha20 poly1305 encryption passphrase safe rust memory safety",
            "Hardware-Accelerated Encryption & Memory Safety",
            "TapirusDB includes optional page encryption. The core Rust crate forbids unsafe code; the C FFI crate contains unsafe code. This does not guarantee key zeroization, crash safety, or absence of logic errors.",
            "en",
        ),
        (
            7,
            "billing_and_enterprise_plans",
            "billing enterprise plans pricing commercial license sla support cluster",
            "TapirusDB Enterprise & Licensing Plans",
            "TapirusDB is licensed under BUSL-1.1. Support plans, service levels, clustering, and commercial terms are not established by this repository; consult current published terms.",
            "en",
        ),
        // Malay entries
        (
            8,
            "database_architecture_and_rag",
            "pangkalan data seni bina quad-model vektor hnsw graf opencypher dokumen sql rag melayu",
            "Seni Bina Quad-Model TapirusDB",
            "TapirusDB menyediakan API SQL, vektor, graf dan JSON dalam perpustakaan Rust terbenam. Sokongan query, persistence, penggunaan sumber dan keselamatan perlu diuji mengikut build dan workload.",
            "ms",
        ),
        (
            9,
            "tap_cognitive_engine",
            "tap enjin kognitif niat klasifikasi verifikasi sub-milisaat ai memori melayu",
            "Enjin Kognitif TAP Sub-Milisaat",
            "Helper TAP menggunakan isyarat heuristik untuk klasifikasi niat dan routing. Skornya bukan kebarangkalian terkalibrasi atau pengesahan fakta; masa jalan bergantung pada workload.",
            "ms",
        ),
        (
            10,
            "billing_and_enterprise_plans",
            "pelan langganan enterprise harga bayaran sokongan sla lesen komersial beli pakej",
            "Pelan Enterprise & Sokongan Komersial TapirusDB",
            "Terma sokongan, SLA, kluster, dan lesen komersial tidak ditetapkan oleh repositori ini. Semak terma rasmi semasa sebelum membuat keputusan.",
            "ms",
        ),
        (
            11,
            "python_and_developer_sdk",
            "python cara guna pasang sdk sambung kod skrip tutorial pembangunan",
            "Panduan Pembangunan Python TapirusDB",
            "Repositori mengandungi lebih daripada satu laluan integrasi Python. Periksa pakej dan binding yang digunakan; jangan anggap operasi atau tingkah laku persistence setara.",
            "ms",
        ),
        // French
        (
            12,
            "database_architecture_and_rag",
            "base de donnees architecture francais vecteur graphe securite chacha20",
            "Architecture Quad-Model TapirusDB (Français)",
            "TapirusDB est une base de données embarquée réunissant SQL relationnel, recherche vectorielle HNSW, graphes openCypher et documents JSON avec chiffrement ChaCha20-Poly1305. Les capacités et performances dépendent de la version et de la charge de travail.",
            "fr",
        ),
        // German
        (
            13,
            "database_architecture_and_rag",
            "datenbank architektur deutsch vektor graph sicherheit rust",
            "TapirusDB Quad-Model-Architektur (Deutsch)",
            "TapirusDB bietet eingebettete Rust-APIs f?r SQL, Vektoren, Graphen und JSON. Umfang, Persistenz und Ressourcenverbrauch m?ssen f?r Build und Arbeitslast gepr?ft werden.",
            "de",
        ),
        // Spanish
        (
            14,
            "database_architecture_and_rag",
            "base de datos arquitectura espanol vector grafo seguridad chacha20",
            "Arquitectura Quad-Model TapirusDB (Español)",
            "TapirusDB ofrece API Rust integradas para SQL, vectores, grafos y JSON. Compatibilidad, persistencia y recursos dependen de la versi?n y del uso.",
            "es",
        ),
    ];

    for (id, cat, kw, title, content, lang) in entries {
        let sql = format!(
            "INSERT INTO tap_knowledge_base (id, category, keywords, title, content, language) VALUES ({}, '{}', '{}', '{}', '{}', '{}');",
            id,
            sql_escape_string(cat),
            sql_escape_string(kw),
            sql_escape_string(title),
            sql_escape_string(content),
            sql_escape_string(lang)
        );
        conn.execute(&sql).map_err(|e| e.to_string())?;
    }

    Ok(())
}

fn retrieve_grounded_answer(
    conn: &Connection,
    intent: &str,
    user_message: &str,
    lang: &str,
) -> (String, &'static str, bool) {
    let lower_msg = user_message.to_lowercase();
    let words: Vec<&str> = lower_msg.split_whitespace().collect();

    let safe_lang = sql_escape_string(lang);
    let sql = format!(
        "SELECT id, category, keywords, title, content, language FROM tap_knowledge_base WHERE language = '{}' OR language = 'en';",
        safe_lang
    );

    let rows = conn.query(&sql).unwrap_or_default();
    if rows.is_empty() {
        let fallback = match lang {
            "ms" => {
                "TapirusDB ialah perpustakaan Rust terbenam dengan API SQL, vektor, graf dan JSON. Skop dan prestasi bergantung pada binaan dan workload."
            }
            _ => {
                "TapirusDB provides embedded Rust APIs for SQL, vector, graph, and JSON workloads. API scope and performance depend on the build and workload."
            }
        };
        return (fallback.to_string(), "tap_default_grounding", true);
    }

    let mut best_score = -1.0f32;
    let mut best_row: Option<(String, String)> = None;

    for row in &rows {
        let cat = row
            .values()
            .get(1)
            .and_then(|v| match v {
                Value::Text(s) => Some(s.as_str()),
                _ => None,
            })
            .unwrap_or("");
        let kw = row
            .values()
            .get(2)
            .and_then(|v| match v {
                Value::Text(s) => Some(s.as_str()),
                _ => None,
            })
            .unwrap_or("");
        let title = row
            .values()
            .get(3)
            .and_then(|v| match v {
                Value::Text(s) => Some(s.as_str()),
                _ => None,
            })
            .unwrap_or("");
        let content = row
            .values()
            .get(4)
            .and_then(|v| match v {
                Value::Text(s) => Some(s.as_str()),
                _ => None,
            })
            .unwrap_or("");
        let row_lang = row
            .values()
            .get(5)
            .and_then(|v| match v {
                Value::Text(s) => Some(s.as_str()),
                _ => None,
            })
            .unwrap_or("");

        let mut score = 0.0f32;
        if cat == intent {
            score += 3.0;
        }
        if row_lang == lang {
            score += 2.0;
        }

        let kw_lower = kw.to_lowercase();
        let title_lower = title.to_lowercase();
        for &w in &words {
            if w.len() >= 3 {
                if kw_lower.contains(w) {
                    score += 1.5;
                }
                if title_lower.contains(w) {
                    score += 2.0;
                }
            }
        }

        if score > best_score {
            best_score = score;
            best_row = Some((title.to_string(), content.to_string()));
        }
    }

    if let Some((title, content)) = best_row {
        let formatted = match lang {
            "ms" => format!(
                "### {}\n\n{}\n\n*(Jawapan disahkan melalui rekod tap_knowledge_base TapirusDB)*",
                title, content
            ),
            "fr" => format!(
                "### {}\n\n{}\n\n*(Réponse validée depuis la base de connaissances TapirusDB)*",
                title, content
            ),
            "de" => format!(
                "### {}\n\n{}\n\n*(Antwort verifiziert über tap_knowledge_base)*",
                title, content
            ),
            "es" => format!(
                "### {}\n\n{}\n\n*(Respuesta verificada en tap_knowledge_base)*",
                title, content
            ),
            _ => format!(
                "### {}\n\n{}\n\n*(Retrieved from the local knowledge base using heuristic matching; verify before relying on the answer.)",
                title, content
            ),
        };
        (formatted, "tap_knowledge_base", true)
    } else {
        let generic = match lang {
            "ms" => {
                "TapirusDB ialah perpustakaan Rust terbenam dengan API SQL, vektor, graf dan JSON. Skop dan prestasi bergantung pada binaan dan workload."
            }
            _ => {
                "TapirusDB provides embedded SQL, vector, graph, and JSON APIs. TAP helpers use heuristics; latency and resource use depend on the workload."
            }
        };
        (generic.to_string(), "tap_default_grounding", true)
    }
}

fn handle_chatbot_request(
    db: Arc<Mutex<Connection>>,
    body_str: &str,
) -> Result<serde_json::Value, String> {
    let start_time = Instant::now();

    let json_req: serde_json::Value =
        serde_json::from_str(body_str).map_err(|e| format!("Invalid JSON request payload: {e}"))?;

    let user_message = json_req
        .get("message")
        .and_then(|m| m.as_str())
        .unwrap_or("")
        .trim();

    if user_message.is_empty() {
        return Err("User message cannot be empty".to_string());
    }

    let session_id = json_req
        .get("session_id")
        .and_then(|s| s.as_str())
        .unwrap_or("default-session")
        .to_string();

    let conn = db.lock();

    // Ensure database tables exist and are seeded
    ensure_chatbot_schema(&conn)?;

    let lang = detect_language(user_message);

    // 1. Run TAP Cognitive Intent Classification
    let candidate_intents = [
        "database_architecture_and_rag",
        "tap_cognitive_engine",
        "python_and_developer_sdk",
        "security_and_encryption",
        "billing_and_enterprise_plans",
        "general_greeting_or_help",
    ];

    let classify_result = conn
        .tap()
        .classify(user_message, &candidate_intents)
        .map_err(|e| format!("TAP classification error: {e}"))?;

    let intent = classify_result.top_choice.clone();
    let confidence = classify_result.confidence;

    // 2. Run TAP Safety & Policy Verification
    let lower_msg = user_message.to_lowercase();
    let is_destructive = lower_msg.contains("drop table")
        || lower_msg.contains("drop database")
        || lower_msg.contains("truncate table")
        || lower_msg.contains("rm -rf")
        || lower_msg.contains("bypass security")
        || lower_msg.contains("ignore previous instructions");

    let is_safe = if is_destructive {
        false
    } else {
        // Run TAP verification to ensure query doesn't contradict safe policy
        let v = conn.tap().verify(user_message, user_message);
        v.map(|r| r.is_verified).unwrap_or(false)
    };

    // 3. Grounded Retrieval
    let (reply, grounding_source, is_grounded) = if !is_safe {
        let msg = match lang {
            "ms" => {
                "Mesej anda mengandungi ungkapan yang tidak mematuhi polisi keselamatan kami. Sila kemukakan soalan berkaitan pangkalan data secara sopan."
            }
            "fr" => {
                "Votre message ne respecte pas les règles de sécurité. Veuillez poser une question constructive."
            }
            "de" => {
                "Ihre Anfrage entspricht nicht unseren Sicherheitsrichtlinien. Bitte stellen Sie eine sachliche Frage."
            }
            "es" => {
                "Su mensaje no cumple con las directivas de seguridad. Por favor formule una consulta válida."
            }
            _ => {
                "Your query was flagged by TAP safety verification. Please submit a constructive database inquiry."
            }
        };
        (msg.to_string(), "tap_policy_guardrail", false)
    } else if intent == "general_greeting_or_help" {
        let greeting = match lang {
            "ms" => {
                "Hai. Demo setempat ini menggunakan klasifikasi niat heuristik dan carian kata kunci. Skornya tidak dikalibrasi dan jawapan perlu disemak."
            }
            "fr" => {
                "Bonjour. Cette d?monstration utilise des heuristiques et une recherche locale par mots-cl?s. Les scores ne sont pas calibr?s; les r?ponses doivent ?tre v?rifi?es."
            }
            "de" => {
                "Hallo. Diese Demo verwendet heuristische Klassifizierung und lokale Stichwortsuche. Die Werte sind nicht kalibriert; Antworten sollten ?berpr?ft werden."
            }
            "es" => {
                "Hola. Esta demostraci?n usa heur?sticas y b?squeda local por palabras clave. Las puntuaciones no est?n calibradas; las respuestas deben verificarse."
            }
            _ => {
                "Hello. This local demo uses heuristic intent classification and keyword retrieval. Its scores are not calibrated and its answers should be checked."
            }
        };
        (greeting.to_string(), "tap_conversational_core", true)
    } else {
        retrieve_grounded_answer(&conn, &intent, user_message, lang)
    };

    let elapsed = start_time.elapsed();
    let latency_us = elapsed.as_micros() as i64;
    let latency_ms = elapsed.as_secs_f64() * 1000.0;

    // 4. Persist to tap_chat_logs in SQL database
    let now_ts = web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let created_at = format!("{now_ts}");

    let insert_sql = format!(
        "INSERT INTO tap_chat_logs (session_id, user_message, bot_reply, intent, confidence, is_safe, latency_us, created_at) VALUES ('{}', '{}', '{}', '{}', {:.4}, {}, {}, '{}');",
        sql_escape_string(&session_id),
        sql_escape_string(user_message),
        sql_escape_string(&reply),
        sql_escape_string(&intent),
        confidence,
        if is_safe { 1 } else { 0 },
        latency_us,
        sql_escape_string(&created_at)
    );

    let logged_to_sql = conn.execute(&insert_sql).is_ok();

    Ok(serde_json::json!({
        "session_id": session_id,
        "user_message": user_message,
        "reply": reply,
        "intent": intent,
        "confidence": (confidence * 100.0).round() / 100.0,
        "confidence_kind": "heuristic_unvalidated",
        "policy_check_kind": "heuristic_not_a_safety_guarantee",
        "is_safe": is_safe,
        "is_grounded": is_grounded,
        "grounding_source": grounding_source,
        "latency_us": latency_us,
        "latency_ms": (latency_ms * 100.0).round() / 100.0,
        "logged_to_sql": logged_to_sql
    }))
}

const CHATBOT_HTML: &str = r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>TapirusDB AI Cognitive Chatbot</title>
<style>
:root {
  --bg-dark: #080c14;
  --bg-panel: #0f172a;
  --bg-card: #1e293b;
  --bg-hover: #334155;
  --border: rgba(255, 255, 255, 0.08);
  --border-glow: rgba(56, 189, 248, 0.25);
  --text-main: #f8fafc;
  --text-muted: #94a3b8;
  --cyan: #38bdf8;
  --blue: #0284c7;
  --emerald: #10b981;
  --indigo: #6366f1;
}

* { box-sizing: border-box; margin: 0; padding: 0; }

body {
  font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Inter", sans-serif;
  background-color: var(--bg-dark);
  background-image: radial-gradient(at 0% 0%, rgba(2, 132, 199, 0.15) 0px, transparent 50%),
                    radial-gradient(at 100% 100%, rgba(99, 102, 241, 0.12) 0px, transparent 50%);
  color: var(--text-main);
  height: 100vh;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

header {
  background: rgba(15, 23, 42, 0.85);
  backdrop-filter: blur(16px);
  border-bottom: 1px solid var(--border);
  padding: 12px 24px;
  display: flex;
  align-items: center;
  justify-content: space-between;
  z-index: 20;
}

.logo-group {
  display: flex;
  align-items: center;
  gap: 12px;
}

.logo-icon {
  width: 36px;
  height: 36px;
  background: linear-gradient(135deg, #0284c7, #6366f1);
  border-radius: 10px;
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 1.25rem;
  box-shadow: 0 0 15px rgba(2, 132, 199, 0.4);
}

.logo-text h1 {
  font-size: 1.15rem;
  font-weight: 700;
  letter-spacing: -0.02em;
  color: #ffffff;
}

.logo-text p {
  font-size: 0.75rem;
  color: var(--cyan);
  font-weight: 500;
}

.header-actions {
  display: flex;
  align-items: center;
  gap: 10px;
}

.status-pill {
  display: flex;
  align-items: center;
  gap: 6px;
  background: rgba(16, 185, 129, 0.1);
  border: 1px solid rgba(16, 185, 129, 0.3);
  color: #34d399;
  font-size: 0.75rem;
  font-weight: 600;
  padding: 5px 12px;
  border-radius: 999px;
}

.status-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: #10b981;
  box-shadow: 0 0 8px #10b981;
  animation: pulse 2s infinite;
}

@keyframes pulse {
  0% { transform: scale(0.95); opacity: 0.8; }
  50% { transform: scale(1.15); opacity: 1; }
  100% { transform: scale(0.95); opacity: 0.8; }
}

.btn-header {
  background: var(--bg-card);
  color: var(--text-main);
  border: 1px solid var(--border);
  padding: 6px 14px;
  border-radius: 8px;
  font-size: 0.8rem;
  font-weight: 600;
  cursor: pointer;
  transition: all 0.2s;
  display: inline-flex;
  align-items: center;
  gap: 6px;
}

.btn-header:hover {
  background: var(--bg-hover);
  border-color: var(--border-glow);
}

main {
  flex: 1;
  display: flex;
  flex-direction: column;
  max-width: 960px;
  width: 100%;
  margin: 0 auto;
  padding: 16px 20px 20px;
  height: calc(100vh - 65px);
  overflow: hidden;
}

.chat-container {
  flex: 1;
  overflow-y: auto;
  padding-right: 8px;
  display: flex;
  flex-direction: column;
  gap: 18px;
  scroll-behavior: smooth;
}

.chat-container::-webkit-scrollbar {
  width: 6px;
}
.chat-container::-webkit-scrollbar-thumb {
  background: #334155;
  border-radius: 4px;
}

.welcome-card {
  background: rgba(30, 41, 59, 0.6);
  backdrop-filter: blur(12px);
  border: 1px solid var(--border);
  border-radius: 14px;
  padding: 24px;
  text-align: center;
  margin-top: 10px;
  box-shadow: 0 10px 25px rgba(0, 0, 0, 0.3);
}

.welcome-badge {
  display: inline-block;
  background: linear-gradient(135deg, rgba(2, 132, 199, 0.2), rgba(99, 102, 241, 0.2));
  border: 1px solid var(--border-glow);
  color: var(--cyan);
  padding: 4px 12px;
  border-radius: 999px;
  font-size: 0.75rem;
  font-weight: 700;
  letter-spacing: 0.05em;
  margin-bottom: 12px;
}

.welcome-card h2 {
  font-size: 1.4rem;
  margin-bottom: 8px;
  color: #ffffff;
}

.welcome-card p {
  color: var(--text-muted);
  font-size: 0.9rem;
  line-height: 1.6;
  max-width: 680px;
  margin: 0 auto 18px;
}

.suggestions-grid {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  justify-content: center;
}

.suggestion-chip {
  background: rgba(15, 23, 42, 0.8);
  border: 1px solid var(--border);
  color: #e2e8f0;
  padding: 7px 14px;
  border-radius: 20px;
  font-size: 0.82rem;
  cursor: pointer;
  transition: all 0.2s;
  display: inline-flex;
  align-items: center;
  gap: 6px;
}

.suggestion-chip:hover {
  background: rgba(2, 132, 199, 0.25);
  border-color: var(--cyan);
  color: #ffffff;
  transform: translateY(-1px);
}

.msg-row {
  display: flex;
  width: 100%;
}

.msg-row.user {
  justify-content: flex-end;
}

.msg-row.bot {
  justify-content: flex-start;
}

.msg-bubble {
  max-width: 82%;
  border-radius: 14px;
  padding: 14px 18px;
  font-size: 0.92rem;
  line-height: 1.6;
  word-break: break-word;
}

.msg-row.user .msg-bubble {
  background: linear-gradient(135deg, #0284c7, #2563eb);
  color: #ffffff;
  border-bottom-right-radius: 4px;
  box-shadow: 0 4px 15px rgba(2, 132, 199, 0.25);
}

.msg-row.bot .msg-bubble {
  background: var(--bg-card);
  border: 1px solid var(--border-glow);
  color: #f1f5f9;
  border-bottom-left-radius: 4px;
  box-shadow: 0 4px 20px rgba(0, 0, 0, 0.25);
}

.telemetry-bar {
  margin-top: 12px;
  padding-top: 10px;
  border-top: 1px solid rgba(255, 255, 255, 0.08);
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
}

.telemetry-pill {
  font-size: 0.72rem;
  font-weight: 600;
  padding: 3px 8px;
  border-radius: 6px;
  display: inline-flex;
  align-items: center;
  gap: 4px;
}

.pill-intent {
  background: rgba(99, 102, 241, 0.15);
  border: 1px solid rgba(99, 102, 241, 0.35);
  color: #a5b4fc;
}

.pill-latency {
  background: rgba(16, 185, 129, 0.15);
  border: 1px solid rgba(16, 185, 129, 0.35);
  color: #6ee7b7;
}

.pill-safe {
  background: rgba(56, 189, 248, 0.15);
  border: 1px solid rgba(56, 189, 248, 0.35);
  color: #7dd3fc;
}

.pill-sql {
  background: rgba(245, 158, 11, 0.15);
  border: 1px solid rgba(245, 158, 11, 0.35);
  color: #fcd34d;
}

pre {
  background: #090d16;
  border: 1px solid #334155;
  border-radius: 8px;
  padding: 10px 14px;
  margin: 10px 0;
  overflow-x: auto;
  font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
  font-size: 0.85em;
  color: #38bdf8;
}

code {
  font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
  background: rgba(15, 23, 42, 0.6);
  padding: 2px 6px;
  border-radius: 4px;
  font-size: 0.88em;
  color: #38bdf8;
}

.typing-indicator {
  display: flex;
  align-items: center;
  gap: 5px;
  padding: 10px 16px;
  background: var(--bg-card);
  border: 1px solid var(--border);
  border-radius: 14px;
  width: fit-content;
}

.typing-dot {
  width: 6px;
  height: 6px;
  background: var(--cyan);
  border-radius: 50%;
  animation: typing 1.4s infinite ease-in-out;
}
.typing-dot:nth-child(2) { animation-delay: 0.2s; }
.typing-dot:nth-child(3) { animation-delay: 0.4s; }

@keyframes typing {
  0%, 60%, 100% { transform: translateY(0); opacity: 0.4; }
  30% { transform: translateY(-4px); opacity: 1; }
}

.input-container {
  margin-top: 14px;
  display: flex;
  gap: 10px;
  align-items: center;
  background: rgba(15, 23, 42, 0.8);
  backdrop-filter: blur(12px);
  border: 1px solid var(--border-glow);
  border-radius: 12px;
  padding: 6px 8px 6px 14px;
  box-shadow: 0 10px 25px rgba(0, 0, 0, 0.35);
}

.input-container:focus-within {
  border-color: var(--cyan);
  box-shadow: 0 0 18px rgba(56, 189, 248, 0.3);
}

#chat-input {
  flex: 1;
  background: transparent;
  border: none;
  outline: none;
  color: #ffffff;
  font-size: 0.95rem;
  font-family: inherit;
  padding: 8px 0;
}

#chat-input::placeholder {
  color: #64748b;
}

.send-btn {
  background: linear-gradient(135deg, #0284c7, #2563eb);
  color: #ffffff;
  border: none;
  outline: none;
  padding: 10px 18px;
  border-radius: 8px;
  font-weight: 600;
  font-size: 0.9rem;
  cursor: pointer;
  transition: all 0.2s;
  display: inline-flex;
  align-items: center;
  gap: 6px;
}

.send-btn:hover {
  filter: brightness(1.15);
  transform: translateY(-1px);
}

.send-btn:disabled {
  background: #334155;
  color: #94a3b8;
  cursor: not-allowed;
  transform: none;
}

/* Modal Drawer for SQL Logs */
.drawer-backdrop {
  position: fixed;
  top: 0; left: 0; right: 0; bottom: 0;
  background: rgba(0, 0, 0, 0.7);
  backdrop-filter: blur(6px);
  z-index: 100;
  display: none;
  justify-content: flex-end;
}

.drawer {
  background: #0f172a;
  border-left: 1px solid var(--border-glow);
  width: 90%;
  max-width: 780px;
  height: 100%;
  display: flex;
  flex-direction: column;
  box-shadow: -10px 0 35px rgba(0,0,0,0.6);
  animation: slideIn 0.3s ease-out;
}

@keyframes slideIn {
  from { transform: translateX(100%); }
  to { transform: translateX(0); }
}

.drawer-header {
  padding: 16px 20px;
  border-bottom: 1px solid var(--border);
  display: flex;
  justify-content: space-between;
  align-items: center;
}

.drawer-header h3 {
  font-size: 1.1rem;
  color: #ffffff;
  display: flex;
  align-items: center;
  gap: 8px;
}

.drawer-body {
  flex: 1;
  overflow-y: auto;
  padding: 16px 20px;
}

table {
  width: 100%;
  border-collapse: collapse;
  font-size: 0.8rem;
}

th, td {
  padding: 10px 12px;
  text-align: left;
  border-bottom: 1px solid rgba(255,255,255,0.06);
}

th {
  background: #1e293b;
  color: var(--cyan);
  font-weight: 600;
  position: sticky;
  top: 0;
}

tr:hover {
  background: rgba(255,255,255,0.03);
}

.badge-id {
  background: #1e293b;
  color: #38bdf8;
  padding: 2px 6px;
  border-radius: 4px;
  font-weight: 600;
}

.badge-intent {
  color: #a5b4fc;
}

.badge-latency {
  color: #34d399;
  font-weight: 600;
}

.truncate-cell {
  max-width: 220px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
</style>
</head>
<body>

<header>
  <div class="logo-group">
    <div class="logo-icon">🦣</div>
    <div class="logo-text">
      <h1>TapirusDB chat prototype</h1>
      <p>Heuristic intent classification and local keyword retrieval</p>
    </div>
  </div>
  <div class="header-actions">
    <div class="status-pill">
      <div class="status-dot"></div>
      <span>Demo active</span>
    </div>
    <button class="btn-header" onclick="openLogsDrawer()">📊 Inspect SQL Logs</button>
    <button class="btn-header" onclick="clearChat()">🗑️ Clear</button>
  </div>
</header>

<main>
  <div class="chat-container" id="chat-messages">
    <div class="welcome-card" id="welcome-banner">
      <div class="welcome-badge">TAP EMBEDDED COGNITIVE ARCHITECTURE</div>
      <h2>TapirusDB Online AI Assistant</h2>
      <p>
        Direct in-process cognitive triage running within TapirusDB. Your queries undergo 
        <strong>intent classification</strong>, <strong>policy verification</strong>, and 
        <strong>local knowledge retrieval using heuristic matching. The demo does not provide a latency or factual-accuracy guarantee.
      </p>
      <div class="suggestions-grid">
        <button class="suggestion-chip" onclick="askQuick(this.innerText)">🇲🇾 Saya nak pelan langganan enterprise TapirusDB</button>
        <button class="suggestion-chip" onclick="askQuick(this.innerText)">🇲🇾 Bagaimana cara sambung TapirusDB guna Python?</button>
        <button class="suggestion-chip" onclick="askQuick(this.innerText)">🇬🇧 How do the TAP decision helpers work?</button>
        <button class="suggestion-chip" onclick="askQuick(this.innerText)">🇬🇧 Explain HNSW vector search and grounding in TapirusDB</button>
        <button class="suggestion-chip" onclick="askQuick(this.innerText)">🇫🇷 Architecture et sécurité de TapirusDB</button>
      </div>
    </div>
  </div>

  <div class="input-container">
    <input type="text" id="chat-input" placeholder="Type your query (English, Melayu, Français, Deutsch, Español)..." autofocus>
    <button class="send-btn" id="send-btn" onclick="sendMessage()">
      <span>Send</span>
      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M22 2L11 13M22 2l-7 20-4-9-9-4 20-7z"/></svg>
    </button>
  </div>
</main>

<div class="drawer-backdrop" id="logs-drawer" onclick="closeLogsOnBackdrop(event)">
  <div class="drawer">
    <div class="drawer-header">
      <h3>📊 Live Relational SQL Persistence (tap_chat_logs)</h3>
      <div style="display:flex;gap:8px;">
        <button class="btn-header" onclick="loadSqlLogs()">🔄 Refresh</button>
        <button class="btn-header" onclick="closeLogsDrawer()">✕ Close</button>
      </div>
    </div>
    <div class="drawer-body">
      <table>
        <thead>
          <tr>
            <th>ID</th>
            <th>User Query</th>
            <th>Intent</th>
            <th>TAP heuristic score (not probability)</th>
            <th>Latency</th>
            <th>Policy</th>
            <th>Time</th>
          </tr>
        </thead>
        <tbody id="logs-tbody">
          <tr><td colspan="7" style="text-align:center;padding:20px;color:#94a3b8;">Loading persisted logs from tap_chat_logs...</td></tr>
        </tbody>
      </table>
    </div>
  </div>
</div>

<script>
const sessionId = 'session-' + Math.random().toString(36).substring(2, 10);
let isWaiting = false;

function escapeHtml(str) {
  var div = document.createElement('div');
  div.textContent = str;
  return div.innerHTML;
}

function renderMarkdown(text) {
  let html = escapeHtml(text);
  // Code blocks
  html = html.replace(/```([\s\S]*?)```/g, function(match, p1) {
    return '<pre><code>' + p1 + '</code></pre>';
  });
  // Inline code
  html = html.replace(/`([^`]+)`/g, '<code>$1</code>');
  // Bold
  html = html.replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>');
  // Heading ###
  html = html.replace(/^### (.*$)/gim, '<h4 style="margin:8px 0 6px;color:#38bdf8;font-size:1.05rem;">$1</h4>');
  // Lists
  html = html.replace(/^\* (.*$)/gim, '&bull; $1');
  // Newlines
  html = html.replace(/\n/g, '<br>');
  return html;
}

function appendUserMessage(text) {
  const container = document.getElementById('chat-messages');
  const row = document.createElement('div');
  row.className = 'msg-row user';
  row.innerHTML = `<div class="msg-bubble">${escapeHtml(text)}</div>`;
  container.appendChild(row);
  container.scrollTop = container.scrollHeight;
}

function appendBotMessage(data) {
  const container = document.getElementById('chat-messages');
  const row = document.createElement('div');
  row.className = 'msg-row bot';

  const replyHtml = renderMarkdown(data.reply || '');
  const confPercent = Math.round((data.confidence || 0) * 100);
  const latencyUs = data.latency_us || 0;
  const latencyMs = data.latency_ms || (latencyUs / 1000).toFixed(2);

  row.innerHTML = `
    <div class="msg-bubble">
      <div>${replyHtml}</div>
      <div class="telemetry-bar">
        <span class="telemetry-pill pill-intent">🎯 Intent heuristic: ${escapeHtml(data.intent || 'general')} (${confPercent}%)</span>
        <span class="telemetry-pill pill-latency">⚡ Latency: ${latencyUs} µs (${latencyMs} ms)</span>
        <span class="telemetry-pill pill-safe">${data.is_safe ? '🛡️ Heuristic check: passed' : '⚠️ Heuristic check: flagged'}</span>
        <span class="telemetry-pill pill-safe">🧠 ${escapeHtml(data.grounding_source || 'grounded')}</span>
        <span class="telemetry-pill pill-sql">💾 Stored: tap_chat_logs</span>
      </div>
    </div>
  `;
  container.appendChild(row);
  container.scrollTop = container.scrollHeight;
}

function appendErrorMessage(errText) {
  const container = document.getElementById('chat-messages');
  const row = document.createElement('div');
  row.className = 'msg-row bot';
  row.innerHTML = `<div class="msg-bubble" style="border-color:#ef4444;color:#fca5a5;">⚠️ Error: ${escapeHtml(errText)}</div>`;
  container.appendChild(row);
  container.scrollTop = container.scrollHeight;
}

function showTypingIndicator() {
  const container = document.getElementById('chat-messages');
  const row = document.createElement('div');
  row.className = 'msg-row bot';
  row.id = 'typing-row';
  row.innerHTML = `
    <div class="typing-indicator">
      <div class="typing-dot"></div>
      <div class="typing-dot"></div>
      <div class="typing-dot"></div>
    </div>
  `;
  container.appendChild(row);
  container.scrollTop = container.scrollHeight;
}

function removeTypingIndicator() {
  const row = document.getElementById('typing-row');
  if (row) row.remove();
}

async function sendMessage(customText) {
  const input = document.getElementById('chat-input');
  const text = (customText !== undefined ? customText : input.value).trim();
  if (!text || isWaiting) return;

  const banner = document.getElementById('welcome-banner');
  if (banner) banner.style.display = 'none';

  input.value = '';
  isWaiting = true;
  document.getElementById('send-btn').disabled = true;

  appendUserMessage(text);
  showTypingIndicator();

  try {
    const res = await fetch('/api/chat', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ message: text, session_id: sessionId })
    });
    const data = await res.json();
    removeTypingIndicator();
    if (res.ok) {
      appendBotMessage(data);
    } else {
      appendErrorMessage(data.error || 'Failed to process cognitive request');
    }
  } catch (err) {
    removeTypingIndicator();
    appendErrorMessage('Connection error: ' + err.message);
  } finally {
    isWaiting = false;
    document.getElementById('send-btn').disabled = false;
    input.focus();
  }
}

function askQuick(text) {
  sendMessage(text);
}

function clearChat() {
  const container = document.getElementById('chat-messages');
  container.innerHTML = `
    <div class="welcome-card" id="welcome-banner">
      <div class="welcome-badge">TAP EMBEDDED COGNITIVE ARCHITECTURE</div>
      <h2>TapirusDB Online AI Assistant</h2>
      <p>The local demo uses heuristic intent classification and keyword retrieval. Its results are not calibrated or independently verified, and no latency guarantee is made.</p>
      <div class="suggestions-grid">
        <button class="suggestion-chip" onclick="askQuick(this.innerText)">🇲🇾 Saya nak pelan langganan enterprise TapirusDB</button>
        <button class="suggestion-chip" onclick="askQuick(this.innerText)">🇲🇾 Bagaimana cara sambung TapirusDB guna Python?</button>
        <button class="suggestion-chip" onclick="askQuick(this.innerText)">🇬🇧 How do the TAP decision helpers work?</button>
        <button class="suggestion-chip" onclick="askQuick(this.innerText)">🇬🇧 Explain HNSW vector search and grounding in TapirusDB</button>
      </div>
    </div>
  `;
}

function openLogsDrawer() {
  document.getElementById('logs-drawer').style.display = 'flex';
  loadSqlLogs();
}

function closeLogsDrawer() {
  document.getElementById('logs-drawer').style.display = 'none';
}

function closeLogsOnBackdrop(e) {
  if (e.target.id === 'logs-drawer') {
    closeLogsDrawer();
  }
}

async function loadSqlLogs() {
  const tbody = document.getElementById('logs-tbody');
  tbody.innerHTML = '<tr><td colspan="7" style="text-align:center;padding:20px;color:#94a3b8;">Querying tap_chat_logs...</td></tr>';
  try {
    const res = await fetch('/api/chat/logs');
    const data = await res.json();
    if (data.logs && data.logs.length > 0) {
      tbody.innerHTML = data.logs.map(log => `
        <tr>
          <td><span class="badge-id">#${log.id}</span></td>
          <td><div class="truncate-cell">${escapeHtml(log.user_message || '')}</div></td>
          <td><span class="badge-intent">${escapeHtml(log.intent || '')}</span></td>
          <td>${Math.round((log.confidence || 0) * 100)}%</td>
          <td><span class="badge-latency">${log.latency_us || 0} µs</span></td>
          <td>${log.is_safe ? '✅ Safe' : '⚠️ Flagged'}</td>
          <td>${new Date((log.created_at || 0) * 1000).toLocaleTimeString()}</td>
        </tr>
      `).join('');
    } else {
      tbody.innerHTML = '<tr><td colspan="7" style="text-align:center;padding:20px;color:#94a3b8;">No chat records in tap_chat_logs yet. Send a message to see live persistence!</td></tr>';
    }
  } catch (err) {
    tbody.innerHTML = `<tr><td colspan="7" style="text-align:center;padding:20px;color:#ef4444;">Error loading logs: ${escapeHtml(err.message)}</td></tr>`;
  }
}

document.getElementById('chat-input').addEventListener('keydown', function(e) {
  if (e.key === 'Enter' && !e.shiftKey) {
    e.preventDefault();
    sendMessage();
  }
});
</script>
</body>
</html>
"##;

fn run_repl(conn: &Connection, target: &str) {
    let stdin = io::stdin();
    let mut reader = stdin.lock();
    let mut stdout = io::stdout();
    let mut buffer = String::new();

    loop {
        if buffer.trim().is_empty() {
            print!("tapirus> ");
        } else {
            print!("   ...> ");
        }
        let _ = stdout.flush();

        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => break, // EOF
            Ok(_) => {}
            Err(e) => {
                eprintln!("Read error: {e}");
                break;
            }
        }

        let trimmed = line.trim();

        // Handle dot-commands when buffer is empty
        if buffer.trim().is_empty() && trimmed.starts_with('.') {
            if handle_dot_command(conn, trimmed, target) {
                break; // Exit requested
            }
            continue;
        }

        buffer.push(' ');
        buffer.push_str(trimmed);

        if buffer.contains(';') {
            let statements: Vec<&str> = buffer.split(';').collect();
            let num_complete = statements.len() - 1;

            for stmt in statements.iter().take(num_complete) {
                let sql = stmt.trim();
                if !sql.is_empty() {
                    let _ = execute_statement(conn, sql);
                }
            }

            buffer = statements.last().unwrap_or(&"").to_string();
        }
    }

    println!("\nGoodbye!");
}

fn handle_dot_command(conn: &Connection, cmd: &str, target: &str) -> bool {
    let parts: Vec<&str> = cmd.split_whitespace().collect();
    if parts.is_empty() {
        return false;
    }

    match parts[0] {
        ".exit" | ".quit" | ".q" => true,
        ".help" => {
            print_dot_help();
            false
        }
        ".tables" => {
            let tables = conn.tables();
            if tables.is_empty() {
                println!("(No relational tables found)");
            } else {
                println!("Relational Tables ({}):", tables.len());
                for t in tables {
                    println!("  • {} ({} columns)", t.name, t.columns.len());
                }
            }
            false
        }
        ".schema" => {
            if parts.len() > 1 {
                let table_name = parts[1];
                if let Some(t) = conn.table(table_name) {
                    print_table_ddl(&t);
                } else {
                    println!("Error: Table '{table_name}' not found");
                }
            } else {
                let tables = conn.tables();
                if tables.is_empty() {
                    println!("(No tables defined)");
                } else {
                    for t in tables {
                        print_table_ddl(&t);
                        println!();
                    }
                }
            }
            false
        }
        ".collections" => {
            let cols = conn.collections();
            if cols.is_empty() {
                println!("(No document collections found)");
            } else {
                println!("Document Collections ({}):", cols.len());
                for c in cols {
                    if let Ok(col) = conn.collection(&c) {
                        let count = col.count().unwrap_or(0);
                        println!("  • {c} ({count} documents)");
                    } else {
                        println!("  • {c}");
                    }
                }
            }
            false
        }
        ".doc" => {
            handle_doc_command(conn, &parts);
            false
        }
        ".graph" => {
            handle_graph_command(conn, &parts);
            false
        }
        ".memory" => {
            handle_memory_command(conn, &parts);
            false
        }
        ".checkpoint" => {
            let start = Instant::now();
            match conn.checkpoint() {
                Ok(flushed) => {
                    println!(
                        "WAL Checkpoint complete: {flushed} page(s) flushed in {:?}",
                        start.elapsed()
                    );
                }
                Err(e) => {
                    println!("Checkpoint error: {e}");
                }
            }
            false
        }
        ".backup" => {
            if parts.len() < 2 {
                println!("Usage: .backup <destination.tapir>");
            } else {
                let dest = parts[1];
                let start = Instant::now();
                match conn.backup(dest) {
                    Ok(pages) => {
                        println!(
                            "Hot backup created successfully at '{dest}' ({pages} pages) in {:?}",
                            start.elapsed()
                        );
                    }
                    Err(e) => {
                        println!("Backup error: {e}");
                    }
                }
            }
            false
        }
        ".vacuum" => {
            let start = Instant::now();
            if parts.len() >= 3 && parts[1].eq_ignore_ascii_case("into") {
                let dest = parts[2];
                match conn.vacuum_into(dest) {
                    Ok(pages) => {
                        println!(
                            "Vacuum into '{dest}' completed ({pages} pages) in {:?}",
                            start.elapsed()
                        );
                    }
                    Err(e) => {
                        println!("Vacuum error: {e}");
                    }
                }
            } else {
                match conn.vacuum() {
                    Ok(flushed) => {
                        println!(
                            "In-place vacuum complete: {flushed} page(s) checkpointed in {:?}",
                            start.elapsed()
                        );
                    }
                    Err(e) => {
                        println!("Vacuum error: {e}");
                    }
                }
            }
            false
        }
        ".dump" => {
            handle_dump_command(conn, &parts);
            false
        }
        ".export" => {
            if parts.len() < 3 {
                println!("Usage: .export <table_or_collection> <output_file.json>");
            } else {
                handle_export_command(conn, parts[1], parts[2]);
            }
            false
        }
        ".import" => {
            if parts.len() < 3 {
                println!("Usage: .import <file.csv> <table>");
            } else {
                handle_import_csv(conn, parts[1], parts[2]);
            }
            false
        }
        ".info" => {
            println!("Database: {target}");
            println!("Page Size: 4,096 bytes");
            println!("TapirusDB v{VERSION} - Embedded Rust database");
            let (nodes, edges) = conn.graph_stats();
            println!("Knowledge Graph: {nodes} nodes, {edges} edges");
            println!("Document Collections: {}", conn.collections().len());
            println!("Relational Tables: {}", conn.tables().len());
            false
        }
        _ => {
            println!(
                "Unknown command: '{}'. Type '.help' for available commands.",
                parts[0]
            );
            false
        }
    }
}

fn handle_doc_command(conn: &Connection, parts: &[&str]) {
    if parts.len() < 3 {
        println!("Usage: .doc <collection> find");
        println!("       .doc <collection> insert <json>");
        return;
    }

    let col_name = parts[1];
    let action = parts[2];

    let col = match conn.collection(col_name) {
        Ok(c) => c,
        Err(e) => {
            println!("Error accessing collection '{col_name}': {e}");
            return;
        }
    };

    match action {
        "find" => match col.find_all() {
            Ok(docs) => {
                if docs.is_empty() {
                    println!("(Collection '{col_name}' is empty)");
                } else {
                    println!("Documents in '{col_name}' ({}):", docs.len());
                    for (id, doc) in docs {
                        let formatted = serde_json::to_string_pretty(&doc).unwrap_or_default();
                        println!("  [{id}]: {formatted}");
                    }
                }
            }
            Err(e) => println!("Query error: {e}"),
        },
        "insert" => {
            if parts.len() < 4 {
                println!("Error: Missing JSON payload");
                return;
            }
            let json_str = parts[3..].join(" ");
            match serde_json::from_str::<serde_json::Value>(&json_str) {
                Ok(json_val) => match col.insert_one(&json_val) {
                    Ok(id) => println!("Inserted document with ID: {id}"),
                    Err(e) => println!("Insert error: {e}"),
                },
                Err(e) => println!("Invalid JSON format: {e}"),
            }
        }
        _ => println!("Unknown action '{action}'. Use 'find' or 'insert'."),
    }
}

fn handle_graph_command(conn: &Connection, parts: &[&str]) {
    if parts.len() == 1 {
        let (nodes, edges) = conn.graph_stats();
        println!("Embedded Knowledge Graph Statistics:");
        println!("  • Total Nodes: {nodes}");
        println!("  • Total Edges: {edges}");
        return;
    }

    match parts[1] {
        "nodes" => {
            let nodes = conn.graph_nodes();
            if nodes.is_empty() {
                println!("(Graph contains no nodes)");
            } else {
                println!("Graph Nodes ({}):", nodes.len());
                for n in nodes {
                    println!("  #{}: [{}] {}", n.id, n.label, n.properties);
                }
            }
        }
        "edges" => {
            let edges = conn.graph_edges();
            if edges.is_empty() {
                println!("(Graph contains no edges)");
            } else {
                println!("Graph Edges ({}):", edges.len());
                for e in edges {
                    println!(
                        "  #{}: (#{}) -[{}]-> (#{}), weight: {}",
                        e.id, e.from_id, e.label, e.to_id, e.weight
                    );
                }
            }
        }
        "path" => {
            if parts.len() < 4 {
                println!("Usage: .graph path <from_id> <to_id>");
                return;
            }
            let from_id: u64 = match parts[2].parse() {
                Ok(id) => id,
                Err(_) => {
                    println!("Error: Invalid from_id '{}'", parts[2]);
                    return;
                }
            };
            let to_id: u64 = match parts[3].parse() {
                Ok(id) => id,
                Err(_) => {
                    println!("Error: Invalid to_id '{}'", parts[3]);
                    return;
                }
            };

            let start = Instant::now();
            if let Some(path) = conn.graph_find_path(from_id, to_id, 10) {
                println!(
                    "Shortest path ({}) hops found in {:?}:",
                    path.len(),
                    start.elapsed()
                );
                for (i, edge) in path.iter().enumerate() {
                    println!(
                        "  Step {}: (#{}) -[{}]-> (#{})",
                        i + 1,
                        edge.from_id,
                        edge.label,
                        edge.to_id
                    );
                }
            } else {
                println!("No path found between #{from_id} and #{to_id} within 10 hops.");
            }
        }
        _ => {
            println!("Usage: .graph [nodes | edges | path <from> <to>]");
        }
    }
}

fn execute_statement(conn: &Connection, sql: &str) -> bool {
    let start = Instant::now();
    let upper = sql.trim_start().to_uppercase();
    let is_query = upper.starts_with("SELECT") || upper.starts_with("EXPLAIN");

    if is_query {
        match conn.query(sql) {
            Ok(rows) => {
                let elapsed = start.elapsed();
                render_ascii_table(&rows);
                println!("({} row(s) in {:?})\n", rows.len(), elapsed);
                true
            }
            Err(e) => {
                println!("Query Error: {e}\n");
                false
            }
        }
    } else {
        match conn.execute(sql) {
            Ok(affected) => {
                let elapsed = start.elapsed();
                println!("Query OK, {affected} row(s) affected in {:?}\n", elapsed);
                true
            }
            Err(e) => {
                println!("Execution Error: {e}\n");
                false
            }
        }
    }
}

fn render_ascii_table(rows: &[Row]) {
    if rows.is_empty() {
        println!("(Empty set)");
        return;
    }

    let columns = rows[0].columns();
    let mut widths: Vec<usize> = columns.iter().map(|c| c.len()).collect();

    for row in rows {
        for (i, val) in row.values().iter().enumerate() {
            let str_val = format!("{val}");
            if str_val.len() > widths[i] {
                widths[i] = str_val.len().min(50); // Cap column width at 50 chars
            }
        }
    }

    // Border: +----+------+
    let border: String = widths
        .iter()
        .map(|w| format!("+-{}-", "-".repeat(*w)))
        .collect::<Vec<String>>()
        .join("")
        + "+";

    println!("{border}");

    // Header: | col1 | col2 |
    let header: String = columns
        .iter()
        .enumerate()
        .map(|(i, c)| format!("| {:<width$} ", c, width = widths[i]))
        .collect::<Vec<String>>()
        .join("")
        + "|";

    println!("{header}");
    println!("{border}");

    // Data rows
    for row in rows {
        let row_str: String = row
            .values()
            .iter()
            .enumerate()
            .map(|(i, val)| {
                let mut s = format!("{val}");
                if s.len() > widths[i] {
                    s.truncate(widths[i] - 3);
                    s.push_str("...");
                }
                format!("| {:<width$} ", s, width = widths[i])
            })
            .collect::<Vec<String>>()
            .join("")
            + "|";
        println!("{row_str}");
    }

    println!("{border}");
}

fn print_table_ddl(table: &tapirus::sql::catalog::TableDef) {
    println!("CREATE TABLE {} (", table.name);
    for (i, col) in table.columns.iter().enumerate() {
        let mut def = format!("  {} {}", col.name, format_data_type(&col.data_type));
        if col.primary_key {
            def.push_str(" PRIMARY KEY");
        }
        if col.not_null {
            def.push_str(" NOT NULL");
        }
        if i < table.columns.len() - 1 {
            def.push(',');
        }
        println!("{def}");
    }
    println!(");");
}

fn format_data_type(dt: &DataType) -> String {
    match dt {
        DataType::Integer => "INTEGER".to_string(),
        DataType::Real => "REAL".to_string(),
        DataType::Text => "TEXT".to_string(),
        DataType::Blob => "BLOB".to_string(),
        DataType::Vector(dims) => format!("VECTOR({dims})"),
    }
}

fn print_dot_help() {
    println!("TapirusDB REPL Dot-Commands:");
    println!("  .help                     Show this help message");
    println!("  .tables                   List all relational tables");
    println!("  .schema [table]           Show CREATE TABLE statement for table(s)");
    println!("  .collections              List all document collections");
    println!("  .doc <col> find           Show all documents in a collection");
    println!("  .doc <col> insert <json>  Insert a JSON document into a collection");
    println!("  .dump [table]             Dump database or table as SQL statements");
    println!("  .export <name> <file.json> Export table or collection to JSON");
    println!("  .import <file.csv> <table> Import CSV records into a table");
    println!("  .backup <file.tapir>      Hot online snapshot backup to destination file");
    println!("  .vacuum [into <file>]     Vacuum database in place or into target file");
    println!("  .memory remember <text>   Store a memory in AI Agent Memory");
    println!("  .memory recall <query>    Recall memories using hybrid BM25 + temporal decay");
    println!("  .memory count             Show total stored memories");
    println!("  .graph                    Display graph topology statistics");
    println!("  .graph nodes              List nodes in the knowledge graph");
    println!("  .graph edges              List edges in the knowledge graph");
    println!("  .graph path <src> <dst>   Find BFS shortest path between two nodes");
    println!("  .checkpoint               Flush Write-Ahead Log (WAL) to database file");
    println!("  .info                     Display database metadata and connection status");
    println!("  .exit, .quit, .q          Exit this shell");
    println!();
    println!("SQL Syntax Examples:");
    println!("  CREATE TABLE items (id INTEGER PRIMARY KEY, title TEXT, embedding VECTOR(4));");
    println!(
        "  INSERT INTO items (id, title, embedding) VALUES (1, 'Rover', [0.1, 0.2, 0.3, 0.4]);"
    );
    println!("  SELECT id, title FROM items WHERE id > 0 AND title LIKE '%Rover%';");
    println!("  SELECT id, title FROM items WHERE title MATCH 'Rover AI';");
    println!("  EXPLAIN QUERY PLAN SELECT * FROM items WHERE id = 1;");
    println!("  SELECT id, title FROM items VECTOR NEAR embedding = [0.1, 0.2, 0.3, 0.4] TOP 5;");
    println!("  VACUUM INTO 'backup.tapir';");
}

fn handle_dump_command(conn: &Connection, parts: &[&str]) {
    let target_tables = if parts.len() > 1 {
        let tname = parts[1];
        match conn.table(tname) {
            Some(t) => vec![t],
            None => {
                println!("Error: Table '{tname}' not found");
                return;
            }
        }
    } else {
        conn.tables()
    };

    println!("-- TapirusDB Database Dump");
    println!("BEGIN TRANSACTION;");

    for t in target_tables {
        print_table_ddl(&t);
        println!(";");

        let sql = format!("SELECT * FROM {};", t.name);
        if let Ok(rows) = conn.query(&sql) {
            let col_names = t.column_names();
            for row in rows {
                let vals: Vec<String> = row
                    .values()
                    .iter()
                    .map(|v| match v {
                        Value::Null => "NULL".to_string(),
                        Value::Integer(i) => i.to_string(),
                        Value::Real(r) => r.to_string(),
                        Value::Text(s) => format!("'{}'", s.replace('\'', "''")),
                        Value::Blob(b) => {
                            let hex_str: String =
                                b.iter().map(|byte| format!("{:02X}", byte)).collect();
                            format!("X'{hex_str}'")
                        }
                        Value::Vector(v) => format!(
                            "[{}]",
                            v.iter()
                                .map(|f| f.to_string())
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                    })
                    .collect();
                println!(
                    "INSERT INTO {} ({}) VALUES ({});",
                    t.name,
                    col_names.join(", "),
                    vals.join(", ")
                );
            }
        }
        println!();
    }
    println!("COMMIT;");
}

fn handle_export_command(conn: &Connection, target_name: &str, output_path: &str) {
    let start = Instant::now();
    // 1. Check if it's a relational table
    if let Some(table) = conn.table(target_name) {
        let sql = format!("SELECT * FROM {};", table.name);
        match conn.query(&sql) {
            Ok(rows) => {
                let json_rows: Vec<serde_json::Value> = rows
                    .iter()
                    .map(|r| {
                        let mut map = serde_json::Map::new();
                        for (col, val) in r.columns().iter().zip(r.values().iter()) {
                            map.insert(col.clone(), value_to_json(val));
                        }
                        serde_json::Value::Object(map)
                    })
                    .collect();
                let json_str = serde_json::to_string_pretty(&json_rows).unwrap_or_default();
                if let Err(e) = std::fs::write(output_path, json_str) {
                    println!("Export error: Failed to write to '{output_path}': {e}");
                } else {
                    println!(
                        "Exported {} rows from table '{}' to '{}' in {:?}",
                        rows.len(),
                        target_name,
                        output_path,
                        start.elapsed()
                    );
                }
            }
            Err(e) => println!("Export query error: {e}"),
        }
        return;
    }

    // 2. Check if it's a document collection
    if let Ok(col) = conn.collection(target_name) {
        match col.find_all() {
            Ok(docs) => {
                let json_docs: Vec<serde_json::Value> = docs.into_iter().map(|(_, d)| d).collect();
                let json_str = serde_json::to_string_pretty(&json_docs).unwrap_or_default();
                if let Err(e) = std::fs::write(output_path, json_str) {
                    println!("Export error: Failed to write to '{output_path}': {e}");
                } else {
                    println!(
                        "Exported {} documents from collection '{}' to '{}' in {:?}",
                        json_docs.len(),
                        target_name,
                        output_path,
                        start.elapsed()
                    );
                }
            }
            Err(e) => println!("Export collection error: {e}"),
        }
        return;
    }

    println!("Error: Target '{target_name}' is neither a table nor a document collection");
}

fn parse_csv_line(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut bracket_depth = 0usize;

    let chars: Vec<char> = line.chars().collect();
    let mut idx = 0;
    while idx < chars.len() {
        let ch = chars[idx];
        match ch {
            '"' => {
                if in_quotes && idx + 1 < chars.len() && chars[idx + 1] == '"' {
                    current.push('"');
                    idx += 1; // Skip escaped quote
                } else {
                    in_quotes = !in_quotes;
                }
            }
            '[' if !in_quotes => {
                bracket_depth += 1;
                current.push(ch);
            }
            ']' if !in_quotes => {
                bracket_depth = bracket_depth.saturating_sub(1);
                current.push(ch);
            }
            ',' if !in_quotes && bracket_depth == 0 => {
                fields.push(current.trim().to_string());
                current.clear();
            }
            _ => {
                current.push(ch);
            }
        }
        idx += 1;
    }
    fields.push(current.trim().to_string());
    fields
}

fn format_csv_sql_value(v: &str) -> String {
    let trimmed = v.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("null") {
        "NULL".to_string()
    } else if let Ok(i) = trimmed.parse::<i64>() {
        i.to_string()
    } else if let Ok(f) = trimmed.parse::<f64>() {
        f.to_string()
    } else if trimmed.starts_with('[') && trimmed.ends_with(']') {
        trimmed.to_string()
    } else {
        format!("'{}'", trimmed.replace('\'', "''"))
    }
}

fn import_csv_file(conn: &Connection, csv_path: &str, table_name: &str, batch_size: usize) {
    let start = Instant::now();
    let file = match std::fs::File::open(csv_path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("Import Error: Could not open CSV file '{csv_path}': {e}");
            return;
        }
    };

    let reader = io::BufReader::new(file);
    let mut lines_iter = reader.lines();

    let header_line = match lines_iter.next() {
        Some(Ok(l)) if !l.trim().is_empty() => l,
        _ => {
            eprintln!("Import Error: CSV file '{csv_path}' is empty or missing headers.");
            return;
        }
    };

    let headers = parse_csv_line(&header_line);
    if headers.is_empty() {
        eprintln!("Import Error: No columns found in CSV header.");
        return;
    }

    let clean_headers: Vec<String> = headers
        .iter()
        .map(|h| {
            let trimmed = h.trim();
            if trimmed.is_empty() {
                "col".to_string()
            } else {
                trimmed
                    .chars()
                    .map(|c| {
                        if c.is_alphanumeric() || c == '_' {
                            c
                        } else {
                            '_'
                        }
                    })
                    .collect()
            }
        })
        .collect();

    // Check if table already exists in database
    if conn.table(table_name).is_none() {
        // Collect first batch of lines for type inference and processing
        let mut sample_rows = Vec::new();
        let mut row_buffer = Vec::new();

        for line in lines_iter.by_ref() {
            let line = match line {
                Ok(line) => line,
                Err(error) => {
                    eprintln!("CSV read failed: {error}");
                    return;
                }
            };
            if !line.trim().is_empty() {
                let fields = parse_csv_line(&line);
                if fields.len() == clean_headers.len() {
                    if sample_rows.len() < 100 {
                        sample_rows.push(fields.clone());
                    }
                    row_buffer.push(fields);
                    if row_buffer.len() >= batch_size {
                        break;
                    }
                }
            }
        }

        // Infer column data types
        let mut col_types = Vec::new();
        for col_idx in 0..clean_headers.len() {
            let mut all_int = true;
            let mut all_float = true;
            let mut vector_dim: Option<usize> = None;
            let mut has_non_empty = false;

            for row in &sample_rows {
                if let Some(val) = row.get(col_idx) {
                    let val = val.trim();
                    if val.is_empty() || val.eq_ignore_ascii_case("null") {
                        continue;
                    }
                    has_non_empty = true;

                    if val.starts_with('[') && val.ends_with(']') {
                        let inner = &val[1..val.len() - 1];
                        let parts: Vec<&str> = inner.split(',').collect();
                        if !parts.is_empty()
                            && parts.iter().all(|p| p.trim().parse::<f32>().is_ok())
                        {
                            vector_dim = Some(parts.len());
                            all_int = false;
                            all_float = false;
                            continue;
                        }
                    }

                    if val.parse::<i64>().is_err() {
                        all_int = false;
                    }
                    if val.parse::<f64>().is_err() {
                        all_float = false;
                    }
                }
            }

            let dtype = if let Some(dims) = vector_dim {
                format!("VECTOR({dims})")
            } else if has_non_empty && all_int {
                "INTEGER".to_string()
            } else if has_non_empty && all_float {
                "REAL".to_string()
            } else {
                "TEXT".to_string()
            };

            col_types.push(dtype);
        }

        let col_defs: Vec<String> = clean_headers
            .iter()
            .zip(col_types.iter())
            .map(|(name, dtype)| format!("{name} {dtype}"))
            .collect();

        let create_sql = format!(
            "CREATE TABLE IF NOT EXISTS {} ({});",
            table_name,
            col_defs.join(", ")
        );

        if let Err(e) = conn.execute(&create_sql) {
            eprintln!("Import Error: Failed to auto-create table '{table_name}': {e}");
            return;
        }

        println!(
            "  ✓ Schema auto-inferred and table '{}' initialized.",
            table_name
        );

        let header_list = clean_headers.join(", ");
        let mut total_imported = 0usize;

        let _ = conn.begin_transaction();
        for row in row_buffer {
            let formatted_vals: Vec<String> = row.iter().map(|v| format_csv_sql_value(v)).collect();

            let sql = format!(
                "INSERT INTO {} ({}) VALUES ({});",
                table_name,
                header_list,
                formatted_vals.join(", ")
            );
            if conn.execute(&sql).is_ok() {
                total_imported += 1;
            }
        }
        let _ = conn.commit();

        let _ = conn.begin_transaction();
        let mut batch_count = 0usize;

        for line in lines_iter {
            let line = match line {
                Ok(line) => line,
                Err(error) => {
                    let _ = conn.rollback();
                    eprintln!("CSV read failed: {error}");
                    return;
                }
            };
            if !line.trim().is_empty() {
                let fields = parse_csv_line(&line);
                if fields.len() == clean_headers.len() {
                    let formatted_vals: Vec<String> =
                        fields.iter().map(|v| format_csv_sql_value(v)).collect();

                    let sql = format!(
                        "INSERT INTO {} ({}) VALUES ({});",
                        table_name,
                        header_list,
                        formatted_vals.join(", ")
                    );
                    if conn.execute(&sql).is_ok() {
                        total_imported += 1;
                        batch_count += 1;
                        if batch_count >= batch_size {
                            let _ = conn.commit();
                            let _ = conn.begin_transaction();
                            batch_count = 0;
                        }
                    }
                }
            }
        }
        let _ = conn.commit();

        let elapsed = start.elapsed();
        let rate = if elapsed.as_secs_f64() > 0.0 {
            total_imported as f64 / elapsed.as_secs_f64()
        } else {
            total_imported as f64
        };

        println!(
            "\x1b[1;32m✓\x1b[0m Successfully imported \x1b[1m{}\x1b[0m rows into table '\x1b[36m{}\x1b[0m' in {:.2?} ({:.0} rows/sec)",
            total_imported, table_name, elapsed, rate
        );
        return;
    }

    // Table already exists: Stream directly
    let header_list = clean_headers.join(", ");
    let mut total_imported = 0usize;
    let mut batch_count = 0usize;

    let _ = conn.begin_transaction();
    for line in lines_iter {
        let line = match line {
            Ok(line) => line,
            Err(error) => {
                let _ = conn.rollback();
                eprintln!("CSV read failed: {error}");
                return;
            }
        };
        if !line.trim().is_empty() {
            let fields = parse_csv_line(&line);
            if fields.len() == clean_headers.len() {
                let formatted_vals: Vec<String> =
                    fields.iter().map(|v| format_csv_sql_value(v)).collect();

                let sql = format!(
                    "INSERT INTO {} ({}) VALUES ({});",
                    table_name,
                    header_list,
                    formatted_vals.join(", ")
                );
                if conn.execute(&sql).is_ok() {
                    total_imported += 1;
                    batch_count += 1;
                    if batch_count >= batch_size {
                        let _ = conn.commit();
                        let _ = conn.begin_transaction();
                        batch_count = 0;
                    }
                }
            }
        }
    }
    let _ = conn.commit();

    let elapsed = start.elapsed();
    let rate = if elapsed.as_secs_f64() > 0.0 {
        total_imported as f64 / elapsed.as_secs_f64()
    } else {
        total_imported as f64
    };

    println!(
        "\x1b[1;32m✓\x1b[0m Successfully imported \x1b[1m{}\x1b[0m rows into table '\x1b[36m{}\x1b[0m' in {:.2?} ({:.0} rows/sec)",
        total_imported, table_name, elapsed, rate
    );
}

fn import_jsonl_file(conn: &Connection, json_path: &str, collection_name: &str, batch_size: usize) {
    let start = Instant::now();
    let col = match conn.collection(collection_name) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Import Error: Could not access collection '{collection_name}': {e}");
            return;
        }
    };

    let content = match std::fs::read_to_string(json_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Import Error: Could not read JSON file '{json_path}': {e}");
            return;
        }
    };

    let mut total_imported = 0usize;
    let initial_max_id = col
        .find_all()
        .map(|v| v.iter().map(|(id, _)| *id).max().unwrap_or(0))
        .unwrap_or(0);
    let mut next_id = initial_max_id + 1;

    // Check if whole file is a single JSON array
    let trimmed = content.trim();
    if trimmed.starts_with('[') {
        if let Ok(serde_json::Value::Array(arr)) = serde_json::from_str(trimmed) {
            let _ = conn.begin_transaction();
            let mut batch_count = 0;
            for doc in arr {
                if col.insert_with_id(next_id, &doc).is_ok() {
                    next_id += 1;
                    total_imported += 1;
                    batch_count += 1;
                    if batch_count >= batch_size {
                        let _ = conn.commit();
                        let _ = conn.begin_transaction();
                        batch_count = 0;
                    }
                }
            }
            let _ = conn.commit();

            let elapsed = start.elapsed();
            let rate = if elapsed.as_secs_f64() > 0.0 {
                total_imported as f64 / elapsed.as_secs_f64()
            } else {
                total_imported as f64
            };
            println!(
                "\x1b[1;32m✓\x1b[0m Successfully imported \x1b[1m{}\x1b[0m documents into collection '\x1b[36m{}\x1b[0m' in {:.2?} ({:.0} docs/sec)",
                total_imported, collection_name, elapsed, rate
            );
            return;
        }
    }

    // Line-delimited JSON (JSONL)
    let _ = conn.begin_transaction();
    let mut batch_count = 0;

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<serde_json::Value>(line) {
            Ok(doc) => match col.insert_with_id(next_id, &doc) {
                Ok(_) => {
                    next_id += 1;
                    total_imported += 1;
                    batch_count += 1;
                    if batch_count >= batch_size {
                        let _ = conn.commit();
                        let _ = conn.begin_transaction();
                        batch_count = 0;
                    }
                }
                Err(e) => eprintln!("Insert error: {e}"),
            },
            Err(e) => eprintln!("JSON parse error: {e}"),
        }
    }
    let _ = conn.commit();

    let elapsed = start.elapsed();
    let rate = if elapsed.as_secs_f64() > 0.0 {
        total_imported as f64 / elapsed.as_secs_f64()
    } else {
        total_imported as f64
    };

    println!(
        "\x1b[1;32m✓\x1b[0m Successfully imported \x1b[1m{}\x1b[0m documents into collection '\x1b[36m{}\x1b[0m' in {:.2?} ({:.0} docs/sec)",
        total_imported, collection_name, elapsed, rate
    );
}

fn import_markdown_file(
    conn: &Connection,
    md_path: &str,
    namespace: &str,
    session_id: Option<&str>,
    tags: &[String],
) {
    let start = Instant::now();
    let content = match std::fs::read_to_string(md_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Import Error: Could not read Markdown file '{md_path}': {e}");
            return;
        }
    };

    let p = Path::new(md_path);
    let filename = p
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("document.md");

    // Chunk markdown by headings
    let mut sections: Vec<(String, String)> = Vec::new();
    let mut current_heading = filename.to_string();
    let mut current_body = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            let heading_text = trimmed.trim_start_matches('#').trim().to_string();
            let body = current_body.join("\n").trim().to_string();
            if !body.is_empty() || sections.is_empty() {
                if !body.is_empty() {
                    sections.push((current_heading, body));
                }
                current_heading = heading_text;
                current_body.clear();
            } else {
                current_heading = heading_text;
            }
        } else {
            current_body.push(line);
        }
    }
    let final_body = current_body.join("\n").trim().to_string();
    if !final_body.is_empty() {
        sections.push((current_heading, final_body));
    }

    if sections.is_empty() {
        println!("Import warning: Markdown file '{md_path}' has no text content.");
        return;
    }

    let embedder = tapirus::DeterministicHashEmbedder::default();
    let tag_refs: Vec<&str> = tags.iter().map(|s| s.as_str()).collect();

    let max_existing_node_id = conn
        .graph_nodes()
        .into_iter()
        .map(|n| n.id)
        .max()
        .unwrap_or(0);
    let mut next_node_id = max_existing_node_id + 1;

    // Create Root Document Node in Knowledge Graph
    let doc_node_id = next_node_id;
    next_node_id += 1;

    let doc_props = serde_json::json!({
        "filename": filename,
        "path": md_path,
        "sections_count": sections.len(),
        "total_chars": content.len()
    });
    let _ = conn.graph_add_node(doc_node_id, "Document", &doc_props.to_string());

    let mut memories_stored = 0usize;
    let mut sections_stored = 0usize;
    let mut edges_created = 0usize;
    let mut prev_section_node_id: Option<u64> = None;

    for (heading, body) in &sections {
        let chunk_text = format!("## {heading}\n{body}");

        if conn
            .memory_remember_text_scoped(&chunk_text, 0.7, &tag_refs, Some(namespace), session_id)
            .is_ok()
        {
            memories_stored += 1;
        }

        let sec_node_id = next_node_id;
        next_node_id += 1;

        let sec_vector = embedder.embed_text(&chunk_text);
        let sec_props = serde_json::json!({
            "title": heading,
            "char_count": chunk_text.len(),
            "namespace": namespace,
            "document": filename
        });

        if conn
            .graph_add_node_with_vector(
                sec_node_id,
                "Section",
                &sec_props.to_string(),
                Some(&sec_vector),
            )
            .is_ok()
        {
            sections_stored += 1;

            if conn
                .graph_add_edge(doc_node_id, sec_node_id, "CONTAINS", 1.0, "{}")
                .is_ok()
            {
                edges_created += 1;
            }

            if let Some(prev_id) = prev_section_node_id {
                if conn
                    .graph_add_edge(prev_id, sec_node_id, "PRECEDES", 1.0, "{}")
                    .is_ok()
                {
                    edges_created += 1;
                }
            }
            prev_section_node_id = Some(sec_node_id);
        }
    }

    let elapsed = start.elapsed();
    println!(
        "\x1b[1;32m✓\x1b[0m Successfully imported Markdown '\x1b[36m{}\x1b[0m' into TapirusDB in {:.2?}:",
        filename, elapsed
    );
    println!(
        "  • AI Agent Memories:   \x1b[1m{}\x1b[0m chunks (Namespace: '\x1b[33m{}\x1b[0m', 128D Embeddings)",
        memories_stored, namespace
    );
    println!(
        "  • Knowledge Graph:     \x1b[1m{}\x1b[0m Nodes (1 Document + {} Sections)",
        sections_stored + 1,
        sections_stored
    );
    println!(
        "  • Graph Relationships: \x1b[1m{}\x1b[0m Edges (CONTAINS, PRECEDES)",
        edges_created
    );
    println!("  ? GraphRAG Status:     Records imported; retrieval quality not measured");
}

fn print_import_help() {
    println!("Usage: tapirus import <FORMAT> <FILE> [OPTIONS]");
    println!("       tapirus import <FILE> [OPTIONS]");
    println!();
    println!("High-throughput streaming data importer for TapirusDB.");
    println!("Supports Tabular Relational CSV, JSON / JSONL Document collections,");
    println!("and Markdown AI Agent Memory chunking with GraphRAG entity linking.");
    println!();
    println!("Formats:");
    println!("  csv                      Import CSV records into a relational SQL table");
    println!("  jsonl, json              Import JSON/JSONL documents into a Document collection");
    println!("  markdown, md             Chunk Markdown into AI Agent Memory & Knowledge Graph");
    println!();
    println!("Common Options:");
    println!("  --db <PATH>              Path to target database file (default: production.tapir)");
    println!("  --passphrase <KEY>       Passphrase for encrypted database (ChaCha20-Poly1305)");
    println!("  --batch <N>              Batch size for transaction commits (default: 500)");
    println!("  -h, --help               Print this help message");
    println!();
    println!("CSV Specific Options:");
    println!("  --table <NAME>           Target table name (defaults to file name stem)");
    println!();
    println!("JSON / JSONL Specific Options:");
    println!(
        "  --collection <NAME>      Target document collection name (defaults to file name stem)"
    );
    println!();
    println!("Markdown Specific Options:");
    println!("  --namespace <NAME>       Target memory namespace (defaults to file name stem)");
    println!("  --session-id <ID>        Optional session ID for agent memory");
    println!("  --tags <TAG1,TAG2,...>   Comma-separated tags for memory recall indexing");
    println!();
    println!("Examples:");
    println!("  tapirus import csv users.csv --table users --db app.tapir");
    println!("  tapirus import jsonl events.jsonl --collection events --db app.tapir");
    println!("  tapirus import md handbook.md --namespace docs --tags company,handbook");
}

fn run_import_command(args: &[String]) {
    if args.is_empty() || args[0] == "-h" || args[0] == "--help" {
        print_import_help();
        return;
    }

    let mut format: Option<String> = None;
    let mut file_path: Option<String> = None;
    let mut db_path = "production.tapir".to_string();
    let mut table_name: Option<String> = None;
    let mut collection_name: Option<String> = None;
    let mut namespace: Option<String> = None;
    let mut session_id: Option<String> = None;
    let mut tags = Vec::new();
    let mut passphrase: Option<String> = None;
    let mut batch_size = 500usize;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" => {
                print_import_help();
                return;
            }
            "--db" => {
                if i + 1 < args.len() {
                    db_path = args[i + 1].clone();
                    i += 1;
                }
            }
            "--table" => {
                if i + 1 < args.len() {
                    table_name = Some(args[i + 1].clone());
                    i += 1;
                }
            }
            "--collection" => {
                if i + 1 < args.len() {
                    collection_name = Some(args[i + 1].clone());
                    i += 1;
                }
            }
            "--namespace" => {
                if i + 1 < args.len() {
                    namespace = Some(args[i + 1].clone());
                    i += 1;
                }
            }
            "--session-id" => {
                if i + 1 < args.len() {
                    session_id = Some(args[i + 1].clone());
                    i += 1;
                }
            }
            "--tags" => {
                if i + 1 < args.len() {
                    tags = args[i + 1]
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                    i += 1;
                }
            }
            "--passphrase" => {
                if i + 1 < args.len() {
                    passphrase = Some(args[i + 1].clone());
                    i += 1;
                }
            }
            "--batch" | "--batch-size" => {
                if i + 1 < args.len() {
                    batch_size = args[i + 1].parse().unwrap_or(500);
                    i += 1;
                }
            }
            arg if !arg.starts_with('-') => {
                if format.is_none()
                    && (arg == "csv"
                        || arg == "jsonl"
                        || arg == "json"
                        || arg == "markdown"
                        || arg == "md")
                {
                    format = Some(arg.to_string());
                } else if file_path.is_none() {
                    file_path = Some(arg.to_string());
                }
            }
            _ => {}
        }
        i += 1;
    }

    let file_path = match file_path {
        Some(f) => f,
        None => {
            eprintln!("Error: Target file to import is required.");
            println!();
            print_import_help();
            std::process::exit(1);
        }
    };

    let p = Path::new(&file_path);
    if !p.exists() {
        eprintln!("Error: File '{file_path}' does not exist.");
        std::process::exit(1);
    }

    let inferred_format = format.unwrap_or_else(|| {
        let ext = p
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();
        match ext.as_str() {
            "csv" => "csv".to_string(),
            "jsonl" => "jsonl".to_string(),
            "json" => "json".to_string(),
            "md" | "markdown" => "markdown".to_string(),
            _ => "csv".to_string(),
        }
    });

    let conn = if let Some(ref pass) = passphrase {
        match Connection::open_encrypted(Path::new(&db_path), pass) {
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

    let stem = p
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("data")
        .to_string();

    match inferred_format.as_str() {
        "csv" => {
            let table = table_name.unwrap_or(stem);
            import_csv_file(&conn, &file_path, &table, batch_size);
        }
        "jsonl" | "json" => {
            let col = collection_name.unwrap_or(stem);
            import_jsonl_file(&conn, &file_path, &col, batch_size);
        }
        "markdown" | "md" => {
            let ns = namespace.unwrap_or(stem);
            import_markdown_file(&conn, &file_path, &ns, session_id.as_deref(), &tags);
        }
        other => {
            eprintln!(
                "Error: Unsupported import format '{other}'. Choose from: csv, jsonl, json, markdown."
            );
            std::process::exit(1);
        }
    }
}

fn handle_import_csv(conn: &Connection, csv_path: &str, table_name: &str) {
    import_csv_file(conn, csv_path, table_name, 500);
}

fn handle_memory_command(conn: &Connection, parts: &[&str]) {
    if parts.len() < 2 {
        println!("Usage: .memory <count|remember <text>|recall <query>>");
        return;
    }

    match parts[1] {
        "count" => {
            println!("Stored AI Memories: {}", conn.memory_count());
        }
        "remember" => {
            if parts.len() < 3 {
                println!("Usage: .memory remember <content text>");
                return;
            }
            let content = parts[2..].join(" ");
            match conn.memory_remember(&content, None, 0.5, &[]) {
                Ok(id) => println!("Remembered as Memory #{id}"),
                Err(e) => println!("Error storing memory: {e}"),
            }
        }
        "recall" => {
            if parts.len() < 3 {
                println!("Usage: .memory recall <query text>");
                return;
            }
            let query = parts[2..].join(" ");
            let filter = tapirus::MemoryRecallFilter::default();
            let results = conn.memory_recall(Some(&query), None, 5, &filter);
            if results.is_empty() {
                println!("No matching memories found for '{query}'");
            } else {
                println!("Recalled Memories ({}):", results.len());
                for (idx, r) in results.iter().enumerate() {
                    println!(
                        "  [{}] #{} (Score: {:.3}, Lex: {:.2}, Rec: {:.2}) - {}",
                        idx + 1,
                        r.entry.id,
                        r.combined_score,
                        r.lexical_score,
                        r.recency_score,
                        r.entry.content
                    );
                }
            }
        }
        _ => {
            println!(
                "Unknown memory command: '{}'. Use .memory <count|remember|recall>",
                parts[1]
            );
        }
    }
}

#[derive(Clone, Debug, Default)]
struct McpServerConfig {
    embed_endpoint: Option<String>,
    embed_model: Option<String>,
    embed_dims: Option<usize>,
    embed_api_key: Option<String>,
}

fn print_mcp_help() {
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

fn print_claude_config(db_path: &str) {
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

fn run_mcp_command(args: &[String]) {
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

struct ResolvedMcpEmbedding {
    vector: Vec<f32>,
    model: String,
    mode: &'static str,
}

fn parse_embedding_array_arg(
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
    if let Some(dims) = expected_dims {
        if vec.len() != dims {
            return Err(format!(
                "embedding dimension mismatch: got {}, expected {}",
                vec.len(),
                dims
            ));
        }
    }
    Ok(vec)
}

fn decode_http_chunked_body(raw: &[u8]) -> std::result::Result<Vec<u8>, String> {
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

fn parse_http_endpoint_authority_and_path(
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

fn fetch_http_embedding_plain(
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
    if let Some(key) = api_key {
        if !key.trim().is_empty() {
            req.push_str(&format!("Authorization: Bearer {}\r\n", key.trim()));
        }
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
                if header_end_opt.is_none() {
                    if let Some(end) = find_subsequence(&raw_resp, b"\r\n\r\n") {
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
fn fetch_http_embedding(
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
fn fetch_http_embedding(
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

fn resolve_mcp_embedding(
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

    if let Some(raw_vec) = args.get("embedding") {
        if !raw_vec.is_null() {
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
fn handle_mcp_tool_call(conn: &Connection, tool_name: &str, args: &serde_json::Value) -> String {
    handle_mcp_tool_call_with_config(conn, tool_name, args, &McpServerConfig::default())
}

fn handle_mcp_tool_call_with_config(
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

fn run_grep_command(args: &[String]) {
    let mut pattern: Option<String> = None;
    let mut search_path = ".".to_string();
    let mut ignore_case = false;
    let mut max_count = 25usize;
    let mut extensions: Option<Vec<String>> = None;
    let mut use_vector = false;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" => {
                println!("Tapirus Grep (tg) — Accelerated Hybrid Workspace Code Search");
                println!();
                println!("Usage: tapirus grep [OPTIONS] <PATTERN> [PATH]");
                println!("Alias: tapirus tg [OPTIONS] <PATTERN> [PATH]");
                println!();
                println!("Options:");
                println!("  -i, --ignore-case       Case-insensitive matching");
                println!("  -m, --max <N>           Maximum matching lines to show (default: 25)");
                println!("  --ext <EXTS>            Comma-separated extensions (e.g. rs,py,md)");
                println!("  --vector                Enable local semantic vector ranking");
                println!("  -h, --help              Show this help message");
                return;
            }
            "-i" | "--ignore-case" => {
                ignore_case = true;
                i += 1;
            }
            "-m" | "--max" => {
                if i + 1 < args.len() {
                    max_count = args[i + 1].parse().unwrap_or(25);
                    i += 2;
                } else {
                    i += 1;
                }
            }
            "--ext" => {
                if i + 1 < args.len() {
                    extensions = Some(
                        args[i + 1]
                            .split(',')
                            .map(|s| s.trim().to_lowercase())
                            .collect(),
                    );
                    i += 2;
                } else {
                    i += 1;
                }
            }
            "--vector" => {
                use_vector = true;
                i += 1;
            }
            other if !other.starts_with('-') => {
                if pattern.is_none() {
                    pattern = Some(other.to_string());
                } else {
                    search_path = other.to_string();
                }
                i += 1;
            }
            _ => {
                i += 1;
            }
        }
    }

    let pattern = match pattern {
        Some(p) if !p.is_empty() => p,
        _ => {
            eprintln!("Error: Search pattern is required.");
            eprintln!("Usage: tapirus grep [OPTIONS] <PATTERN> [PATH]");
            return;
        }
    };

    println!(
        "\x1b[1;36m🔍 Tapirus Grep (tg)\x1b[0m — Searching for '\x1b[1;33m{pattern}\x1b[0m' in '{search_path}'..."
    );
    let start_time = Instant::now();

    let embedder = if use_vector {
        Some(tapirus::memory::embedder::DeterministicHashEmbedder::new(
            64,
        ))
    } else {
        None
    };

    let query_vector = embedder.as_ref().map(|e| e.embed_text(&pattern));
    let pattern_lower = pattern.to_lowercase();
    let query_tokens: Vec<String> = pattern_lower
        .split_whitespace()
        .map(|s| s.to_string())
        .collect();

    let mut matches = Vec::new();

    struct SearchPattern<'a> {
        raw: &'a str,
        lowercase: &'a str,
        tokens: &'a [String],
    }

    // Recursive directory walk
    fn walk_dir(
        dir: &Path,
        query: &SearchPattern<'_>,
        ignore_case: bool,
        embedder: Option<&tapirus::memory::embedder::DeterministicHashEmbedder>,
        query_vector: Option<&[f32]>,
        extensions: Option<&[String]>,
        matches: &mut Vec<(f32, String, usize, String)>,
    ) {
        let pattern = query.raw;
        let pattern_lower = query.lowercase;
        let query_tokens = query.tokens;
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with('.')
                    || name == "target"
                    || name == "node_modules"
                    || name == "dist"
                    || name == "__pycache__"
                {
                    continue;
                }
            }

            if path.is_dir() {
                walk_dir(
                    &path,
                    query,
                    ignore_case,
                    embedder,
                    query_vector,
                    extensions,
                    matches,
                );
            } else if path.is_file() {
                // Check extension
                if let Some(exts) = extensions {
                    let ext = path
                        .extension()
                        .and_then(|e| e.to_str())
                        .unwrap_or("")
                        .to_lowercase();
                    if !exts.contains(&ext) {
                        continue;
                    }
                }

                // Read file
                if let Ok(file) = std::fs::File::open(&path) {
                    let reader = io::BufReader::new(file);
                    for (line_idx, line_res) in reader.lines().enumerate() {
                        let line = match line_res {
                            Ok(l) => l,
                            Err(_) => break, // non-utf8/binary file
                        };

                        let line_lower = if ignore_case {
                            line.to_lowercase()
                        } else {
                            line.clone()
                        };
                        let has_substring = if ignore_case {
                            line_lower.contains(pattern_lower)
                        } else {
                            line.contains(pattern)
                        };

                        let mut token_score = 0.0f32;
                        for tok in query_tokens {
                            if line_lower.contains(tok) {
                                token_score += 1.0;
                            }
                        }

                        let mut vector_sim = 0.0f32;
                        if let (Some(emb), Some(qv)) = (embedder, query_vector) {
                            if line.trim().len() > 3 {
                                let lv = emb.embed_text(&line);
                                let dist = tapirus::vector::simd::simd_cosine_distance(qv, &lv);
                                vector_sim = (1.0 - dist).max(0.0);
                            }
                        }

                        if has_substring || token_score > 0.0 || vector_sim > 0.75 {
                            let mut score = 0.0f32;
                            if has_substring {
                                score += 10.0;
                            }
                            score += token_score * 2.0;
                            score += vector_sim * 5.0;

                            matches.push((score, path.display().to_string(), line_idx + 1, line));
                        }
                    }
                }
            }
        }
    }

    walk_dir(
        Path::new(&search_path),
        &SearchPattern {
            raw: &pattern,
            lowercase: &pattern_lower,
            tokens: &query_tokens,
        },
        ignore_case,
        embedder.as_ref(),
        query_vector.as_deref(),
        extensions.as_deref(),
        &mut matches,
    );

    // Sort by score descending
    matches.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

    let total_matches = matches.len();
    let display_matches: Vec<_> = matches.into_iter().take(max_count).collect();

    for (score, path, line_no, content) in &display_matches {
        let trimmed = content.trim();
        println!(
            "\x1b[36m{}\x1b[0m:\x1b[32m{}\x1b[0m \x1b[90m[{:.2}]\x1b[0m {}",
            path, line_no, score, trimmed
        );
    }

    let elapsed = start_time.elapsed();
    println!();
    println!(
        "\x1b[1;32m✓\x1b[0m Showing {} of {} match(es) in {:.2?}",
        display_matches.len(),
        total_matches,
        elapsed
    );
}

fn run_bitnet_command(args: &[String]) {
    if args.is_empty() || args[0] == "-h" || args[0] == "--help" {
        println!("TapirusDB BitNet b1.58 ternary tensor prototype");
        println!();
        println!("Commands:");
        println!(
            "  tapirus bitnet <input> <cand1> <cand2> ...   Categorical classification via BitNet ternary layers"
        );
        println!(
            "  tapirus bitnet verify <premise> <hypothesis> Heuristic text comparison (not fact verification)"
        );
        println!(
            "  tapirus bitnet benchmark                      Run an illustrative local timing loop"
        );
        return;
    }

    let engine = tapirus::TapDeepEngine::new();

    if args[0] == "verify" {
        if args.len() < 3 {
            eprintln!("Usage: tapirus bitnet verify <premise> <hypothesis>");
            return;
        }
        let premise = &args[1];
        let hypothesis = &args[2];
        let start = web_time::Instant::now();
        match engine.verify_deep(premise, hypothesis) {
            Ok((verified, score)) => {
                let us = start.elapsed().as_micros();
                println!("┌─────────────────────────────────────────────────────────────┐");
                println!("BitNet text-match heuristic demo (not fact verification)");
                println!("├─────────────────────────────────────────────────────────────┤");
                println!("│ Premise:    {}", premise);
                println!("│ Hypothesis: {}", hypothesis);
                println!(
                    "Heuristic match: {} (score: {:.4}, not a probability)",
                    if verified {
                        "\x1b[1;32mMATCH\x1b[0m"
                    } else {
                        "\x1b[1;31mNO MATCH\x1b[0m"
                    },
                    score
                );
                println!(
                    "│ Latency:    {} µs (Multiplication-free ternary addition)    │",
                    us
                );
                println!("└─────────────────────────────────────────────────────────────┘");
            }
            Err(e) => eprintln!("Error: {e}"),
        }
        return;
    }

    if args[0] == "benchmark" {
        println!(
            "Benchmarking BitNet b1.58 ternary linear contraction (local exploratory timing)..."
        );
        let block = match tapirus::BitNetBlock::new(256, 512) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("Failed to initialize BitNet block: {e}");
                return;
            }
        };
        let input = vec![0.5f32; 256];
        let warmup = block.forward(std::hint::black_box(&input));
        let _ = std::hint::black_box(warmup);

        let iterations = if cfg!(debug_assertions) {
            1_000
        } else {
            10_000
        };
        let start = web_time::Instant::now();
        for _ in 0..iterations {
            let output = block.forward(std::hint::black_box(&input));
            let _ = std::hint::black_box(output);
        }
        let total_us = start.elapsed().as_micros();
        let per_op_ns = (total_us as f64 * 1000.0) / iterations as f64;
        let ops_per_sec = (iterations as f64 / (total_us.max(1) as f64 / 1_000_000.0)) as u64;

        println!(
            "BitNet b1.58 [256 -> 512 SwiGLU -> 256] local timing (not comparative evidence):"
        );
        println!("  Total Contractions: {}", iterations);
        println!(
            "  Latency per block:  {:.2} ns ({:.3} µs)",
            per_op_ns,
            per_op_ns / 1000.0
        );
        println!("  Throughput:         {} ternary blocks/sec", ops_per_sec);
        println!("  Multiply Ops:       0 (Zero floating-point weight multiplications)");
        return;
    }

    // Default: classify input against candidate labels
    let input = &args[0];
    let candidates: Vec<&str> = args[1..].iter().map(|s| s.as_str()).collect();
    if candidates.is_empty() {
        eprintln!("Please provide at least one candidate label to classify against.");
        return;
    }

    let start = web_time::Instant::now();
    match engine.classify_deep(input, &candidates) {
        Ok((winner, score)) => {
            let us = start.elapsed().as_micros();
            println!("┌─────────────────────────────────────────────────────────────┐");
            println!("│ BitNet b1.58 Ternary Categorical Classification             │");
            println!("├─────────────────────────────────────────────────────────────┤");
            println!("│ Input:      {}", input);
            println!(
                "│ Top Choice: \x1b[1;32m{}\x1b[0m (Score: {:.4})",
                winner, score
            );
            println!("│ Candidates: {:?}", candidates);
            println!(
                "│ Latency:    {} µs (local run; workload-specific)         │",
                us
            );
            println!("└─────────────────────────────────────────────────────────────┘");
        }
        Err(e) => eprintln!("Error: {e}"),
    }
}

#[cfg(test)]
mod audit_regression_tests {
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
        db.execute(
            "CREATE TABLE vectors (id INTEGER PRIMARY KEY, body TEXT, embedding VECTOR(2));",
        )?;
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

        let allowlist =
            parse_cors_origins("https://app.example.com/, http://localhost:5173").unwrap();
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
            memories.iter().all(|m| {
                m["namespace"] == "tenant_alpha" && m["content"] != "Secret beta memory"
            }),
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

        // 2. Read-only key cannot access non-allowlisted table directly, via JOIN, or via subquery
        assert!(
            send_sql("ro-public", "SELECT * FROM secrets;").starts_with("HTTP/1.1 403 Forbidden")
        );
        assert!(
            send_sql(
                "ro-public",
                "SELECT p.id FROM public_items p WHERE EXISTS (SELECT id FROM secrets);"
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
}
