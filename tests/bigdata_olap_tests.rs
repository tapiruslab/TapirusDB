//! Integration tests for the Big Data / Columnar Parquet OLAP pushdown engine.

#![cfg(all(feature = "lakehouse", feature = "olap"))]

use arrow_array::RecordBatch;
use arrow_array::array::{Float64Array, Int64Array, StringArray};
use arrow_schema::{DataType, Field, Schema};
use parquet::arrow::ArrowWriter;
use parquet::file::properties::WriterProperties;
use std::fs::{self, File};
use std::path::Path;
use std::sync::Arc;
use tapirus::{Connection, Value};

fn write_partition_parquet(path: &Path, start_id: i64, count: i64, tag: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("amount", DataType::Int64, false),
        Field::new("score", DataType::Float64, false),
        Field::new("cat", DataType::Utf8, false),
    ]));

    let props = WriterProperties::builder()
        .set_max_row_group_bytes(Some(512))
        .build();
    let file = File::create(path).unwrap();
    let mut writer = ArrowWriter::try_new(file, schema.clone(), Some(props)).unwrap();

    // Write sorted blocks of 100 rows each so every row group has tight min/max statistics.
    for offset in (0..count).step_by(100) {
        let end = (offset + 100).min(count);
        let len = (end - offset) as usize;
        let mut ids = Vec::with_capacity(len);
        let mut amounts = Vec::with_capacity(len);
        let mut scores = Vec::with_capacity(len);
        let mut cats = Vec::with_capacity(len);
        for k in offset..end {
            let val = k + 1; // 1..=count monotonic per file
            ids.push(start_id + k);
            amounts.push(val);
            scores.push((val as f64) * 1.5);
            cats.push(format!("{tag}_{}", k % 4));
        }
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(Int64Array::from(ids)),
                Arc::new(Int64Array::from(amounts)),
                Arc::new(Float64Array::from(scores)),
                Arc::new(StringArray::from(cats)),
            ],
        )
        .unwrap();
        writer.write(&batch).unwrap();
    }
    writer.close().unwrap();
}

fn build_hive_lake(dir: &Path) -> String {
    write_partition_parquet(
        &dir.join("year=2025/region=EMEA/part-0.parquet"),
        1,
        500,
        "emea",
    );
    write_partition_parquet(
        &dir.join("year=2026/region=APAC/part-0.parquet"),
        1001,
        500,
        "apac",
    );
    write_partition_parquet(
        &dir.join("year=2026/region=US/part-0.parquet"),
        2001,
        500,
        "us",
    );
    format!("{}/**/*.parquet", dir.to_string_lossy().replace('\\', "/"))
}

#[test]
fn test_bigdata_metadata_count_and_hive_zone_map_pruning() {
    let dir = tempfile::tempdir().unwrap();
    let glob = build_hive_lake(dir.path());
    let db_path = dir.path().join("olap.tapir");
    let conn = Connection::open(&db_path).unwrap();

    // 1. Metadata-only COUNT(*) across 3 files (1,500 total rows, 0 projected columns decoded).
    let (meta_rows, meta_stats) = conn
        .query_olap(&format!("SELECT COUNT(*) AS n FROM read_parquet('{glob}')"))
        .unwrap();
    assert_eq!(meta_rows.len(), 1);
    assert_eq!(meta_rows[0].get_value("n"), Some(&Value::Integer(1500)));
    assert_eq!(meta_stats.files_scanned, 3);
    assert_eq!(meta_stats.columns_projected, 0);

    // Also verify Connection::query matches identically.
    let direct_rows = conn
        .query(&format!("SELECT COUNT(*) AS n FROM read_parquet('{glob}')"))
        .unwrap();
    assert_eq!(direct_rows[0].get_value("n"), Some(&Value::Integer(1500)));

    // 2. Hive partition pruning + Zone-Map Row-Group min/max pruning + Column Projection Pushdown:
    //    year=2026 & region='APAC' prunes 2 files (2025/EMEA and 2026/US).
    //    amount > 420 prunes the first 4 row groups (1..=400) out of 5 in APAC!
    //    Only 1 physical column (`amount`) is projected.
    let sql = format!(
        "SELECT COUNT(*) AS cnt, MIN(amount) AS mn, MAX(amount) AS mx, SUM(amount) AS total \
         FROM read_parquet('{glob}') \
         WHERE year = 2026 AND region = 'APAC' AND amount > 420"
    );
    let (rows, stats) = conn.query_olap(&sql).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get_value("cnt"), Some(&Value::Integer(80)));
    assert_eq!(rows[0].get_value("mn"), Some(&Value::Integer(421)));
    assert_eq!(rows[0].get_value("mx"), Some(&Value::Integer(500)));
    // Sum of 421..=500 = 80 * (421 + 500) / 2 = 36840
    assert_eq!(rows[0].get_value("total"), Some(&Value::Integer(36840)));

    assert_eq!(stats.files_pruned_hive, 2, "expected 2 Hive files pruned");
    assert_eq!(stats.files_scanned, 1);
    assert_eq!(stats.row_groups_total, 5);
    assert!(
        stats.row_groups_pruned >= 4,
        "expected >=4 pruned row groups, got {}",
        stats.row_groups_pruned
    );
    assert_eq!(
        stats.columns_projected, 1,
        "only `amount` should be decoded from Parquet"
    );
    assert_eq!(
        conn.query(&sql).unwrap()[0].get_value("total"),
        Some(&Value::Integer(36840))
    );
}

#[test]
fn test_bigdata_parallel_group_by_analytics_and_projection_limit() {
    let dir = tempfile::tempdir().unwrap();
    let glob = build_hive_lake(dir.path());
    let db_path = dir.path().join("olap_gb.tapir");
    let conn = Connection::open(&db_path).unwrap();

    let sql = format!(
        "SELECT region, COUNT(*) AS cnt, SUM(amount) AS total, AVG(amount) AS avg_amt, \
         STDDEV(amount) AS sd, MEDIAN(amount) AS med, PERCENTILE_CONT(amount, 0.9) AS p90, \
         COUNT(DISTINCT cat) AS uniq_cat \
         FROM read_parquet('{glob}') \
         WHERE year = 2026 AND amount <= 100 \
         GROUP BY region \
         HAVING COUNT(*) = 100 \
         ORDER BY region ASC \
         LIMIT 2"
    );
    let (rows, stats) = conn.query_olap(&sql).unwrap();
    assert_eq!(
        stats.files_pruned_hive, 1,
        "year=2025 file should be pruned"
    );
    assert_eq!(stats.files_scanned, 2);
    assert!(
        stats.row_groups_pruned >= 8,
        "each 2026 file has 4 row groups (>100) pruned: got {}",
        stats.row_groups_pruned
    );
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows[0].get_value("region"),
        Some(&Value::Text("APAC".into()))
    );
    assert_eq!(rows[1].get_value("region"), Some(&Value::Text("US".into())));

    for row in &rows {
        assert_eq!(row.get_value("cnt"), Some(&Value::Integer(100)));
        assert_eq!(row.get_value("total"), Some(&Value::Integer(5050)));
        assert_eq!(row.get_value("avg_amt"), Some(&Value::Real(50.5)));
        assert_eq!(row.get_value("med"), Some(&Value::Real(50.5)));
        assert_eq!(row.get_value("uniq_cat"), Some(&Value::Integer(4)));
        if let Some(Value::Real(p90)) = row.get_value("p90") {
            assert!((*p90 - 90.1).abs() < 0.05, "p90={p90}");
        } else {
            panic!("missing p90");
        }
        if let Some(Value::Real(sd)) = row.get_value("sd") {
            assert!((*sd - 29.01149).abs() < 0.05, "sd={sd}");
        } else {
            panic!("missing sd");
        }
    }

    // Also test projected SELECT + ORDER BY + LIMIT + OFFSET
    let scan_sql = format!(
        "SELECT id, amount FROM read_parquet('{glob}') \
         WHERE year = 2026 AND region = 'US' AND amount >= 496 AND amount <= 500 \
         ORDER BY amount DESC LIMIT 2 OFFSET 1"
    );
    let (scan_rows, scan_stats) = conn.query_olap(&scan_sql).unwrap();
    assert_eq!(scan_rows.len(), 2);
    assert_eq!(scan_rows[0].columns(), &["id", "amount"]);
    assert_eq!(scan_rows[0].get_value("amount"), Some(&Value::Integer(499)));
    assert_eq!(scan_rows[1].get_value("amount"), Some(&Value::Integer(498)));
    assert_eq!(scan_stats.columns_projected, 2);
}
