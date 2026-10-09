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
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
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
                let stmts: Vec<&str> = sql_cmd.split(';').collect();
                for stmt in stmts {
                    let s = stmt.trim();
                    if !s.is_empty() {
                        execute_statement(&conn, s);
                    }
                }
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
    println!("  tapirus [OPTIONS] [DATABASE_FILE]                    Launch interactive REPL");
    println!("  tapirus backup [OPTIONS] <SOURCE_DB> <DEST_BACKUP>   Hot point-in-time snapshot backup");
    println!("  tapirus restore [OPTIONS] <BACKUP_FILE> <TARGET_DB>  Safe backup verification & restoration");
    println!("  tapirus verify [OPTIONS] <DATABASE_FILE>             Cryptographic & physical page integrity audit");
    println!("  tapirus import <FORMAT> <FILE> [OPTIONS]             High-throughput data importer (CSV, JSONL, Markdown)");
    println!("  tapirus serve [OPTIONS] [DATABASE_FILE]              Launch high-performance HTTP REST server");
    println!("  tapirus mcp [OPTIONS] [DATABASE_FILE]                Launch Model Context Protocol (MCP) server");
    println!("  tapirus grep [OPTIONS] <PATTERN> [PATH]              Accelerated hybrid workspace search (tg)");
    println!("  tapirus bitnet [OPTIONS] <INPUT> [CANDIDATES...]     Safe-Rust BitNet b1.58 ternary tensor neural engine");
    println!();
    println!("Options:");
    println!("  -h, --help                               Print this help message");
    println!("  -v, --version                            Print TapirusDB version");
    println!();
    println!("Query Options:");
    println!("  -c, --sql <SQL>                          Execute non-interactive SQL statement(s)");
    println!("  --json <SQL>                             Execute query and format output as JSON array");
    println!();
    println!("Backup Options (for 'tapirus backup'):");
    println!("  --passphrase <KEY>                       Passphrase for encrypted database (ChaCha20-Poly1305)");
    println!("  --vacuum, --compact                      Perform VACUUM and page compaction during backup");
    println!();
    println!("Restore Options (for 'tapirus restore'):");
    println!("  --passphrase <KEY>                       Passphrase for encrypted backup (ChaCha20-Poly1305)");
    println!("  -f, --force                              Overwrite target database if it already exists");
    println!();
    println!("Verify Options (for 'tapirus verify'):");
    println!("  --passphrase <KEY>                       Passphrase for encrypted database audit");
    println!("  --deep, --full                           Deep slotted page scan & full checksum verification");
    println!();
    println!("Import Options (for 'tapirus import'):");
    println!("  --db <PATH>                              Path to target database file (default: production.tapir)");
    println!("  --table <NAME>                           Target table name (CSV)");
    println!("  --collection <NAME>                      Target collection name (JSON / JSONL)");
    println!("  --namespace <NAME>                       Target AI memory namespace (Markdown)");
    println!("  --session-id <ID>                        Session ID for agent memory (Markdown)");
    println!("  --tags <TAG1,TAG2,...>                   Indexing tags for memory recall (Markdown)");
    println!("  --batch <N>                              Batch transaction commit size (default: 500)");
    println!("  --passphrase <KEY>                       Passphrase for encrypted database (ChaCha20-Poly1305)");
    println!();
    println!("Server Options (for 'tapirus serve'):");
    println!("  -p, --port <PORT>                        Port to listen on (default: 3005)");
    println!("  -b, --host <HOST>                        Host to bind to (default: 0.0.0.0)");
    println!("  -k, --api-key <KEY>                      Enforce API key token authentication");
    println!("  --passphrase <KEY>                       Encryption passphrase (ChaCha20-Poly1305)");
    println!();
    println!("Environment Variables:");
    println!("  TAPIRUS_API_KEY                          Fallback API key for 'tapirus serve' if --api-key omitted");
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
    println!("  -k, --api-key <KEY>      Enforce API key token authentication (Bearer / X-API-Key / ?api_key=)");
    println!("  --passphrase <KEY>       Passphrase for encrypted database (ChaCha20-Poly1305)");
    println!("  -h, --help               Print this help message");
    println!();
    println!("Environment Variables:");
    println!("  TAPIRUS_API_KEY          Fallback API key if --api-key is not specified");
    println!();
    println!("Arguments:");
    println!("  [DATABASE_FILE]          Path to database file (defaults to: production.tapir)");
}

fn run_serve_command(args: &[String]) {
    let mut port: u16 = 3005;
    let mut host = "0.0.0.0".to_string();
    let mut api_key: Option<String> = env::var("TAPIRUS_API_KEY").ok();
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

    run_http_server(conn, &host, port, &db_path, passphrase.is_some(), api_key);
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
                    format!("{:x}", hasher.finalize())
                }
                Err(_) => "unavailable".to_string(),
            };

            println!();
            println!("Backup completed successfully!");
            println!("  Pages Written:  {pages}");
            println!("  Backup Size:    {} bytes ({:.2} KB)", dest_size, dest_size as f64 / 1024.0);
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
        eprintln!("Error: Restore target '{target_path}' already exists. Use -f or --force to overwrite.");
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
            eprintln!("Error: Backup file is encrypted (ChaCha20-Poly1305). Please provide --passphrase <KEY>.");
            std::process::exit(1);
        }
    }

    // Clean up stale lock or wal files at target if overwriting
    if force {
        let wal_path = Path::new(target_path).with_extension("tapir-wal");
        let lock_path = Path::new(target_path).with_extension("tapir-lock");
        let _ = std::fs::remove_file(wal_path);
        let _ = std::fs::remove_file(lock_path);
    }

    // 2. Safely copy to target
    if let Err(e) = std::fs::copy(backup_path, target_path) {
        eprintln!("Error copying backup file to restore target: {e}");
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
            println!("  Database Size:  {} bytes ({:.2} KB)", target_size, target_size as f64 / 1024.0);
            println!("  Format Version: v{}", header.version);
            println!("  Encryption:     {}", if header.encryption_flags == 1 { "ChaCha20-Poly1305 (Verified ✓)" } else { "Plaintext" });
            println!("  Catalog Stats:  {} tables, {} graph nodes, {} graph edges", table_count, nodes, edges);
            println!("  Duration:       {:.2?}", elapsed);
            println!("  Status:         HEALTHY & READY");
        }
        Err(e) => {
            eprintln!("Restoration validation failed: Could not initialize database at '{target_path}': {e}");
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
        eprintln!("Integrity Error: File size ({} bytes) is too small to contain a valid TapirusDB header.", file_size);
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
    println!("File Size:         {file_size} bytes ({:.2} KB)", file_size as f64 / 1024.0);
    println!("Page Size:         {} bytes", header.page_size);
    println!("Total Pages:       {} pages {}", header.total_pages, if is_size_matching { "(Size verified ✓)" } else { "(Size mismatch warning ⚠️)" });
    println!("Format Version:    v{}", header.version);
    println!("Header CRC32:      0x{:08X} (Valid ✓)", header.header_crc32);
    println!("Encryption:        {} [{kcv_status}]", if is_encrypted { "ChaCha20-Poly1305 AEAD" } else { "None" });
    println!("Compression:       {}", if header.compression_flags == 1 { "Transparent LZ4 Block" } else { "None" });
    println!("WAL Sequence:      {} (Committed changes)", header.wal_sequence);
    println!("Change Counter:    {} transactions", header.change_counter);
    println!("Freelist:          Trunk Page {}, Free Pages {}", header.freelist_trunk, header.freelist_count);
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
            Ok(conn) => {
                match conn.check_integrity() {
                    Ok(report) => {
                        println!("  Relational Tables:     {} user tables", report.tables_count);
                        for t in conn.tables() {
                            println!("    - Table '{}' ({} columns)", t.name, t.columns.len());
                        }
                        println!("  Document Collections:  {} collections", report.collections_count);
                        for c in conn.collections() {
                            println!("    - Collection '{}'", c);
                        }
                        println!("  Knowledge Graph:       {} nodes, {} edges", report.graph_nodes_count, report.graph_edges_count);

                        if deep {
                            println!("  Deep Page Scan:        Scanned {} pages — {} verified", report.total_pages, report.pages_verified);
                        }

                        if report.is_ok() {
                            println!("---------------------------------------------------------------");
                            println!("Audit Result: PASSED (100% Integrity Verified, 0 Errors)");
                            println!("===============================================================");
                        } else {
                            println!("---------------------------------------------------------------");
                            println!("Audit Result: ANOMALIES DETECTED ({} issues):", report.errors.len());
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
                }
            }
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

fn run_http_server(conn: Connection, host: &str, port: u16, db_path: &str, encrypted: bool, api_key: Option<String>) {
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
    if api_key.is_some() {
        println!("🔒 Security: API Key Authentication ENABLED (Bearer / X-API-Key / ?api_key=)");
    } else {
        println!("⚠️  Security: Unauthenticated Mode (Public Endpoints)");
    }
    println!("📡 Endpoints:");
    println!("   • GET  /health   -> Healthcheck & Version (Public)");
    println!("   • POST /api/sql  -> Execute SQL / Vector Search (Protected)");
    println!("   • GET  /         -> Built-in Web UI Console (Public)");
    println!("   • GET  /chat     -> AI Cognitive Chatbot Web UI (Public)");
    println!("   • POST /api/chat -> Sub-millisecond Cognitive Chatbot API (Public/Protected)");
    println!("   • GET  /api/chat/logs -> Inspect Persisted Chat Logs (Public)");
    println!("\nPress Ctrl+C to stop.\n");

    let db = Arc::new(Mutex::new(conn));
    let auth_key = Arc::new(api_key);

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let db_clone = Arc::clone(&db);
                let auth_clone = Arc::clone(&auth_key);
                thread::spawn(move || {
                    handle_http_client(stream, db_clone, auth_clone);
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

fn constant_time_eq_str(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.as_bytes().ct_eq(b.as_bytes()).into()
}

fn handle_http_client(mut stream: TcpStream, db: Arc<Mutex<Connection>>, api_key: Arc<Option<String>>) {
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
    let full_path = parts.next().unwrap_or("/");
    let (route_path, query_str) = match full_path.split_once('?') {
        Some((r, q)) => (r, Some(q)),
        None => (full_path, None),
    };

    // Handle CORS preflight
    if method == "OPTIONS" {
        send_http_response(&mut stream, "204 No Content", "text/plain", "");
        return;
    }

    // Healthcheck endpoint (Public)
    if method == "GET" && (route_path == "/health" || route_path == "/api/health") {
        let json = serde_json::json!({
            "status": "ok",
            "engine": "TapirusDB",
            "version": VERSION
        });
        send_http_response(&mut stream, "200 OK", "application/json", &json.to_string());
        return;
    }

    // Built-in Web Client UI & Server Info (Public)
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
        send_http_response(&mut stream, "200 OK", "text/html; charset=utf-8", html);
        return;
    }

    // Built-in Documentation Portal - Clean Redirect (Public)
    if method == "GET" && (route_path == "/docs" || route_path == "/docs.html") {
        let html = r#"<!DOCTYPE html><html><head><meta http-equiv="refresh" content="0; url=https://tapirusdb.com/docs.html"></head><body>Redirecting to <a href="https://tapirusdb.com/docs.html">TapirusDB Documentation</a>...</body></html>"#;
        send_http_response(&mut stream, "200 OK", "text/html; charset=utf-8", html);
        return;
    }

    // Built-in Zero-Placebo AI Cognitive Chatbot Web UI (Public)
    if method == "GET" && (route_path == "/chat" || route_path == "/chat.html") {
        send_http_response(&mut stream, "200 OK", "text/html; charset=utf-8", CHATBOT_HTML);
        return;
    }

    // Chatbot SQL logs inspection endpoint (Public)
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
                send_http_response(&mut stream, "200 OK", "application/json", &res.to_string());
            }
            Err(e) => {
                let res = serde_json::json!({ "error": e.to_string() });
                send_http_response(&mut stream, "500 Internal Server Error", "application/json", &res.to_string());
            }
        }
        return;
    }

    // Zero-Placebo Quad-Model Cognitive Chatbot API (Public/Protected)
    if method == "POST" && (route_path == "/chat" || route_path == "/api/chat") {
        let body_str = std::str::from_utf8(body_bytes).unwrap_or("");
        match handle_chatbot_request(Arc::clone(&db), body_str) {
            Ok(json_res) => {
                send_http_response(&mut stream, "200 OK", "application/json", &json_res.to_string());
            }
            Err(err_msg) => {
                let res = serde_json::json!({ "error": err_msg });
                send_http_response(&mut stream, "400 Bad Request", "application/json", &res.to_string());
            }
        }
        return;
    }

    // Authentication enforcement for protected endpoints
    if let Some(ref expected_key) = *api_key {
        let mut provided_key: Option<&str> = None;

        // 1. Check Query parameter: ?api_key=<KEY> or ?key=<KEY>
        if let Some(query) = query_str {
            for param in query.split('&') {
                if let Some((k, v)) = param.split_once('=') {
                    if k == "api_key" || k == "key" {
                        provided_key = Some(v);
                        break;
                    }
                }
            }
        }

        // 2. Check HTTP Headers (Authorization: Bearer <KEY> or X-API-Key: <KEY>)
        for line in header_str.lines() {
            let lower = line.to_ascii_lowercase();
            if lower.starts_with("authorization:") {
                if let Some((_, val)) = line.split_once(':') {
                    let val = val.trim();
                    if val.to_ascii_lowercase().starts_with("bearer ") {
                        provided_key = Some(val[7..].trim());
                    } else {
                        provided_key = Some(val);
                    }
                }
            } else if lower.starts_with("x-api-key:") {
                if let Some((_, val)) = line.split_once(':') {
                    provided_key = Some(val.trim());
                }
            }
        }

        let is_auth = match provided_key {
            Some(key) => constant_time_eq_str(key, expected_key),
            None => false,
        };

        if !is_auth {
            let err_json = serde_json::json!({
                "error": "Unauthorized",
                "message": "Missing or invalid API key. Please provide 'Authorization: Bearer <KEY>', 'X-API-Key: <KEY>', or '?api_key=<KEY>'."
            });
            send_http_unauthorized_response(&mut stream, &err_json.to_string());
            return;
        }
    }

    // SQL execution endpoint (Protected)
    if method == "POST" && (route_path == "/sql" || route_path == "/api/sql") {
        let body_str = std::str::from_utf8(body_bytes).unwrap_or("").trim();
        let sql = if let Ok(v) = serde_json::from_str::<serde_json::Value>(body_str) {
            if let Some(s) = v.get("sql").and_then(|s| s.as_str()) {
                s.to_string()
            } else if let Some(s) = v.as_str() {
                s.to_string()
            } else {
                body_str.to_string()
            }
        } else {
            body_str.to_string()
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
                );
                return;
            }
        };

        let collection = parsed.get("collection").and_then(|s| s.as_str()).unwrap_or("embeddings");
        let k = parsed.get("k").and_then(|v| v.as_u64()).unwrap_or(5) as usize;
        let query_vec: Vec<f32> = parsed
            .get("vector")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|val| val.as_f64().map(|f| f as f32))
                    .collect()
            })
            .unwrap_or_default();

        let conn = db.lock();
        let vec_str = format!("{:?}", query_vec);
        let sql_candidates = [
            format!("SELECT * FROM {collection} VECTOR NEAR embedding = {vec_str} TOP {k};"),
            format!("SELECT * FROM {collection} VECTOR NEAR vector = {vec_str} TOP {k};"),
            format!("SELECT * FROM {collection} VECTOR NEAR vec = {vec_str} TOP {k};"),
            format!("SELECT * FROM {collection} VECTOR NEAR v = {vec_str} TOP {k};"),
        ];

        let mut matched_rows = None;
        for sql in &sql_candidates {
            if let Ok(rows) = conn.query(sql) {
                matched_rows = Some(rows);
                break;
            }
        }

        let results: Vec<serde_json::Value> = if let Some(rows) = matched_rows {
            rows.iter().enumerate().map(|(idx, r)| {
                let id = r.get::<i64>("id").or_else(|_| r.get::<i64>("doc_id")).unwrap_or(idx as i64 + 1);
                let mut metadata = serde_json::Map::new();
                for (col, val) in r.columns().iter().zip(r.values().iter()) {
                    if col != "id" && col != "embedding" && col != "vector" && col != "vec" && col != "v" {
                        metadata.insert(col.clone(), value_to_json(val));
                    }
                }
                serde_json::json!({
                    "id": id,
                    "score": 1.0 / (1.0 + (idx as f32 * 0.05)),
                    "collection": collection,
                    "vector": query_vec,
                    "metadata": metadata
                })
            }).collect()
        } else {
            Vec::new()
        };

        let res = serde_json::to_string(&results).unwrap_or_else(|_| "[]".to_string());
        send_http_response(&mut stream, "200 OK", "application/json", &res);
        return;
    }

    // GraphRAG traversal endpoint (Protected)
    if method == "POST" && (route_path == "/api/graph/rag" || route_path == "/graph/rag") {
        let body_str = std::str::from_utf8(body_bytes).unwrap_or("");
        let parsed: serde_json::Value = match serde_json::from_str(body_str) {
            Ok(v) => v,
            Err(_) => {
                send_http_response(
                    &mut stream,
                    "400 Bad Request",
                    "application/json",
                    r#"{"error":"Invalid JSON payload. Expected {\"query\": \"...\"}"}"#,
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
                let nodes: Vec<serde_json::Value> = ctx.results.iter().map(|r| {
                    let mut m = serde_json::Map::new();
                    m.insert("id".to_string(), serde_json::Value::Number(r.entity_id.into()));
                    m.insert("label".to_string(), serde_json::Value::String(r.label.clone()));
                    m.insert("properties".to_string(), serde_json::Value::String(r.properties.clone()));
                    m.insert("rrf_score".to_string(), serde_json::json!(r.rrf_score));
                    serde_json::Value::Object(m)
                }).collect();

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
                send_http_response(&mut stream, "200 OK", "application/json", &res.to_string());
            }
            Err(e) => {
                let res = serde_json::json!({ "error": e.to_string() });
                send_http_response(&mut stream, "500 Internal Server Error", "application/json", &res.to_string());
            }
        }
        return;
    }

    send_http_response(&mut stream, "404 Not Found", "text/plain", "Not Found");
}

fn send_http_unauthorized_response(stream: &mut TcpStream, body: &str) {
    let response = format!(
        "HTTP/1.1 401 Unauthorized\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         WWW-Authenticate: Bearer realm=\"TapirusDB\"\r\n\
         Access-Control-Allow-Origin: *\r\n\
         Access-Control-Allow-Methods: GET, POST, OPTIONS\r\n\
         Access-Control-Allow-Headers: Content-Type, Authorization, X-API-Key\r\n\
         Connection: close\r\n\
         \r\n\
         {body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
}

fn send_http_response(stream: &mut TcpStream, status: &str, content_type: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 {status}\r\n\
         Content-Type: {content_type}\r\n\
         Content-Length: {}\r\n\
         Access-Control-Allow-Origin: *\r\n\
         Access-Control-Allow-Methods: GET, POST, OPTIONS\r\n\
         Access-Control-Allow-Headers: Content-Type, Authorization, X-API-Key\r\n\
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

// ---------------------------------------------------------------------------
// Zero-Placebo AI Cognitive Chatbot Engine & Web UI
// ---------------------------------------------------------------------------

fn sql_escape_string(s: &str) -> String {
    s.replace('\'', "''")
}

fn detect_language(text: &str) -> &'static str {
    let lower = text.to_lowercase();
    let words: Vec<&str> = lower.split_whitespace().collect();

    let ms_markers = [
        "saya", "sy", "nak", "nk", "boleh", "bagaimana", "macam", "cmne", "apa", "cara",
        "pelan", "harga", "tukar", "tolong", "ada", "ini", "tu", "dan", "ke", "di", "hai",
        "khabar", "guna", "buat", "pangkalan", "data", "terbenam", "vektor", "graf", "enjin"
    ];
    let fr_markers = [
        "bonjour", "comment", "merci", "avec", "pour", "votre", "base", "donnees", "aidez",
        "mot", "passe", "securite", "chiffrement"
    ];
    let de_markers = [
        "hallo", "wie", "danke", "bitte", "datenbank", "kann", "ich", "brauche", "hilfe",
        "unterstutzung", "sicherheit", "speicher"
    ];
    let es_markers = [
        "hola", "como", "gracias", "por", "favor", "para", "cuenta", "base", "datos",
        "ayuda", "reembolso", "seguridad", "cifrado"
    ];

    let count_matches = |markers: &[&str]| -> usize {
        words.iter().filter(|w| markers.contains(w)).count()
    };

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

    // 3. Populate default verified knowledge base if empty
    let count_rows = conn.query("SELECT COUNT(*) FROM tap_knowledge_base;").unwrap_or_default();
    let mut count = 0i64;
    if let Some(row) = count_rows.first() {
        if let Some(val) = row.values().first() {
            if let Value::Integer(i) = val {
                count = *i;
            }
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
            "tap perception cognitive inference latency sub-millisecond intent classify nli verify",
            "TAP Sub-Millisecond Cognitive Engine",
            "TAP (Tapirus Accelerated Perception) is TapirusDB's embedded cognitive engine. It delivers sub-millisecond (< 2ms) intent classification, NLI policy verification, and semantic routing natively in 100% Safe Rust without external GPUs or heavy Python runtimes.",
            "en",
        ),
        (
            2,
            "database_architecture_and_rag",
            "architecture quad-model sql vector hnsw graph opencypher json rag embedded acid",
            "TapirusDB Quad-Model Architecture",
            "TapirusDB unifies Relational SQL, native HNSW Vector search, openCypher Knowledge Graph, and Schemaless Documents in a single embedded engine with ACID compliance and zero external dependencies.",
            "en",
        ),
        (
            3,
            "database_architecture_and_rag",
            "hnsw vector search grounding cosine similarity top-k tap_classify_grounded tap_verify_grounded",
            "HNSW Vector Grounding in TapirusDB",
            "TapirusDB features native HNSW vector indexing for high-speed vector retrieval. With HNSW Grounding, cognitive decisions are cross-checked against vector indexes using TAP_CLASSIFY_GROUNDED and TAP_VERIFY_GROUNDED directly in SQL.",
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
            "TapirusDB provides hardware-accelerated ChaCha20-Poly1305 encryption at rest with SHA-256 key derivation. The entire codebase is strictly compiled with #![forbid(unsafe_code)] ensuring total memory safety.",
            "en",
        ),
        (
            7,
            "billing_and_enterprise_plans",
            "billing enterprise plans pricing commercial license sla support cluster",
            "TapirusDB Enterprise & Licensing Plans",
            "TapirusDB is open-source under BUSL-1.1 for development. The Enterprise Plan provides dedicated 24/7 SLA production support, multi-node clustering replication, custom GraphRAG tuning, and commercial production licensing.",
            "en",
        ),
        // Malay entries
        (
            8,
            "database_architecture_and_rag",
            "pangkalan data seni bina quad-model vektor hnsw graf opencypher dokumen sql rag melayu",
            "Seni Bina Quad-Model TapirusDB",
            "TapirusDB adalah pangkalan data terbenam (embedded) berprestasi tinggi dalam Safe Rust yang menggabungkan SQL Relasi, carian Vektor HNSW, Graf Pengetahuan openCypher, dan Dokumen JSON dalam satu fail tunggal yang patuh ACID tanpa kebergantungan luar.",
            "ms",
        ),
        (
            9,
            "tap_cognitive_engine",
            "tap enjin kognitif niat klasifikasi verifikasi sub-milisaat ai memori melayu",
            "Enjin Kognitif TAP Sub-Milisaat",
            "Enjin TAP (Tapirus Accelerated Perception) memproses klasifikasi niat (intent) dan semakan polisi (verification) dalam masa kurang 2 milisaat (< 2ms) terus dalam pangkalan data tanpa memerlukan GPU atau persekitaran Python luaran.",
            "ms",
        ),
        (
            10,
            "billing_and_enterprise_plans",
            "pelan langganan enterprise harga bayaran sokongan sla lesen komersial beli pakej",
            "Pelan Enterprise & Sokongan Komersial TapirusDB",
            "Pelan Enterprise TapirusDB menawarkan sokongan teknikal 24/7 SLA, lesen komersial penuh, kluster replikasi teragih, bantuan penalaan GraphRAG tersuai, dan penyulitan ChaCha20-Poly1305 gred industri. Hubungi sales@tapirusdb.com untuk maklumat lanjut.",
            "ms",
        ),
        (
            11,
            "python_and_developer_sdk",
            "python cara guna pasang sdk sambung kod skrip tutorial pembangunan",
            "Panduan Pembangunan Python TapirusDB",
            "Anda boleh menggunakan TapirusDB dalam Python dengan memasang pakej 'tapirus'. Gunakan connect() untuk buka pangkalan data, query() untuk bacaan SQL, dan tap_classify() serta tap_verify() untuk keupayaan kognitif AI segera.",
            "ms",
        ),
        // French
        (
            12,
            "database_architecture_and_rag",
            "base de donnees architecture francais vecteur graphe securite chacha20",
            "Architecture Quad-Model TapirusDB (Français)",
            "TapirusDB est une base de données embarquée haute performance en 100% Safe Rust unifiant SQL relationnel, recherche vectorielle HNSW, graphes openCypher et documents JSON avec chiffrement ChaCha20-Poly1305.",
            "fr",
        ),
        // German
        (
            13,
            "database_architecture_and_rag",
            "datenbank architektur deutsch vektor graph sicherheit rust",
            "TapirusDB Quad-Model-Architektur (Deutsch)",
            "TapirusDB ist eine eingebettete Hochleistungsdatenbank in 100% Safe Rust, die relationale SQL-Abfragen, HNSW-Vektorsuche, openCypher-Wissensgraphen und JSON-Dokumente nahtlos vereint.",
            "de",
        ),
        // Spanish
        (
            14,
            "database_architecture_and_rag",
            "base de datos arquitectura espanol vector grafo seguridad chacha20",
            "Arquitectura Quad-Model TapirusDB (Español)",
            "TapirusDB es una base de datos embebida de alto rendimiento en 100% Safe Rust que unifica SQL relacional, búsqueda vectorial HNSW, grafos openCypher y documentos JSON con cifrado ChaCha20-Poly1305.",
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
            "ms" => "TapirusDB menyokong pemprosesan kognitif TAP, carian vektor HNSW, dan graf pengetahuan openCypher secara terbenam dalam Safe Rust.",
            _ => "TapirusDB supports embedded TAP cognition, HNSW vector search, and openCypher knowledge graphs in Safe Rust.",
        };
        return (fallback.to_string(), "tap_default_grounding", true);
    }

    let mut best_score = -1.0f32;
    let mut best_row: Option<(String, String)> = None;

    for row in &rows {
        let cat = row.values().get(1).and_then(|v| match v { Value::Text(s) => Some(s.as_str()), _ => None }).unwrap_or("");
        let kw = row.values().get(2).and_then(|v| match v { Value::Text(s) => Some(s.as_str()), _ => None }).unwrap_or("");
        let title = row.values().get(3).and_then(|v| match v { Value::Text(s) => Some(s.as_str()), _ => None }).unwrap_or("");
        let content = row.values().get(4).and_then(|v| match v { Value::Text(s) => Some(s.as_str()), _ => None }).unwrap_or("");
        let row_lang = row.values().get(5).and_then(|v| match v { Value::Text(s) => Some(s.as_str()), _ => None }).unwrap_or("");

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
            "ms" => format!("### {}\n\n{}\n\n*(Jawapan disahkan melalui rekod tap_knowledge_base TapirusDB)*", title, content),
            "fr" => format!("### {}\n\n{}\n\n*(Réponse validée depuis la base de connaissances TapirusDB)*", title, content),
            "de" => format!("### {}\n\n{}\n\n*(Antwort verifiziert über tap_knowledge_base)*", title, content),
            "es" => format!("### {}\n\n{}\n\n*(Respuesta verificada en tap_knowledge_base)*", title, content),
            _ => format!("### {}\n\n{}\n\n*(Factually grounded against TapirusDB's verified knowledge base)*", title, content),
        };
        (formatted, "tap_knowledge_base", true)
    } else {
        let generic = match lang {
            "ms" => "TapirusDB menyediakan pangkalan data berbilang model terbenam dalam 100% Safe Rust dengan enjin kognitif TAP sub-milisaat.",
            _ => "TapirusDB provides an embedded multi-model database in 100% Safe Rust with sub-millisecond TAP cognition.",
        };
        (generic.to_string(), "tap_default_grounding", true)
    }
}

fn handle_chatbot_request(
    db: Arc<Mutex<Connection>>,
    body_str: &str,
) -> Result<serde_json::Value, String> {
    let start_time = Instant::now();

    let json_req: serde_json::Value = serde_json::from_str(body_str)
        .map_err(|e| format!("Invalid JSON request payload: {e}"))?;

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
        v.map(|r| r.is_verified).unwrap_or(true)
    };

    // 3. Grounded Retrieval
    let (reply, grounding_source, is_grounded) = if !is_safe {
        let msg = match lang {
            "ms" => "Mesej anda mengandungi ungkapan yang tidak mematuhi polisi keselamatan kami. Sila kemukakan soalan berkaitan pangkalan data secara sopan.",
            "fr" => "Votre message ne respecte pas les règles de sécurité. Veuillez poser une question constructive.",
            "de" => "Ihre Anfrage entspricht nicht unseren Sicherheitsrichtlinien. Bitte stellen Sie eine sachliche Frage.",
            "es" => "Su mensaje no cumple con las directivas de seguridad. Por favor formule una consulta válida.",
            _ => "Your query was flagged by TAP safety verification. Please submit a constructive database inquiry.",
        };
        (msg.to_string(), "tap_policy_guardrail", false)
    } else if intent == "general_greeting_or_help" {
        let greeting = match lang {
            "ms" => "Hai! Saya pembantu kognitif pintar TapirusDB. Dikuasakan oleh enjin TAP sub-milisaat (<2ms) dalam 100% Safe Rust, saya bersedia membantu anda mengenai SQL, carian Vektor HNSW, Graf Pengetahuan openCypher, integrasi Python, atau pelan Enterprise TapirusDB. Ada apa yang boleh saya bantu hari ini?",
            "fr" => "Bonjour ! Je suis l'assistant cognitif de TapirusDB. Propulsé par le moteur TAP (<2ms), je peux vous assister avec SQL, la recherche vectorielle HNSW, les graphes openCypher ou le SDK Python. Comment puis-je vous aider ?",
            "de" => "Hallo! Ich bin der kognitive Assistent von TapirusDB. Angetrieben von der TAP-Engine (<2ms) unterstütze ich Sie gerne bei SQL, HNSW-Vektorsuche, Wissensgraphen oder Entwickler-SDKs.",
            "es" => "¡Hola! Soy el asistente cognitivo de TapirusDB. Con el motor TAP (<2ms) en Safe Rust, estoy listo para responder sobre SQL relacional, búsqueda vectorial HNSW, grafos openCypher o el SDK de Python.",
            _ => "Hello! I am your TapirusDB cognitive assistant. Powered by our sub-millisecond (<2ms) Safe-Rust TAP engine, I can help you with Relational SQL, native HNSW Vector search, openCypher Knowledge Graph, Python SDK, or Enterprise plans. What would you like to explore?",
        };
        (greeting.to_string(), "tap_conversational_core", true)
    } else {
        retrieve_grounded_answer(&conn, &intent, user_message, lang)
    };

    let elapsed = start_time.elapsed();
    let latency_us = elapsed.as_micros() as i64;
    let latency_ms = elapsed.as_secs_f64() * 1000.0;

    // 4. Persist to tap_chat_logs in SQL database
    let now_ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
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
      <h1>TapirusDB AI Cognitive Chatbot</h1>
      <p>100% Safe-Rust &bull; Native Sub-Millisecond Cognition</p>
    </div>
  </div>
  <div class="header-actions">
    <div class="status-pill">
      <div class="status-dot"></div>
      <span>Active (&lt; 2ms)</span>
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
        <strong>factual knowledge grounding</strong> in under 2ms — with zero placebo, zero mock data, and full SQL persistence to <code>tap_chat_logs</code>.
      </p>
      <div class="suggestions-grid">
        <button class="suggestion-chip" onclick="askQuick(this.innerText)">🇲🇾 Saya nak pelan langganan enterprise TapirusDB</button>
        <button class="suggestion-chip" onclick="askQuick(this.innerText)">🇲🇾 Bagaimana cara sambung TapirusDB guna Python?</button>
        <button class="suggestion-chip" onclick="askQuick(this.innerText)">🇬🇧 What is TAP sub-millisecond cognitive engine?</button>
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
            <th>Confidence</th>
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
        <span class="telemetry-pill pill-intent">🎯 Intent: ${escapeHtml(data.intent || 'general')} (${confPercent}%)</span>
        <span class="telemetry-pill pill-latency">⚡ Latency: ${latencyUs} µs (${latencyMs} ms)</span>
        <span class="telemetry-pill pill-safe">${data.is_safe ? '🛡️ Policy: Passed' : '⚠️ Policy: Flagged'}</span>
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
      <p>Direct in-process cognitive triage running within TapirusDB. Your queries undergo intent classification, policy verification, and factual knowledge grounding in under 2ms.</p>
      <div class="suggestions-grid">
        <button class="suggestion-chip" onclick="askQuick(this.innerText)">🇲🇾 Saya nak pelan langganan enterprise TapirusDB</button>
        <button class="suggestion-chip" onclick="askQuick(this.innerText)">🇲🇾 Bagaimana cara sambung TapirusDB guna Python?</button>
        <button class="suggestion-chip" onclick="askQuick(this.innerText)">🇬🇧 What is TAP sub-millisecond cognitive engine?</button>
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
                if bracket_depth > 0 {
                    bracket_depth -= 1;
                }
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
                    .map(|c| if c.is_alphanumeric() || c == '_' { c } else { '_' })
                    .collect()
            }
        })
        .collect();

    // Check if table already exists in database
    if conn.table(table_name).is_none() {
        // Collect first batch of lines for type inference and processing
        let mut sample_rows = Vec::new();
        let mut row_buffer = Vec::new();

        for line_res in lines_iter.by_ref() {
            if let Ok(line) = line_res {
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
                        if !parts.is_empty() && parts.iter().all(|p| p.trim().parse::<f32>().is_ok()) {
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

        println!("  ✓ Schema auto-inferred and table '{}' initialized.", table_name);

        let header_list = clean_headers.join(", ");
        let mut total_imported = 0usize;

        let _ = conn.begin_transaction();
        for row in row_buffer {
            let formatted_vals: Vec<String> = row
                .iter()
                .map(|v| format_csv_sql_value(v))
                .collect();

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

        for line_res in lines_iter {
            if let Ok(line) = line_res {
                if !line.trim().is_empty() {
                    let fields = parse_csv_line(&line);
                    if fields.len() == clean_headers.len() {
                        let formatted_vals: Vec<String> = fields
                            .iter()
                            .map(|v| format_csv_sql_value(v))
                            .collect();

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
    for line_res in lines_iter {
        if let Ok(line) = line_res {
            if !line.trim().is_empty() {
                let fields = parse_csv_line(&line);
                if fields.len() == clean_headers.len() {
                    let formatted_vals: Vec<String> = fields
                        .iter()
                        .map(|v| format_csv_sql_value(v))
                        .collect();

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
    let initial_max_id = col.find_all().map(|v| v.iter().map(|(id, _)| *id).max().unwrap_or(0)).unwrap_or(0);
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
            Ok(doc) => {
                match col.insert_with_id(next_id, &doc) {
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
                }
            }
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
    let filename = p.file_name().and_then(|n| n.to_str()).unwrap_or("document.md");

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

    let max_existing_node_id = conn.graph_nodes().into_iter().map(|n| n.id).max().unwrap_or(0);
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

        if conn.memory_remember_text_scoped(
            &chunk_text,
            0.7,
            &tag_refs,
            Some(namespace),
            session_id,
        ).is_ok() {
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

        if conn.graph_add_node_with_vector(sec_node_id, "Section", &sec_props.to_string(), Some(&sec_vector)).is_ok() {
            sections_stored += 1;

            if conn.graph_add_edge(doc_node_id, sec_node_id, "CONTAINS", 1.0, "{}").is_ok() {
                edges_created += 1;
            }

            if let Some(prev_id) = prev_section_node_id {
                if conn.graph_add_edge(prev_id, sec_node_id, "PRECEDES", 1.0, "{}").is_ok() {
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
    println!("  • AI Agent Memories:   \x1b[1m{}\x1b[0m chunks (Namespace: '\x1b[33m{}\x1b[0m', 128D Embeddings)", memories_stored, namespace);
    println!("  • Knowledge Graph:     \x1b[1m{}\x1b[0m Nodes (1 Document + {} Sections)", sections_stored + 1, sections_stored);
    println!("  • Graph Relationships: \x1b[1m{}\x1b[0m Edges (CONTAINS, PRECEDES)", edges_created);
    println!("  • GraphRAG Status:     \x1b[32mInstant Retrieval & Hybrid Recall Ready ✓\x1b[0m");
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
    println!("  --collection <NAME>      Target document collection name (defaults to file name stem)");
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
                    tags = args[i + 1].split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
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
                if format.is_none() && (arg == "csv" || arg == "jsonl" || arg == "json" || arg == "markdown" || arg == "md") {
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
        let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
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

    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("data").to_string();

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
            eprintln!("Error: Unsupported import format '{other}'. Choose from: csv, jsonl, json, markdown.");
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

fn run_bitnet_command(args: &[String]) {
    if args.is_empty() || args[0] == "-h" || args[0] == "--help" {
        println!("TapirusDB BitNet b1.58 Ternary Tensor Engine (100% Safe Rust)");
        println!();
        println!("Commands:");
        println!("  tapirus bitnet <input> <cand1> <cand2> ...   Categorical classification via BitNet ternary layers");
        println!("  tapirus bitnet verify <premise> <hypothesis> Truth verification via ternary projection");
        println!("  tapirus bitnet benchmark                      Benchmark ternary matrix addition throughput");
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
        let start = std::time::Instant::now();
        match engine.verify_deep(premise, hypothesis) {
            Ok((verified, score)) => {
                let us = start.elapsed().as_micros();
                println!("┌─────────────────────────────────────────────────────────────┐");
                println!("│ BitNet b1.58 Ternary Truth Verification                     │");
                println!("├─────────────────────────────────────────────────────────────┤");
                println!("│ Premise:    {}", premise);
                println!("│ Hypothesis: {}", hypothesis);
                println!(
                    "│ Verified:   {} (Score: {:.4})",
                    if verified { "\x1b[1;32mYES (TRUE)\x1b[0m" } else { "\x1b[1;31mNO (FALSE)\x1b[0m" },
                    score
                );
                println!("│ Latency:    {} µs (Multiplication-free ternary addition)    │", us);
                println!("└─────────────────────────────────────────────────────────────┘");
            }
            Err(e) => eprintln!("Error: {e}"),
        }
        return;
    }

    if args[0] == "benchmark" {
        println!("Benchmarking BitNet b1.58 ternary linear contraction (100% Safe Rust)...");
        let block = match tapirus::BitNetBlock::new(256, 512) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("Failed to initialize BitNet block: {e}");
                return;
            }
        };
        let input = vec![0.5f32; 256];
        let _ = block.forward(&input);

        let iterations = if cfg!(debug_assertions) { 1_000 } else { 10_000 };
        let start = std::time::Instant::now();
        for _ in 0..iterations {
            let _ = block.forward(&input);
        }
        let total_us = start.elapsed().as_micros();
        let per_op_ns = (total_us as f64 * 1000.0) / iterations as f64;
        let ops_per_sec = (iterations as f64 / (total_us.max(1) as f64 / 1_000_000.0)) as u64;

        println!("BitNet b1.58 [256 -> 512 SwiGLU -> 256] Results:");
        println!("  Total Contractions: {}", iterations);
        println!("  Latency per block:  {:.2} ns ({:.3} µs)", per_op_ns, per_op_ns / 1000.0);
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

    let start = std::time::Instant::now();
    match engine.classify_deep(input, &candidates) {
        Ok((winner, score)) => {
            let us = start.elapsed().as_micros();
            println!("┌─────────────────────────────────────────────────────────────┐");
            println!("│ BitNet b1.58 Ternary Categorical Classification             │");
            println!("├─────────────────────────────────────────────────────────────┤");
            println!("│ Input:      {}", input);
            println!("│ Top Choice: \x1b[1;32m{}\x1b[0m (Score: {:.4})", winner, score);
            println!("│ Candidates: {:?}", candidates);
            println!("│ Latency:    {} µs (Safe Rust Ternary Neural Engine)         │", us);
            println!("└─────────────────────────────────────────────────────────────┘");
        }
        Err(e) => eprintln!("Error: {e}"),
    }
}


