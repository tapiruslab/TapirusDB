use super::*;

pub(super) fn run_repl(conn: &Connection, target: &str) {
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

pub(super) fn handle_dot_command(conn: &Connection, cmd: &str, target: &str) -> bool {
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

pub(super) fn handle_doc_command(conn: &Connection, parts: &[&str]) {
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

pub(super) fn handle_graph_command(conn: &Connection, parts: &[&str]) {
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

pub(super) fn execute_statement(conn: &Connection, sql: &str) -> bool {
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

pub(super) fn render_ascii_table(rows: &[Row]) {
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

pub(super) fn print_table_ddl(table: &tapirus::sql::catalog::TableDef) {
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

pub(super) fn format_data_type(dt: &DataType) -> String {
    match dt {
        DataType::Integer => "INTEGER".to_string(),
        DataType::Real => "REAL".to_string(),
        DataType::Text => "TEXT".to_string(),
        DataType::Blob => "BLOB".to_string(),
        DataType::Vector(dims) => format!("VECTOR({dims})"),
    }
}

pub(super) fn print_dot_help() {
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

pub(super) fn handle_dump_command(conn: &Connection, parts: &[&str]) {
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

pub(super) fn handle_export_command(conn: &Connection, target_name: &str, output_path: &str) {
    let start = Instant::now();
    let lower = output_path.to_ascii_lowercase();
    let is_lakehouse_fmt = lower.ends_with(".parquet")
        || lower.ends_with(".xlsx")
        || lower.ends_with(".csv")
        || lower.ends_with(".tsv")
        || lower.ends_with(".jsonl")
        || lower.ends_with(".ndjson")
        || lower.ends_with(".sql");

    // 1. Check if it's a relational table
    if let Some(table) = conn.table(target_name) {
        let sql = format!("SELECT * FROM {};", table.name);
        match conn.query(&sql) {
            Ok(rows) => {
                if is_lakehouse_fmt {
                    match tapirus::lakehouse::export_rows_to_path(
                        &rows,
                        &table.name,
                        Path::new(output_path),
                        None,
                        None,
                    ) {
                        Ok(stats) => println!(
                            "Exported {} rows ({} file(s)) from table '{}' to '{}' in {:.2}ms",
                            stats.rows_exported,
                            stats.files_written,
                            target_name,
                            output_path,
                            stats.elapsed_ms
                        ),
                        Err(e) => println!("Export error: {e}"),
                    }
                    return;
                }
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

pub(super) fn import_csv_file(
    conn: &Connection,
    csv_path: &str,
    table_name: &str,
    batch_size: usize,
) {
    match tapirus::lakehouse::csv_stream::ingest_csv_fast(
        conn,
        Path::new(csv_path),
        table_name,
        None,
        batch_size.max(256),
        &[],
    ) {
        Ok(stats) => {
            let rate = if stats.elapsed_ms > 0.0 {
                stats.rows_ingested as f64 / (stats.elapsed_ms / 1000.0)
            } else {
                stats.rows_ingested as f64
            };
            println!(
                "\x1b[1;32m✓\x1b[0m Successfully imported \x1b[1m{}\x1b[0m rows into table '\x1b[36m{}\x1b[0m' in {:.2}ms ({:.0} rows/sec)",
                stats.rows_ingested, stats.target, stats.elapsed_ms, rate
            );
        }
        Err(e) => {
            eprintln!("Import Error: Could not import CSV file '{csv_path}': {e}");
        }
    }
}

pub(super) fn import_jsonl_file(
    conn: &Connection,
    json_path: &str,
    collection_name: &str,
    batch_size: usize,
) {
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
    if trimmed.starts_with('[')
        && let Ok(serde_json::Value::Array(arr)) = serde_json::from_str(trimmed)
    {
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

pub(super) fn import_markdown_file(
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

            if let Some(prev_id) = prev_section_node_id
                && conn
                    .graph_add_edge(prev_id, sec_node_id, "PRECEDES", 1.0, "{}")
                    .is_ok()
            {
                edges_created += 1;
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

pub(super) fn print_import_help() {
    println!("Usage: tapirus import <FORMAT> <FILE_OR_GLOB> [OPTIONS]");
    println!("       tapirus import <FILE_OR_GLOB> [OPTIONS]");
    println!();
    println!("High-throughput streaming & lakehouse importer for TapirusDB.");
    println!("Supports CSV/TSV/.csv.gz, Excel (.xlsx/.xls/.xlsb/.ods), JSON/JSONL,");
    println!("Apache Parquet, SQLite binaries, PostgreSQL/MySQL/SQLite SQL dumps, PDF, and Text.");
    println!();
    println!("Formats:");
    println!("  csv, tsv                 Stream CSV/TSV/.gz into a SQL table (batch encoder)");
    println!(
        "  excel, xlsx, xls, ods    Import Excel / OpenDocument sheets (--source-table for sheet)"
    );
    println!(
        "  parquet                  Stream Apache Parquet (+ Hive partitions & Arrow vectors)"
    );
    println!("  jsonl, json              Import JSON/JSONL to SQL table (or --collection docs)");
    println!("  sqlite, db               Import tables from SQLite3 binary DB (Pure Safe Rust)");
    println!("  sql, dump                Import .sql dump (PostgreSQL COPY/INSERT, MySQL, SQLite)");
    println!(
        "  pdf, txt, log            Extract & chunk PDF/Text into Table, Document, or AI Memory"
    );
    println!("  markdown, md             Chunk Markdown into AI Memory/Graph, Document, or Table");
    println!();
    println!("Common Options:");
    println!("  --db <PATH>              Path to target database file (default: production.tapir)");
    println!("  --passphrase <KEY>       Passphrase for encrypted database (ChaCha20-Poly1305)");
    println!("  --batch <N>              Batch size for transaction commits (default: 2048)");
    println!("  --table <NAME>           Target SQL table name (defaults to file stem)");
    println!("  --source-table <NAME>    Specific source table when reading multi-table SQLite/DB");
    println!("  --delimiter <CHAR>       Explicit field delimiter for CSV/TSV");
    println!("  -h, --help               Print this help message");
    println!();
    println!("JSONL Document & Unstructured Options:");
    println!("  --collection <NAME>      Import JSON/JSONL into a Document collection");
    println!(
        "  --mode <table|doc|ai>    Target store for PDF/Text/Markdown (default: table; md w/ --namespace uses ai)"
    );
    println!("  --chunk-size <N>         Character window when chunking PDF/Text (default: 1200)");
    println!("  --namespace <NAME>       Target AI memory namespace (defaults to file stem)");
    println!("  --session-id <ID>        Optional session ID for agent memory");
    println!("  --tags <TAG1,TAG2,...>   Comma-separated tags for AI memory recall indexing");
    println!();
    println!("Examples:");
    println!("  tapirus import csv sales.csv.gz --table sales --db lake.tapir");
    println!(
        "  tapirus import parquet \"lake/year=2026/*.parquet\" --table events --db lake.tapir"
    );
    println!("  tapirus import sqlite legacy.db --db modern.tapir");
    println!("  tapirus import pdf report.pdf --mode ai --namespace research --db app.tapir");
}
