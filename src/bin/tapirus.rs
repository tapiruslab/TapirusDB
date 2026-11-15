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

#[path = "tapirus/admin.rs"]
mod admin;
#[path = "tapirus/chatbot.rs"]
mod chatbot;
#[path = "tapirus/cli_ops.rs"]
mod cli_ops;
#[path = "tapirus/http.rs"]
mod http;
#[path = "tapirus/mcp.rs"]
mod mcp;
#[path = "tapirus/repl.rs"]
mod repl;

#[cfg(test)]
#[path = "tapirus/tests.rs"]
mod audit_regression_tests;

use admin::*;
use chatbot::*;
use cli_ops::*;
use http::*;
use mcp::*;
use repl::*;

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

    // Check if sub-command is "export"
    if args.len() > 1 && args[1] == "export" {
        run_export_command(&args[2..]);
        return;
    }

    // Check if sub-command is "convert"
    if args.len() > 1 && args[1] == "convert" {
        run_convert_command(&args[2..]);
        return;
    }

    // Check if sub-command is "lake" or "lakehouse"
    if args.len() > 1 && (args[1] == "lake" || args[1] == "lakehouse") {
        run_lake_command(&args[2..]);
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
        "  tapirus import <FORMAT> <FILE> [OPTIONS]             High-speed importer (CSV/Excel/Parquet/SQLite/SQL/PDF/Text)"
    );
    println!(
        "  tapirus export <TARGET> <OUT> [OPTIONS]              Export table/query to Parquet, Excel, CSV, TSV, JSONL, SQL"
    );
    println!(
        "  tapirus convert <SRC> <DEST> [OPTIONS]               Universal converter (CSV/Excel/Parquet/SQLite/SQL/PDF <-> .tapir)"
    );
    println!(
        "  tapirus lake <scan|ingest|export> [OPTIONS]          Data Lake globs, Hive partitions & analytics"
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
