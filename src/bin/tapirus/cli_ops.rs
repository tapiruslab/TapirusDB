use super::*;

pub(super) fn open_cli_connection(db_path: &str, passphrase: Option<&str>) -> Connection {
    if let Some(pass) = passphrase {
        match Connection::open_encrypted(Path::new(db_path), pass) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("Error opening encrypted database at '{db_path}': {e}");
                std::process::exit(1);
            }
        }
    } else {
        match Connection::open(Path::new(db_path)) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("Error opening database at '{db_path}': {e}");
                std::process::exit(1);
            }
        }
    }
}

pub(super) fn is_known_import_format(token: &str) -> bool {
    matches!(
        token.to_ascii_lowercase().as_str(),
        "csv"
            | "tsv"
            | "jsonl"
            | "ndjson"
            | "json"
            | "parquet"
            | "pq"
            | "excel"
            | "xlsx"
            | "xls"
            | "xlsb"
            | "xlsm"
            | "ods"
            | "sqlite"
            | "sqlite3"
            | "db"
            | "sql"
            | "dump"
            | "pdf"
            | "txt"
            | "text"
            | "log"
            | "markdown"
            | "md"
    )
}

pub(super) fn run_import_command(args: &[String]) {
    if args.is_empty() || args[0] == "-h" || args[0] == "--help" {
        print_import_help();
        return;
    }

    let mut format: Option<String> = None;
    let mut file_path: Option<String> = None;
    let mut db_path = "production.tapir".to_string();
    let mut table_name: Option<String> = None;
    let mut source_table: Option<String> = None;
    let mut collection_name: Option<String> = None;
    let mut namespace: Option<String> = None;
    let mut session_id: Option<String> = None;
    let mut tags = Vec::new();
    let mut passphrase: Option<String> = None;
    let mut batch_size = 2048usize;
    let mut delimiter: Option<char> = None;
    let mut chunk_size = 1200usize;
    let mut text_mode_explicit: Option<tapirus::lakehouse::TextImportMode> = None;

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
            "--source-table" => {
                if i + 1 < args.len() {
                    source_table = Some(args[i + 1].clone());
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
                    batch_size = args[i + 1].parse().unwrap_or(2048);
                    i += 1;
                }
            }
            "--delimiter" => {
                if i + 1 < args.len() {
                    let raw = &args[i + 1];
                    delimiter = if raw == "\\t" || raw.eq_ignore_ascii_case("tab") {
                        Some('\t')
                    } else {
                        raw.chars().next()
                    };
                    i += 1;
                }
            }
            "--chunk-size" => {
                if i + 1 < args.len() {
                    chunk_size = args[i + 1].parse().unwrap_or(1200).max(128);
                    i += 1;
                }
            }
            "--mode" => {
                if i + 1 < args.len() {
                    text_mode_explicit = match args[i + 1].to_ascii_lowercase().as_str() {
                        "table" | "sql" => Some(tapirus::lakehouse::TextImportMode::Table),
                        "doc" | "document" => Some(tapirus::lakehouse::TextImportMode::Document),
                        "ai" | "memory" => Some(tapirus::lakehouse::TextImportMode::AiMemory),
                        other => {
                            eprintln!("Error: Unknown --mode '{other}'. Choose: table, doc, ai.");
                            std::process::exit(1);
                        }
                    };
                    i += 1;
                }
            }
            arg if !arg.starts_with('-') => {
                if format.is_none() && is_known_import_format(arg) {
                    format = Some(arg.to_ascii_lowercase());
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
            eprintln!("Error: Target file or glob pattern to import is required.");
            println!();
            print_import_help();
            std::process::exit(1);
        }
    };

    let parsed_fmt =
        match tapirus::lakehouse::DataFormat::parse_format_or_path(format.as_deref(), &file_path) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        };

    let conn = open_cli_connection(&db_path, passphrase.as_deref());
    let p = Path::new(&file_path);
    let stem = p
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("data")
        .to_string();

    // Backwards-compatible paths when explicit Document/Memory GraphRAG flags are given
    if parsed_fmt == tapirus::lakehouse::DataFormat::Jsonl && collection_name.is_some() {
        let col = collection_name.unwrap_or(stem);
        import_jsonl_file(&conn, &file_path, &col, batch_size);
        return;
    }
    if parsed_fmt == tapirus::lakehouse::DataFormat::Markdown
        && (namespace.is_some() || session_id.is_some())
        && text_mode_explicit.is_none()
    {
        let ns = namespace.unwrap_or(stem);
        import_markdown_file(&conn, &file_path, &ns, session_id.as_deref(), &tags);
        return;
    }

    let text_mode = text_mode_explicit.unwrap_or_else(|| {
        if collection_name.is_some() {
            tapirus::lakehouse::TextImportMode::Document
        } else if namespace.is_some() {
            tapirus::lakehouse::TextImportMode::AiMemory
        } else {
            tapirus::lakehouse::TextImportMode::Table
        }
    });

    let target_name = table_name.or(collection_name).or(namespace);
    let opts = tapirus::lakehouse::LakeIngestOptions {
        target_name,
        source_table,
        batch_size,
        delimiter,
        text_mode,
        chunk_size,
        chunk_overlap: (chunk_size / 8).min(250),
        tags,
    };

    match tapirus::lakehouse::ingest_into_connection(&conn, &file_path, format.as_deref(), &opts) {
        Ok(stats) => {
            let rate = if stats.elapsed_ms > 0.0 {
                stats.rows_ingested as f64 / (stats.elapsed_ms / 1000.0)
            } else {
                stats.rows_ingested as f64
            };
            println!(
                "\x1b[1;32m✓\x1b[0m Ingested \x1b[1m{}\x1b[0m rows ({} files, {} cols) into '\x1b[36m{}\x1b[0m' in {:.2}ms ({:.0} rows/sec)",
                stats.rows_ingested,
                stats.files_processed,
                stats.columns,
                stats.target,
                stats.elapsed_ms,
                rate
            );
        }
        Err(e) => {
            eprintln!("Import Error: {e}");
            std::process::exit(1);
        }
    }
}

pub(super) fn run_export_command(args: &[String]) {
    if args.len() < 2 || args[0] == "-h" || args[0] == "--help" {
        println!(
            "Usage: tapirus export <TABLE_OR_SQL> <OUTPUT_PATH> [--db <DB>] [--format <FMT>] [--partition-by <COL>] [--passphrase <KEY>]"
        );
        return;
    }

    let mut target_or_sql: Option<String> = None;
    let mut output_path: Option<String> = None;
    let mut db_path = "production.tapir".to_string();
    let mut format: Option<String> = None;
    let mut partition_by: Option<String> = None;
    let mut passphrase: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--db" if i + 1 < args.len() => {
                db_path = args[i + 1].clone();
                i += 1;
            }
            "--format" | "--to" if i + 1 < args.len() => {
                format = Some(args[i + 1].clone());
                i += 1;
            }
            "--partition-by" if i + 1 < args.len() => {
                partition_by = Some(args[i + 1].clone());
                i += 1;
            }
            "--passphrase" if i + 1 < args.len() => {
                passphrase = Some(args[i + 1].clone());
                i += 1;
            }
            arg if !arg.starts_with('-') => {
                if target_or_sql.is_none() {
                    target_or_sql = Some(arg.to_string());
                } else if output_path.is_none() {
                    output_path = Some(arg.to_string());
                }
            }
            _ => {}
        }
        i += 1;
    }

    let (Some(target), Some(out)) = (target_or_sql, output_path) else {
        eprintln!("Error: Both <TABLE_OR_SQL> and <OUTPUT_PATH> are required.");
        std::process::exit(1);
    };

    let conn = open_cli_connection(&db_path, passphrase.as_deref());
    let upper = target.trim().to_ascii_uppercase();
    let sql = if upper.starts_with("SELECT ") || upper.starts_with("WITH ") {
        target.clone()
    } else {
        format!("SELECT * FROM {target};")
    };

    let table_label = if upper.starts_with("SELECT ") || upper.starts_with("WITH ") {
        "export_result"
    } else {
        target.as_str()
    };

    let rows = match conn.query(&sql) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Export query error: {e}");
            std::process::exit(1);
        }
    };

    match tapirus::lakehouse::export_rows_to_path(
        &rows,
        table_label,
        Path::new(&out),
        format.as_deref(),
        partition_by.as_deref(),
    ) {
        Ok(stats) => println!(
            "\x1b[1;32m✓\x1b[0m Exported \x1b[1m{}\x1b[0m rows ({} file(s)) to '{}' in {:.2}ms",
            stats.rows_exported, stats.files_written, out, stats.elapsed_ms
        ),
        Err(e) => {
            eprintln!("Export Error: {e}");
            std::process::exit(1);
        }
    }
}

pub(super) fn run_convert_command(args: &[String]) {
    if args.len() < 2 || args[0] == "-h" || args[0] == "--help" {
        println!(
            "Usage: tapirus convert <SOURCE_OR_GLOB> <DEST_PATH> [--from <FMT>] [--to <FMT>] [--table <NAME>] [--source-table <NAME>] [--partition-by <COL>] [--mode <table|doc|ai>] [--passphrase <KEY>]"
        );
        println!(
            "Converts between CSV/TSV/.gz, JSONL, Parquet, SQLite (.db/.sqlite), SQL dumps, PDF, Text/MD, and .tapir."
        );
        return;
    }

    let mut src: Option<String> = None;
    let mut dest: Option<String> = None;
    let mut src_fmt: Option<String> = None;
    let mut dest_fmt: Option<String> = None;
    let mut table_name: Option<String> = None;
    let mut source_table: Option<String> = None;
    let mut partition_by: Option<String> = None;
    let mut passphrase: Option<String> = None;
    let mut text_mode = tapirus::lakehouse::TextImportMode::Table;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--from" if i + 1 < args.len() => {
                src_fmt = Some(args[i + 1].clone());
                i += 1;
            }
            "--to" if i + 1 < args.len() => {
                dest_fmt = Some(args[i + 1].clone());
                i += 1;
            }
            "--table" if i + 1 < args.len() => {
                table_name = Some(args[i + 1].clone());
                i += 1;
            }
            "--source-table" if i + 1 < args.len() => {
                source_table = Some(args[i + 1].clone());
                i += 1;
            }
            "--partition-by" if i + 1 < args.len() => {
                partition_by = Some(args[i + 1].clone());
                i += 1;
            }
            "--passphrase" if i + 1 < args.len() => {
                passphrase = Some(args[i + 1].clone());
                i += 1;
            }
            "--mode" if i + 1 < args.len() => {
                text_mode = match args[i + 1].to_ascii_lowercase().as_str() {
                    "doc" | "document" => tapirus::lakehouse::TextImportMode::Document,
                    "ai" | "memory" => tapirus::lakehouse::TextImportMode::AiMemory,
                    _ => tapirus::lakehouse::TextImportMode::Table,
                };
                i += 1;
            }
            arg if !arg.starts_with('-') => {
                if src.is_none() {
                    src = Some(arg.to_string());
                } else if dest.is_none() {
                    dest = Some(arg.to_string());
                }
            }
            _ => {}
        }
        i += 1;
    }

    let (Some(src), Some(dest)) = (src, dest) else {
        eprintln!("Error: Both <SOURCE> and <DEST> are required.");
        std::process::exit(1);
    };

    let opts = tapirus::lakehouse::LakeIngestOptions {
        target_name: table_name,
        source_table,
        text_mode,
        ..Default::default()
    };

    match tapirus::lakehouse::convert_dataset(
        &src,
        Path::new(&dest),
        src_fmt.as_deref(),
        dest_fmt.as_deref(),
        passphrase.as_deref(),
        &opts,
        partition_by.as_deref(),
    ) {
        Ok((ingest, export_opt)) => {
            if let Some(exp) = export_opt {
                println!(
                    "\x1b[1;32m✓\x1b[0m Converted '{}' -> '{}' ({} rows, {} file(s)) in {:.2}ms",
                    src,
                    dest,
                    exp.rows_exported,
                    exp.files_written,
                    ingest.elapsed_ms + exp.elapsed_ms
                );
            } else {
                println!(
                    "\x1b[1;32m✓\x1b[0m Converted '{}' -> '{}' (table '{}', {} rows) in {:.2}ms",
                    src, dest, ingest.target, ingest.rows_ingested, ingest.elapsed_ms
                );
            }
        }
        Err(e) => {
            eprintln!("Convert Error: {e}");
            std::process::exit(1);
        }
    }
}

pub(super) fn print_lake_help() {
    println!("Usage:");
    println!(
        "  tapirus lake scan <PATH_OR_GLOB> [--format <FMT>] [--table <SRC_TBL>] [--limit <N>]"
    );
    println!("  tapirus lake ingest <PATH_OR_GLOB> --db <DB> [--table <NAME>] [--batch <N>]");
    println!("  tapirus lake export <TABLE_OR_SQL> <DEST> --db <DB> [--partition-by <COL>]");
}

pub(super) fn run_lake_command(args: &[String]) {
    if args.is_empty() || args[0] == "-h" || args[0] == "--help" {
        print_lake_help();
        return;
    }
    match args[0].as_str() {
        "scan" => {
            let mut path: Option<String> = None;
            let mut format: Option<String> = None;
            let mut src_table: Option<String> = None;
            let mut limit = Some(25usize);
            let mut i = 1usize;
            while i < args.len() {
                match args[i].as_str() {
                    "--format" if i + 1 < args.len() => {
                        format = Some(args[i + 1].clone());
                        i += 1;
                    }
                    "--table" | "--source-table" if i + 1 < args.len() => {
                        src_table = Some(args[i + 1].clone());
                        i += 1;
                    }
                    "--limit" if i + 1 < args.len() => {
                        limit = args[i + 1].parse().ok();
                        i += 1;
                    }
                    arg if !arg.starts_with('-') && path.is_none() => {
                        path = Some(arg.to_string());
                    }
                    _ => {}
                }
                i += 1;
            }
            let Some(path) = path else {
                eprintln!("Error: <PATH_OR_GLOB> required for 'tapirus lake scan'.");
                std::process::exit(1);
            };
            let inferred_fmt =
                format.unwrap_or_else(
                    || match tapirus::lakehouse::DataFormat::parse_format_or_path(None, &path) {
                        Ok(tapirus::lakehouse::DataFormat::Parquet) => "parquet".to_string(),
                        Ok(tapirus::lakehouse::DataFormat::Excel) => "excel".to_string(),
                        Ok(tapirus::lakehouse::DataFormat::Tsv) => "tsv".to_string(),
                        Ok(tapirus::lakehouse::DataFormat::Jsonl) => "jsonl".to_string(),
                        Ok(tapirus::lakehouse::DataFormat::Sqlite) => "sqlite".to_string(),
                        Ok(tapirus::lakehouse::DataFormat::Pdf) => "pdf".to_string(),
                        Ok(tapirus::lakehouse::DataFormat::Text)
                        | Ok(tapirus::lakehouse::DataFormat::Markdown) => "text".to_string(),
                        _ => "csv".to_string(),
                    },
                );
            match tapirus::lakehouse::scan_external_rows(
                &inferred_fmt,
                &path,
                src_table.as_deref(),
                limit,
            ) {
                Ok(rows) => {
                    println!(
                        "Scanned {} rows from '{}' ({})",
                        rows.len(),
                        path,
                        inferred_fmt
                    );
                    render_ascii_table(&rows);
                }
                Err(e) => {
                    eprintln!("Lake scan error: {e}");
                    std::process::exit(1);
                }
            }
        }
        "ingest" => run_import_command(&args[1..]),
        "export" => run_export_command(&args[1..]),
        other => {
            eprintln!("Unknown 'tapirus lake' subcommand '{other}'.");
            print_lake_help();
            std::process::exit(1);
        }
    }
}

pub(super) fn handle_import_csv(conn: &Connection, csv_path: &str, table_name: &str) {
    import_csv_file(conn, csv_path, table_name, 500);
}

pub(super) fn handle_memory_command(conn: &Connection, parts: &[&str]) {
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

pub(super) fn run_grep_command(args: &[String]) {
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
            if let Some(name) = path.file_name().and_then(|n| n.to_str())
                && (name.starts_with('.')
                    || name == "target"
                    || name == "node_modules"
                    || name == "dist"
                    || name == "__pycache__")
            {
                continue;
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
                        if let (Some(emb), Some(qv)) = (embedder, query_vector)
                            && line.trim().len() > 3
                        {
                            let lv = emb.embed_text(&line);
                            let dist = tapirus::vector::simd::simd_cosine_distance(qv, &lv);
                            vector_sim = (1.0 - dist).max(0.0);
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

pub(super) fn run_bitnet_command(args: &[String]) {
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
