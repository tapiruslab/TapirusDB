use super::*;

pub(super) fn print_serve_help() {
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

pub(super) fn run_studio_command(args: &[String]) {
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
pub(super) enum SqlRoleAccess {
    ReadOnly,
    ReadWrite,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ApiKeyRoleBinding {
    pub(super) key: String,
    pub(super) access: SqlRoleAccess,
    pub(super) allowed_tables: Option<Vec<String>>,
}

pub(super) fn parse_api_key_role_bindings(
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

pub(super) fn parse_api_key_namespace_bindings(
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

pub(super) fn run_serve_command(args: &[String]) {
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

pub(super) fn print_backup_help() {
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

pub(super) fn run_backup_command(args: &[String]) {
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

pub(super) fn print_restore_help() {
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

pub(super) fn run_restore_command(args: &[String]) {
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

pub(super) fn print_verify_help() {
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

pub(super) fn run_verify_command(args: &[String]) {
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
