#![cfg(feature = "lakehouse")]

use flate2::Compression;
use flate2::write::GzEncoder;
use lopdf::content::{Content, Operation};
use lopdf::{Document as PdfDocument, Object, Stream, dictionary};
use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::time::Instant;
use tapirus::Connection;
use tapirus::lakehouse::{LakeIngestOptions, SqliteFileReader, TextImportMode, convert_dataset};
use tapirus::traits::Value;
use tempfile::TempDir;

#[test]
fn test_batch_insert_speedup_and_correctness() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("batch_bench.tapir");
    let conn = Connection::open(&db_path).unwrap();

    conn.execute(
        "CREATE TABLE sql_items (id INTEGER PRIMARY KEY, category TEXT, amount REAL, code INTEGER);",
    )
    .unwrap();
    conn.execute(
        "CREATE TABLE batch_items (id INTEGER PRIMARY KEY, category TEXT, amount REAL, code INTEGER);",
    )
    .unwrap();

    let sql_n = 1_500usize;
    let t_sql = Instant::now();
    conn.begin_transaction().unwrap();
    for i in 0..sql_n {
        let cat = if i % 3 == 0 {
            "alpha"
        } else if i % 3 == 1 {
            "beta"
        } else {
            "gamma"
        };
        let sql = format!(
            "INSERT INTO sql_items (id, category, amount, code) VALUES ({i}, '{cat}', {:.2}, {});",
            (i as f64) * 1.25,
            i % 17
        );
        conn.execute(&sql).unwrap();
    }
    conn.commit().unwrap();
    let sql_elapsed = t_sql.elapsed().as_secs_f64();
    let sql_rps = (sql_n as f64) / sql_elapsed.max(1e-6);

    let batch_n = 10_000usize;
    let mut batch_rows = Vec::with_capacity(batch_n);
    for i in 0..batch_n {
        let cat = if i % 3 == 0 {
            "alpha"
        } else if i % 3 == 1 {
            "beta"
        } else {
            "gamma"
        };
        batch_rows.push(vec![
            Value::Integer(i as i64),
            Value::Text(cat.to_string()),
            Value::Real((i as f64) * 1.25),
            Value::Integer((i % 17) as i64),
        ]);
    }

    let t_batch = Instant::now();
    let inserted = conn
        .insert_rows_batch("batch_items", None, &batch_rows)
        .unwrap();
    let batch_elapsed = t_batch.elapsed().as_secs_f64();
    let batch_rps = (batch_n as f64) / batch_elapsed.max(1e-6);

    println!(
        "BENCH batch_ingest: sql_insert={sql_rps:.0} rows/sec ({sql_n} rows in {sql_elapsed:.4}s), \
         insert_rows_batch={batch_rps:.0} rows/sec ({batch_n} rows in {batch_elapsed:.4}s), \
         speedup={:.2}x",
        batch_rps / sql_rps.max(1.0)
    );

    assert_eq!(inserted, batch_n);
    let cnt = conn.query("SELECT COUNT(*) FROM batch_items;").unwrap();
    assert_eq!(cnt[0].values()[0], Value::Integer(batch_n as i64));
}

#[test]
fn test_csv_gz_tsv_jsonl_streaming_and_external_scan() {
    let dir = TempDir::new().unwrap();
    let gz_path = dir.path().join("orders.csv.gz");
    let mut csv_buf = String::from("id,city,amount,vec\n");
    csv_buf.push_str("1,\"Kuala Lumpur, MY\",120.5,\"[0.1, 0.2, 0.3]\"\n");
    csv_buf.push_str("2,\"Penang\",85.0,\"[0.4, 0.5, 0.6]\"\n");
    for i in 3..=250 {
        csv_buf.push_str(&format!(
            "{i},\"Johor Bahru\",{:.2},\"[{:.3}, 0.1, 0.2]\"\n",
            (i as f64) * 1.5,
            (i % 10) as f32 / 10.0
        ));
    }
    let file = File::create(&gz_path).unwrap();
    let mut enc = GzEncoder::new(file, Compression::default());
    enc.write_all(csv_buf.as_bytes()).unwrap();
    enc.finish().unwrap();

    let conn = Connection::open_in_memory().unwrap();
    let stats = conn
        .ingest_lakehouse(
            &gz_path.to_string_lossy(),
            None,
            &LakeIngestOptions {
                target_name: Some("orders".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(stats.rows_ingested, 250);
    assert_eq!(stats.columns, 4);

    let r = conn
        .query("SELECT city, amount, vec FROM orders WHERE id = 1;")
        .unwrap();
    assert_eq!(r[0].values()[0], Value::Text("Kuala Lumpur, MY".into()));
    assert_eq!(r[0].values()[1], Value::Real(120.5));
    assert!(matches!(r[0].values()[2], Value::Vector(ref v) if v.len() == 3));

    // Export to TSV and JSONL and verify read_csv / read_jsonl SQL external scans
    let tsv_out = dir.path().join("orders_out.tsv");
    let jsonl_out = dir.path().join("orders_out.jsonl");
    let exp_tsv = conn
        .export_query_to_file(
            "SELECT id, city, amount FROM orders ORDER BY id",
            &tsv_out,
            None,
            None,
        )
        .unwrap();
    assert_eq!(exp_tsv.rows_exported, 250);

    let exp_jsonl = conn
        .export_query_to_file(
            "SELECT id, city, amount FROM orders ORDER BY id",
            &jsonl_out,
            None,
            None,
        )
        .unwrap();
    assert_eq!(exp_jsonl.rows_exported, 250);

    let scan_tsv = conn
        .query(&format!(
            "SELECT city, amount FROM read_csv('{}') LIMIT 2;",
            tsv_out.to_string_lossy().replace('\\', "/")
        ))
        .unwrap();
    assert_eq!(scan_tsv.len(), 2);
    assert_eq!(
        scan_tsv[0].values()[0],
        Value::Text("Kuala Lumpur, MY".into())
    );

    let scan_jsonl = conn
        .query(&format!(
            "SELECT id, city FROM read_jsonl('{}') LIMIT 3;",
            jsonl_out.to_string_lossy().replace('\\', "/")
        ))
        .unwrap();
    assert_eq!(scan_jsonl.len(), 3);
}

#[test]
fn test_parquet_vectors_globs_and_hive_partitions() {
    let dir = TempDir::new().unwrap();
    let conn = Connection::open_in_memory().unwrap();
    conn.execute(
        "CREATE TABLE telemetry (id INTEGER PRIMARY KEY, region TEXT, score REAL, emb VECTOR(4));",
    )
    .unwrap();

    let mut batch = Vec::new();
    for i in 1..=120 {
        let region = if i % 2 == 0 { "apac" } else { "us" };
        batch.push(vec![
            Value::Integer(i),
            Value::Text(region.into()),
            Value::Real((i as f64) * 0.5),
            Value::Vector(vec![0.1, 0.2, 0.3, (i as f32) * 0.01]),
        ]);
    }
    conn.insert_rows_batch("telemetry", None, &batch).unwrap();

    // Single Parquet file roundtrip (including VECTOR(4) -> Arrow FixedSizeList -> Tapirus VECTOR)
    let pq_single = dir.path().join("telemetry.parquet");
    let exp_single = conn
        .export_query_to_file("SELECT * FROM telemetry", &pq_single, None, None)
        .unwrap();
    assert_eq!(exp_single.rows_exported, 120);

    let scanned = conn
        .query(&format!(
            "SELECT COUNT(*), AVG(score) FROM read_parquet('{}');",
            pq_single.to_string_lossy().replace('\\', "/")
        ))
        .unwrap();
    assert_eq!(scanned[0].values()[0], Value::Integer(120));

    // Hive-partitioned Parquet directory export + glob scan
    let lake_dir = dir.path().join("lake_partitioned");
    let exp_part = conn
        .export_query_to_file(
            "SELECT id, region, score FROM telemetry",
            &lake_dir,
            Some("parquet"),
            Some("region"),
        )
        .unwrap();
    assert_eq!(exp_part.files_written, 2);
    assert!(lake_dir.join("region=apac").join("part-0.parquet").exists());
    assert!(lake_dir.join("region=us").join("part-0.parquet").exists());

    let glob_pattern = format!(
        "{}/**/*.parquet",
        lake_dir.to_string_lossy().replace('\\', "/")
    );
    let lake_agg = conn
        .query(&format!(
            "SELECT region, COUNT(*), AVG(score) FROM read_parquet('{glob_pattern}') GROUP BY region ORDER BY region ASC;"
        ))
        .unwrap();
    assert_eq!(lake_agg.len(), 2);
    assert_eq!(lake_agg[0].values()[0], Value::Text("apac".into()));
    assert_eq!(lake_agg[0].values()[1], Value::Integer(60));
    assert_eq!(lake_agg[1].values()[0], Value::Text("us".into()));
    assert_eq!(lake_agg[1].values()[1], Value::Integer(60));
}

#[test]
fn test_pure_rust_sqlite_binary_reader_and_conversion() {
    let dir = TempDir::new().unwrap();
    let sqlite_path = dir.path().join("legacy_app.sqlite");

    // Generate a genuine SQLite 3 binary database using dev-dep `rusqlite`
    // with > 300 rows (forces interior B-Tree pages) + > 5KB text (forces overflow pages)
    let sq_conn = rusqlite::Connection::open(&sqlite_path).unwrap();
    sq_conn
        .execute_batch(
            "CREATE TABLE customers (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                balance REAL,
                payload BLOB,
                notes TEXT
            );",
        )
        .unwrap();
    let long_note = "TapirusDB-Overflow-Check-".repeat(250);
    {
        let tx = sq_conn.unchecked_transaction().unwrap();
        let mut stmt = tx
            .prepare("INSERT INTO customers (id, name, balance, payload, notes) VALUES (?1, ?2, ?3, ?4, ?5)")
            .unwrap();
        for i in 1..=350i64 {
            let name = format!("Pelanggan #{i}");
            let balance = (i as f64) * 12.75 - 100.0;
            let blob = vec![0xCA, 0xFE, (i & 0xFF) as u8];
            let note = if i == 42 {
                Some(long_note.as_str())
            } else if i % 10 == 0 {
                None
            } else {
                Some("standard")
            };
            stmt.execute(rusqlite::params![i, name, balance, blob, note])
                .unwrap();
        }
        drop(stmt);
        tx.commit().unwrap();
    }
    drop(sq_conn);

    // 1. Inspect & decode purely in Safe Rust (zero C SQLite code!)
    let reader = SqliteFileReader::open(&sqlite_path).unwrap();
    let tables = reader.list_tables().unwrap();
    assert_eq!(tables.len(), 1);
    assert_eq!(tables[0].name, "customers");
    let rows = reader.read_table_rows(&tables[0]).unwrap();
    assert_eq!(rows.len(), 350);

    let row42 = rows
        .iter()
        .find(|r| r.values()[0] == Value::Integer(42))
        .unwrap();
    assert_eq!(row42.values()[1], Value::Text("Pelanggan #42".into()));
    assert_eq!(row42.values()[4], Value::Text(long_note.clone()));

    let row10 = rows
        .iter()
        .find(|r| r.values()[0] == Value::Integer(10))
        .unwrap();
    assert_eq!(row10.values()[4], Value::Null);

    // 2. Direct SQL external query via read_sqlite()
    let tapir_conn = Connection::open_in_memory().unwrap();
    let sq_res = tapir_conn
        .query(&format!(
            "SELECT COUNT(*), AVG(balance) FROM read_sqlite('{}', 'customers');",
            sqlite_path.to_string_lossy().replace('\\', "/")
        ))
        .unwrap();
    assert_eq!(sq_res[0].values()[0], Value::Integer(350));

    // 3. Universal convert_dataset: SQLite -> .tapir AND SQLite -> .parquet
    let tapir_out = dir.path().join("migrated.tapir");
    let (ing, _) = convert_dataset(
        &sqlite_path.to_string_lossy(),
        &tapir_out,
        None,
        None,
        None,
        &LakeIngestOptions::default(),
        None,
    )
    .unwrap();
    assert_eq!(ing.rows_ingested, 350);

    let reopened = Connection::open(&tapir_out).unwrap();
    let migrated_cnt = reopened.query("SELECT COUNT(*) FROM customers;").unwrap();
    assert_eq!(migrated_cnt[0].values()[0], Value::Integer(350));

    let pq_out = dir.path().join("customers.parquet");
    let (_, exp) = convert_dataset(
        &sqlite_path.to_string_lossy(),
        &pq_out,
        None,
        None,
        None,
        &LakeIngestOptions {
            source_table: Some("customers".into()),
            ..Default::default()
        },
        None,
    )
    .unwrap();
    assert_eq!(exp.unwrap().rows_exported, 350);
    assert!(pq_out.exists());
}

#[test]
fn test_sql_dump_postgres_copy_mysql_sqlite_roundtrip() {
    let dir = TempDir::new().unwrap();
    let dump_path = dir.path().join("mixed_dump.sql");
    let sql_dump = r#"
CREATE TABLE pg_users (
    id INT PRIMARY KEY,
    username VARCHAR(64),
    score DOUBLE PRECISION
);

INSERT INTO pg_users (id, username, score) VALUES
    (1, 'aisyah', 98.5),
    (2, 'farid', 87.25),
    (3, 'lin''da', 91.0);

CREATE TABLE pg_logs (
    id BIGINT PRIMARY KEY,
    service TEXT,
    latency_ms FLOAT
);

COPY pg_logs (id, service, latency_ms) FROM stdin;
101	auth	12.4
102	billing	45.8
103	search	\N
\.
"#;
    std::fs::write(&dump_path, sql_dump).unwrap();

    let conn = Connection::open_in_memory().unwrap();
    let stats = conn
        .ingest_lakehouse(
            &dump_path.to_string_lossy(),
            None,
            &LakeIngestOptions::default(),
        )
        .unwrap();
    assert_eq!(stats.rows_ingested, 6);

    let u_rows = conn
        .query("SELECT username FROM pg_users WHERE id = 3;")
        .unwrap();
    assert_eq!(u_rows[0].values()[0], Value::Text("lin'da".into()));

    let l_rows = conn
        .query("SELECT service, latency_ms FROM pg_logs WHERE id = 103;")
        .unwrap();
    assert_eq!(l_rows[0].values()[0], Value::Text("search".into()));
    assert_eq!(l_rows[0].values()[1], Value::Null);

    // Export pg_users back to .sql dump & re-import into a fresh DB
    let out_dump = dir.path().join("reexport.sql");
    conn.export_query_to_file(
        "SELECT id, username, score FROM pg_users ORDER BY id",
        &out_dump,
        None,
        None,
    )
    .unwrap();

    let fresh = Connection::open_in_memory().unwrap();
    let re = fresh
        .ingest_lakehouse(
            &out_dump.to_string_lossy(),
            None,
            &LakeIngestOptions::default(),
        )
        .unwrap();
    assert_eq!(re.rows_ingested, 3);
}

fn build_sample_pdf(path: &Path) {
    let mut doc = PdfDocument::with_version("1.5");
    let pages_id = doc.new_object_id();
    let font_id = doc.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Helvetica",
    });
    let resources_id = doc.add_object(dictionary! {
        "Font" => dictionary! {
            "F1" => font_id,
        },
    });

    let p1_content = Content {
        operations: vec![
            Operation::new("BT", vec![]),
            Operation::new("Tf", vec!["F1".into(), 12.into()]),
            Operation::new("Td", vec![50.into(), 700.into()]),
            Operation::new(
                "Tj",
                vec![Object::string_literal(
                    "TapirusDB Data Lake PDF Report Page One",
                )],
            ),
            Operation::new("ET", vec![]),
        ],
    };
    let p1_stream = doc.add_object(Stream::new(dictionary! {}, p1_content.encode().unwrap()));
    let p1_id = doc.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "Contents" => p1_stream,
    });

    let p2_content = Content {
        operations: vec![
            Operation::new("BT", vec![]),
            Operation::new("Tf", vec!["F1".into(), 12.into()]),
            Operation::new("Td", vec![50.into(), 700.into()]),
            Operation::new(
                "Tj",
                vec![Object::string_literal(
                    "Quarterly Revenue Analytics Page Two",
                )],
            ),
            Operation::new("ET", vec![]),
        ],
    };
    let p2_stream = doc.add_object(Stream::new(dictionary! {}, p2_content.encode().unwrap()));
    let p2_id = doc.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "Contents" => p2_stream,
    });

    let pages = dictionary! {
        "Type" => "Pages",
        "Kids" => vec![p1_id.into(), p2_id.into()],
        "Count" => 2,
        "Resources" => resources_id,
        "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
    };
    doc.objects.insert(pages_id, Object::Dictionary(pages));
    let catalog_id = doc.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    });
    doc.trailer.set("Root", catalog_id);
    doc.save(path).unwrap();
}

#[test]
fn test_pdf_and_markdown_multi_mode_ingestion() {
    let dir = TempDir::new().unwrap();
    let pdf_path = dir.path().join("report.pdf");
    let md_path = dir.path().join("architecture.md");

    build_sample_pdf(&pdf_path);
    std::fs::write(
        &md_path,
        "# Overview\nTapirusDB stores tables, documents, vectors and graphs.\n\n# Data Lake\nSupports Apache Parquet and CSV streams.\n",
    )
    .unwrap();

    let conn = Connection::open_in_memory().unwrap();

    // 1. Direct SQL scan of PDF and Markdown/Text
    let pdf_scan = conn
        .query(&format!(
            "SELECT page, content FROM read_pdf('{}');",
            pdf_path.to_string_lossy().replace('\\', "/")
        ))
        .unwrap();
    assert_eq!(pdf_scan.len(), 2);
    assert!(matches!(&pdf_scan[0].values()[1], Value::Text(s) if s.contains("Page One")));
    assert!(matches!(&pdf_scan[1].values()[1], Value::Text(s) if s.contains("Page Two")));

    let txt_scan = conn
        .query(&format!(
            "SELECT page, content FROM read_text('{}');",
            md_path.to_string_lossy().replace('\\', "/")
        ))
        .unwrap();
    assert_eq!(txt_scan.len(), 2);

    // 2. Table, Document, and AiMemory ingestion modes
    let tbl_stats = conn
        .ingest_lakehouse(
            &pdf_path.to_string_lossy(),
            None,
            &LakeIngestOptions {
                target_name: Some("pdf_chunks".into()),
                text_mode: TextImportMode::Table,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(tbl_stats.rows_ingested, 2);

    let doc_stats = conn
        .ingest_lakehouse(
            &pdf_path.to_string_lossy(),
            None,
            &LakeIngestOptions {
                target_name: Some("pdf_docs".into()),
                text_mode: TextImportMode::Document,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(doc_stats.rows_ingested, 2);
    assert_eq!(conn.collection("pdf_docs").unwrap().count().unwrap(), 2);

    let ai_stats = conn
        .ingest_lakehouse(
            &pdf_path.to_string_lossy(),
            None,
            &LakeIngestOptions {
                target_name: Some("pdf_mem".into()),
                text_mode: TextImportMode::AiMemory,
                tags: vec!["finance".into(), "report".into()],
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(ai_stats.rows_ingested, 2);
}

#[test]
fn test_vectorized_analytics_stddev_median_percentile_groupby_and_copy() {
    let dir = TempDir::new().unwrap();
    let conn = Connection::open_in_memory().unwrap();
    conn.execute(
        "CREATE TABLE metrics (id INTEGER PRIMARY KEY, region TEXT, err_code INTEGER, latency_ms REAL);",
    )
    .unwrap();

    // Known values: for "east" we push 10..=50 (step 10) * repeated so we can check exact stats:
    // Distinct values = 5; Values = [10, 20, 30, 40, 50] -> mean=30, median=30, sample var=250, sample stddev=sqrt(250)=15.811388...
    let mut rows = Vec::new();
    let mut id = 1i64;
    for r in 0..200 {
        for v in [10.0, 20.0, 30.0, 40.0, 50.0] {
            rows.push(vec![
                Value::Integer(id),
                Value::Text("east".into()),
                Value::Integer((v as i64) + (r % 2) * 100), // 10 unique err_codes
                Value::Real(v),
            ]);
            id += 1;
        }
    }
    for r in 0..100 {
        for v in [100.0, 200.0, 300.0] {
            rows.push(vec![
                Value::Integer(id),
                Value::Text("west".into()),
                Value::Integer(r % 3),
                Value::Real(v),
            ]);
            id += 1;
        }
    }
    conn.insert_rows_batch("metrics", None, &rows).unwrap();

    let agg = conn
        .query(
            "SELECT region, COUNT(*), COUNT(DISTINCT err_code), AVG(latency_ms), STDDEV(latency_ms), VARIANCE(latency_ms), MEDIAN(latency_ms), PERCENTILE(latency_ms, 0.90) FROM metrics GROUP BY region ORDER BY region ASC;",
        )
        .unwrap();
    assert_eq!(agg.len(), 2);

    // "east"
    assert_eq!(agg[0].values()[0], Value::Text("east".into()));
    assert_eq!(agg[0].values()[1], Value::Integer(1000));
    assert_eq!(agg[0].values()[2], Value::Integer(10));
    let east_avg = match agg[0].values()[3] {
        Value::Real(v) => v,
        _ => panic!("expected real avg"),
    };
    assert!((east_avg - 30.0).abs() < 1e-6);

    let east_sd = match agg[0].values()[4] {
        Value::Real(v) => v,
        _ => panic!("expected real stddev"),
    };
    // Sample stddev over 1000 samples equally split across {10,20,30,40,50}: sumsq = 200*(400+100+0+100+400)=200000, /999 = 200.2002, sqrt = 14.149212
    assert!((east_sd - 14.149212).abs() < 1e-4, "east_sd={east_sd}");

    let east_med = match agg[0].values()[6] {
        Value::Real(v) => v,
        _ => panic!("expected real median"),
    };
    assert!((east_med - 30.0).abs() < 1e-6);

    let east_p90 = match agg[0].values()[7] {
        Value::Real(v) => v,
        _ => panic!("expected real p90"),
    };
    assert!((east_p90 - 50.0).abs() < 1e-6);

    // SQL COPY TO & COPY FROM roundtrip
    let pq_copy = dir.path().join("east_metrics.parquet");
    conn.execute(&format!(
        "COPY (SELECT id, region, err_code, latency_ms FROM metrics WHERE region = 'east') TO '{}';",
        pq_copy.to_string_lossy().replace('\\', "/")
    ))
    .unwrap();
    assert!(pq_copy.exists());

    conn.execute(&format!(
        "COPY metrics_copy FROM '{}';",
        pq_copy.to_string_lossy().replace('\\', "/")
    ))
    .unwrap();
    let copy_cnt = conn.query("SELECT COUNT(*) FROM metrics_copy;").unwrap();
    assert_eq!(copy_cnt[0].values()[0], Value::Integer(1000));
}

#[test]
fn test_excel_xlsx_roundtrip_and_sql_scan() {
    let dir = TempDir::new().unwrap();
    let xlsx_path = dir.path().join("workbook.xlsx");

    // Build a multi-sheet .xlsx workbook natively in Pure Rust via rust_xlsxwriter
    {
        let mut wb = rust_xlsxwriter::Workbook::new();
        let s1 = wb.add_worksheet();
        s1.set_name("sales").unwrap();
        s1.write_string(0, 0, "id").unwrap();
        s1.write_string(0, 1, "item").unwrap();
        s1.write_string(0, 2, "revenue").unwrap();
        s1.write_string(0, 3, "emb").unwrap();

        s1.write_number(1, 0, 1.0).unwrap();
        s1.write_string(1, 1, "Router").unwrap();
        s1.write_number(1, 2, 199.5).unwrap();
        s1.write_string(1, 3, "[0.1, 0.2, 0.3]").unwrap();

        s1.write_number(2, 0, 2.0).unwrap();
        s1.write_string(2, 1, "Switch").unwrap();
        s1.write_number(2, 2, 350.25).unwrap();
        s1.write_string(2, 3, "[0.4, 0.5, 0.6]").unwrap();

        let s2 = wb.add_worksheet();
        s2.set_name("regions").unwrap();
        s2.write_string(0, 0, "code").unwrap();
        s2.write_string(0, 1, "label").unwrap();
        s2.write_string(1, 0, "MY").unwrap();
        s2.write_string(1, 1, "Malaysia").unwrap();

        wb.save(&xlsx_path).unwrap();
    }

    let conn = Connection::open_in_memory().unwrap();

    // 1. Direct SQL scan of default sheet and named sheet via read_excel()
    let scan_default = conn
        .query(&format!(
            "SELECT item, revenue, emb FROM read_excel('{}');",
            xlsx_path.to_string_lossy().replace('\\', "/")
        ))
        .unwrap();
    assert_eq!(scan_default.len(), 2);
    assert_eq!(scan_default[0].values()[0], Value::Text("Router".into()));
    assert_eq!(scan_default[0].values()[1], Value::Real(199.5));
    assert!(matches!(scan_default[0].values()[2], Value::Vector(ref v) if v.len() == 3));

    let scan_sheet2 = conn
        .query(&format!(
            "SELECT code, label FROM read_excel('{}', 'regions');",
            xlsx_path.to_string_lossy().replace('\\', "/")
        ))
        .unwrap();
    assert_eq!(scan_sheet2.len(), 1);
    assert_eq!(scan_sheet2[0].values()[0], Value::Text("MY".into()));

    // 2. Multi-sheet ingest into TapirusDB tables
    let ingest = conn
        .ingest_lakehouse(
            &xlsx_path.to_string_lossy(),
            None,
            &LakeIngestOptions::default(),
        )
        .unwrap();
    assert_eq!(ingest.rows_ingested, 3);
    assert_eq!(
        conn.query("SELECT COUNT(*) FROM sales;").unwrap()[0].values()[0],
        Value::Integer(2)
    );
    assert_eq!(
        conn.query("SELECT COUNT(*) FROM regions;").unwrap()[0].values()[0],
        Value::Integer(1)
    );

    // 3. Convert Excel -> Parquet and export SQL query -> new .xlsx roundtrip
    let pq_out = dir.path().join("sales_from_excel.parquet");
    let (_, exp_pq) = convert_dataset(
        &xlsx_path.to_string_lossy(),
        &pq_out,
        None,
        None,
        None,
        &LakeIngestOptions {
            source_table: Some("sales".into()),
            ..Default::default()
        },
        None,
    )
    .unwrap();
    assert_eq!(exp_pq.unwrap().rows_exported, 2);

    let xlsx_out = dir.path().join("exported.xlsx");
    conn.execute(&format!(
        "COPY (SELECT id, item, revenue FROM sales ORDER BY id) TO '{}';",
        xlsx_out.to_string_lossy().replace('\\', "/")
    ))
    .unwrap();
    assert!(xlsx_out.exists());

    let rescan = conn
        .query(&format!(
            "SELECT SUM(revenue) FROM read_excel('{}');",
            xlsx_out.to_string_lossy().replace('\\', "/")
        ))
        .unwrap();
    let total_rev = match rescan[0].values()[0] {
        Value::Real(v) => v,
        _ => panic!("expected real sum"),
    };
    assert!((total_rev - 549.75).abs() < 1e-6);
}
