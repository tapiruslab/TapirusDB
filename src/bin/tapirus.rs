//! # TapirusDB Interactive Terminal Shell & HTTP Server (`tapirus`)
//!
//! An interactive REPL and lightweight HTTP server for TapirusDB providing unified SQL,
//! Native AI Vector Search, MongoDB-style Documents, and Knowledge Graph inspection.

#![forbid(unsafe_code)]

use std::env;
use std::io::{self, BufRead, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::Arc;
use std::thread;
use std::time::Instant;
use parking_lot::Mutex;
use tapirus::sql::catalog::DataType;
use tapirus::{Connection, EmbeddingEngine, Row, Value};

const VERSION: &str = env!("CARGO_PKG_VERSION");

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
                        let json_rows: Vec<serde_json::Value> = rows.iter().map(|r| {
                            let mut map = serde_json::Map::new();
                            for (c, v) in r.columns().iter().zip(r.values().iter()) {
                                map.insert(c.clone(), serde_json::to_value(v).unwrap_or(serde_json::Value::Null));
                            }
                            serde_json::Value::Object(map)
                        }).collect();
                        println!("{}", serde_json::to_string(&json_rows).unwrap_or_else(|_| "[]".to_string()));
                    }
                    Err(e) => {
                        eprintln!("Error: {e}");
                        std::process::exit(1);
                    }
                }
            } else {
                execute_statement(&conn, &sql_cmd);
            }
            return;
        }
    }

    println!("{BANNER}");
    println!("TapirusDB v{VERSION} — The Safe-Rust Embedded Quad-Model AI Engine");
    println!("Connected to: {target} (Page Size: 4,096 B | 100% Safe Rust)");
    println!("Enter SQL, Vector, or dot-commands. End SQL statements with ';'.");
    println!("Type '.help' for instructions, '.exit' to quit.\n");

    run_repl(&conn, target);
}

fn print_help() {
    println!("Usage:");
    println!("  tapirus [OPTIONS] [DATABASE_FILE]        Launch interactive REPL");
    println!("  tapirus serve [OPTIONS] [DATABASE_FILE]  Launch high-performance HTTP REST server");
    println!("  tapirus mcp [OPTIONS] [DATABASE_FILE]    Launch Model Context Protocol (MCP) server");
    println!("  tapirus grep [OPTIONS] <PATTERN> [PATH]  Accelerated hybrid workspace search (tg)");
    println!();
    println!("Options:");
    println!("  -h, --help                               Print this help message");
    println!("  -v, --version                            Print TapirusDB version");
    println!();
    println!("Server Options (for 'tapirus serve'):");
    println!("  -p, --port <PORT>                        Port to listen on (default: 3005)");
    println!("  -b, --host <HOST>                        Host to bind to (default: 0.0.0.0)");
    println!("  --passphrase <KEY>                       Encryption passphrase (ChaCha20-Poly1305)");
    println!();
    println!("Arguments:");
    println!("  [DATABASE_FILE]                          Path to .tapir database file");
}

fn print_serve_help() {
    println!("Usage: tapirus serve [OPTIONS] [DATABASE_FILE]");
    println!();
    println!("Options:");
    println!("  -p, --port <PORT>        Port to listen on (default: 3005)");
    println!("  -b, --host <HOST>        Host to bind to (default: 0.0.0.0)");
    println!("  --passphrase <KEY>       Passphrase for encrypted database (ChaCha20-Poly1305)");
    println!("  -h, --help               Print this help message");
    println!();
    println!("Arguments:");
    println!("  [DATABASE_FILE]          Path to database file (defaults to: production.tapir)");
}

fn run_serve_command(args: &[String]) {
    let mut port: u16 = 3005;
    let mut host = "0.0.0.0".to_string();
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
            "-b" | "--host" => {
                if i + 1 < args.len() {
                    host = args[i + 1].clone();
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

    let is_memory = db_path == ":memory:";
    let conn = if let Some(ref pass) = passphrase {
        match Connection::open_encrypted(Path::new(&db_path), pass) {
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

    run_http_server(conn, &host, port, &db_path, passphrase.is_some());
}

fn run_http_server(conn: Connection, host: &str, port: u16, db_path: &str, encrypted: bool) {
    let addr = format!("{host}:{port}");
    let listener = match TcpListener::bind(&addr) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("Failed to bind server to {addr}: {e}");
            std::process::exit(1);
        }
    };

    println!("{BANNER}");
    println!("🚀 TapirusDB High-Performance HTTP Server running at http://{addr}");
    println!("📁 Database: {db_path} (Encrypted: {encrypted} | 100% Safe Rust)");
    println!("📡 Endpoints:");
    println!("   • GET  /health   -> Healthcheck & Version");
    println!("   • POST /api/sql  -> Execute SQL / Vector Search");
    println!("   • GET  /         -> Built-in Web UI Console");
    println!("\nPress Ctrl+C to stop.\n");

    let db = Arc::new(Mutex::new(conn));

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let db_clone = Arc::clone(&db);
                thread::spawn(move || {
                    handle_http_client(stream, db_clone);
                });
            }
            Err(e) => {
                eprintln!("Connection error: {e}");
            }
        }
    }
}

fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|window| window == needle)
}

fn handle_http_client(mut stream: TcpStream, db: Arc<Mutex<Connection>>) {
    let mut request_data = Vec::new();
    let mut buf = [0u8; 4096];
    let mut body_start = None;
    let mut content_length: usize = 0;

    loop {
        let n = match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
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
                if let Ok(header_str) = std::str::from_utf8(&request_data[..start]) {
                    for line in header_str.lines() {
                        if line.to_ascii_lowercase().starts_with("content-length:") {
                            if let Some(val) = line.split(':').nth(1) {
                                content_length = val.trim().parse().unwrap_or(0);
                            }
                        }
                    }
                }
            }
        }

        if let Some(start) = body_start {
            if request_data.len() >= start + content_length {
                break;
            }
        } else if request_data.len() > 65536 {
            break;
        }
    }

    if request_data.is_empty() {
        return;
    }

    let body_offset = body_start.unwrap_or(request_data.len());
    let header_bytes = &request_data[..body_offset];
    let body_bytes = if body_offset < request_data.len() {
        &request_data[body_offset..body_offset + content_length.min(request_data.len() - body_offset)]
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
    let path = parts.next().unwrap_or("/");

    // Handle CORS preflight
    if method == "OPTIONS" {
        send_http_response(&mut stream, "204 No Content", "text/plain", "");
        return;
    }

    // Healthcheck endpoint
    if method == "GET" && (path == "/health" || path == "/api/health") {
        let json = serde_json::json!({
            "status": "ok",
            "engine": "TapirusDB",
            "version": VERSION
        });
        send_http_response(&mut stream, "200 OK", "application/json", &json.to_string());
        return;
    }

    // Built-in Web Client UI
    if method == "GET" && (path == "/" || path == "/index.html") {
        let html = include_str!("../../ui/index.html");
        send_http_response(&mut stream, "200 OK", "text/html; charset=utf-8", html);
        return;
    }

    // Built-in Documentation Portal
    if method == "GET" && (path == "/docs" || path == "/docs.html") {
        let html = include_str!("../../ui/docs.html");
        send_http_response(&mut stream, "200 OK", "text/html; charset=utf-8", html);
        return;
    }

    // SQL execution endpoint
    if method == "POST" && (path == "/sql" || path == "/api/sql") {
        let body_str = std::str::from_utf8(body_bytes).unwrap_or("");
        let sql = match serde_json::from_str::<serde_json::Value>(body_str) {
            Ok(v) => v.get("sql").and_then(|s| s.as_str()).unwrap_or("").to_string(),
            Err(_) => {
                send_http_response(
                    &mut stream,
                    "400 Bad Request",
                    "application/json",
                    r#"{"error":"Invalid JSON payload. Expected {\"sql\": \"...\"}"}"#,
                );
                return;
            }
        };

        let trimmed = sql.trim();
        let conn = db.lock();

        if trimmed.to_uppercase().starts_with("SELECT") {
            match conn.query(trimmed) {
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
                    send_http_response(&mut stream, "200 OK", "application/json", &res.to_string());
                }
                Err(e) => {
                    let res = serde_json::json!({ "error": e.to_string() });
                    send_http_response(&mut stream, "400 Bad Request", "application/json", &res.to_string());
                }
            }
        } else {
            match conn.execute(trimmed) {
                Ok(affected) => {
                    let res = serde_json::json!({ "affected": affected });
                    send_http_response(&mut stream, "200 OK", "application/json", &res.to_string());
                }
                Err(e) => {
                    let res = serde_json::json!({ "error": e.to_string() });
                    send_http_response(&mut stream, "400 Bad Request", "application/json", &res.to_string());
                }
            }
        }
        return;
    }

    send_http_response(&mut stream, "404 Not Found", "text/plain", "Not Found");
}

fn send_http_response(stream: &mut TcpStream, status: &str, content_type: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 {status}\r\n\
         Content-Type: {content_type}\r\n\
         Content-Length: {}\r\n\
         Access-Control-Allow-Origin: *\r\n\
         Access-Control-Allow-Methods: GET, POST, OPTIONS\r\n\
         Access-Control-Allow-Headers: Content-Type\r\n\
         Connection: close\r\n\
         \r\n\
         {body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
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
                    execute_statement(conn, sql);
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
                    println!("WAL Checkpoint complete: {flushed} page(s) flushed in {:?}", start.elapsed());
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
                        println!("Hot backup created successfully at '{dest}' ({pages} pages) in {:?}", start.elapsed());
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
                        println!("Vacuum into '{dest}' completed ({pages} pages) in {:?}", start.elapsed());
                    }
                    Err(e) => {
                        println!("Vacuum error: {e}");
                    }
                }
            } else {
                match conn.vacuum() {
                    Ok(flushed) => {
                        println!("In-place vacuum complete: {flushed} page(s) checkpointed in {:?}", start.elapsed());
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
            println!("Engine: TapirusDB v{VERSION} (100% Pure Safe Rust)");
            let (nodes, edges) = conn.graph_stats();
            println!("Knowledge Graph: {nodes} nodes, {edges} edges");
            println!("Document Collections: {}", conn.collections().len());
            println!("Relational Tables: {}", conn.tables().len());
            false
        }
        _ => {
            println!("Unknown command: '{}'. Type '.help' for available commands.", parts[0]);
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
                println!("Shortest path ({}) hops found in {:?}:", path.len(), start.elapsed());
                for (i, edge) in path.iter().enumerate() {
                    println!(
                        "  Step {}: (#{}) -[{}]-> (#{})",
                        i + 1, edge.from_id, edge.label, edge.to_id
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

fn execute_statement(conn: &Connection, sql: &str) {
    let start = Instant::now();
    let upper = sql.trim_start().to_uppercase();
    let is_query = upper.starts_with("SELECT") || upper.starts_with("EXPLAIN");

    if is_query {
        match conn.query(sql) {
            Ok(rows) => {
                let elapsed = start.elapsed();
                render_ascii_table(&rows);
                println!("({} row(s) in {:?})\n", rows.len(), elapsed);
            }
            Err(e) => {
                println!("Query Error: {e}\n");
            }
        }
    } else {
        match conn.execute(sql) {
            Ok(affected) => {
                let elapsed = start.elapsed();
                println!("Query OK, {affected} row(s) affected in {:?}\n", elapsed);
            }
            Err(e) => {
                println!("Execution Error: {e}\n");
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
    println!("  INSERT INTO items (id, title, embedding) VALUES (1, 'Rover', [0.1, 0.2, 0.3, 0.4]);");
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
                            let hex_str: String = b.iter().map(|byte| format!("{:02X}", byte)).collect();
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

    for ch in line.chars() {
        match ch {
            '"' => {
                in_quotes = !in_quotes;
            }
            ',' if !in_quotes => {
                fields.push(current.trim().to_string());
                current.clear();
            }
            _ => {
                current.push(ch);
            }
        }
    }
    fields.push(current.trim().to_string());
    fields
}

fn handle_import_csv(conn: &Connection, csv_path: &str, table_name: &str) {
    let start = Instant::now();
    let content = match std::fs::read_to_string(csv_path) {
        Ok(c) => c,
        Err(e) => {
            println!("Import error: Could not read CSV file '{csv_path}': {e}");
            return;
        }
    };

    let lines: Vec<&str> = content.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.is_empty() {
        println!("Import warning: CSV file '{csv_path}' is empty");
        return;
    }

    let headers = parse_csv_line(lines[0]);
    if headers.is_empty() {
        println!("Import error: CSV has no column headers");
        return;
    }

    let header_str = headers.join(", ");
    let mut imported = 0;

    let _ = conn.begin_transaction();

    for line in &lines[1..] {
        let values = parse_csv_line(line);
        if values.len() != headers.len() {
            continue;
        }

        let formatted_vals: Vec<String> = values
            .iter()
            .map(|v| {
                if let Ok(i) = v.parse::<i64>() {
                    i.to_string()
                } else if let Ok(f) = v.parse::<f64>() {
                    f.to_string()
                } else if v.starts_with('[') && v.ends_with(']') {
                    v.clone()
                } else {
                    format!("'{}'", v.replace('\'', "''"))
                }
            })
            .collect();

        let sql = format!(
            "INSERT INTO {} ({}) VALUES ({});",
            table_name,
            header_str,
            formatted_vals.join(", ")
        );

        if conn.execute(&sql).is_ok() {
            imported += 1;
        }
    }

    let _ = conn.commit();

    println!(
        "Imported {} rows into table '{}' from '{}' in {:?}",
        imported,
        table_name,
        csv_path,
        start.elapsed()
    );
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
            println!("Unknown memory command: '{}'. Use .memory <count|remember|recall>", parts[1]);
        }
    }
}

fn print_mcp_help() {
    println!("Usage: tapirus mcp [OPTIONS] [DATABASE_FILE]");
    println!();
    println!("Launch Model Context Protocol (MCP) JSON-RPC 2.0 stdio server for AI agents.");
    println!("Compatible with Claude Desktop, Cursor, Gemini, and any MCP client.");
    println!();
    println!("Options:");
    println!("  --config-claude          Print ready-to-paste Claude Desktop JSON configuration");
    println!("  --passphrase <KEY>       Passphrase for encrypted database (ChaCha20-Poly1305)");
    println!("  -h, --help               Print this help message");
    println!();
    println!("Arguments:");
    println!("  [DATABASE_FILE]          Path to database file (defaults to: agent_memory.tapir)");
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

    let conn = if let Some(pass) = passphrase {
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
                                        "tags": { "type": "array", "items": { "type": "string" }, "description": "Optional category tags" }
                                    },
                                    "required": ["content"]
                                }
                            },
                            {
                                "name": "tapirus_recall",
                                "description": "Retrieve relevant memories using hybrid BM25 full-text keyword matching, semantic vectors, and temporal recency decay.",
                                "inputSchema": {
                                    "type": "object",
                                    "properties": {
                                        "query": { "type": "string", "description": "Search query or topic to recall" },
                                        "limit": { "type": "integer", "description": "Maximum number of memories to return (default: 5)" },
                                        "tags": { "type": "array", "items": { "type": "string" }, "description": "Filter by tags" }
                                    },
                                    "required": ["query"]
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
                let tool_args = params.get("arguments").cloned().unwrap_or(serde_json::json!({}));

                let tool_result = handle_mcp_tool_call(&conn, tool_name, &tool_args);
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

fn handle_mcp_tool_call(conn: &Connection, tool_name: &str, args: &serde_json::Value) -> String {
    match tool_name {
        "tapirus_remember" => {
            let content = args.get("content").and_then(|v| v.as_str()).unwrap_or("");
            let importance = args.get("importance").and_then(|v| v.as_f64()).unwrap_or(0.5) as f32;
            let namespace = args.get("namespace").and_then(|v| v.as_str());
            let session_id = args.get("session_id").and_then(|v| v.as_str());
            let tags: Vec<String> = args.get("tags")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().filter_map(|t| t.as_str().map(|s| s.to_string())).collect())
                .unwrap_or_default();
            let tag_refs: Vec<&str> = tags.iter().map(|s| s.as_str()).collect();

            match conn.memory_remember_text_scoped(content, importance, &tag_refs, namespace, session_id) {
                Ok(id) => serde_json::json!({
                    "status": "success",
                    "memory_id": id,
                    "stored_content": content,
                    "importance": importance,
                    "tags": tags,
                    "namespace": namespace,
                    "session_id": session_id,
                    "embedding": "auto-embedded (128D)"
                }).to_string(),
                Err(e) => serde_json::json!({ "status": "error", "message": format!("{e}") }).to_string(),
            }
        }
        "tapirus_recall" => {
            let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
            let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(5) as usize;
            let namespace = args.get("namespace").and_then(|v| v.as_str()).map(|s| s.to_string());
            let session_id = args.get("session_id").and_then(|v| v.as_str()).map(|s| s.to_string());
            let tags: Vec<String> = args.get("tags")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().filter_map(|t| t.as_str().map(|s| s.to_string())).collect())
                .unwrap_or_default();
            let tag_refs: Vec<&str> = tags.iter().map(|s| s.as_str()).collect();

            let mut filter = tapirus::MemoryRecallFilter::default().with_tags(&tag_refs);
            filter.namespace = namespace;
            filter.session_id = session_id;

            let embedder = tapirus::DeterministicHashEmbedder::default();
            let vector = embedder.embed_text(query);
            let results = conn.memory_recall(Some(query), Some(&vector), limit, &filter);

            let formatted: Vec<serde_json::Value> = results.into_iter().map(|r| {
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
            }).collect();

            serde_json::json!({
                "status": "success",
                "count": formatted.len(),
                "memories": formatted
            }).to_string()
        }
        "tapirus_sql" => {
            let sql = args.get("sql").and_then(|v| v.as_str()).unwrap_or("");
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
                "memory_safety": "100% Pure Safe Rust (#![forbid(unsafe_code)])",
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
                    extensions = Some(args[i + 1].split(',').map(|s| s.trim().to_lowercase()).collect());
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

    println!("\x1b[1;36m🔍 Tapirus Grep (tg)\x1b[0m — Searching for '\x1b[1;33m{pattern}\x1b[0m' in '{search_path}'...");
    let start_time = Instant::now();

    let embedder = if use_vector {
        Some(tapirus::memory::embedder::DeterministicHashEmbedder::new(64))
    } else {
        None
    };

    let query_vector = embedder.as_ref().map(|e| e.embed_text(&pattern));
    let pattern_lower = pattern.to_lowercase();
    let query_tokens: Vec<String> = pattern_lower.split_whitespace().map(|s| s.to_string()).collect();

    let mut matches = Vec::new();

    // Recursive directory walk
    fn walk_dir(
        dir: &Path,
        pattern: &str,
        pattern_lower: &str,
        query_tokens: &[String],
        ignore_case: bool,
        embedder: Option<&tapirus::memory::embedder::DeterministicHashEmbedder>,
        query_vector: Option<&[f32]>,
        extensions: Option<&[String]>,
        matches: &mut Vec<(f32, String, usize, String)>,
    ) {
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with('.') || name == "target" || name == "node_modules" || name == "dist" || name == "__pycache__" {
                    continue;
                }
            }

            if path.is_dir() {
                walk_dir(&path, pattern, pattern_lower, query_tokens, ignore_case, embedder, query_vector, extensions, matches);
            } else if path.is_file() {
                // Check extension
                if let Some(exts) = extensions {
                    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
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

                        let line_lower = if ignore_case { line.to_lowercase() } else { line.clone() };
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
        &pattern,
        &pattern_lower,
        &query_tokens,
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

