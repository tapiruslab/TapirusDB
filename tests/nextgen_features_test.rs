use tapirus::{
    reciprocal_rank_fusion, ChangeOp, Connection, DeterministicHashEmbedder, EmbeddingEngine,
    ProductQuantizer,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

#[test]
fn test_product_quantizer_subspace_compression() {
    // Generate synthetic 64-dimensional vectors
    let vectors: Vec<Vec<f32>> = (0..20)
        .map(|i| (0..64).map(|j| ((i * 64 + j) as f32 * 0.1).cos()).collect())
        .collect();

    // 64-dim into 4 subspaces of 16-dim each, 8 centroids each
    let pq = ProductQuantizer::train(&vectors, 4, 8);
    let sample = &vectors[0];
    let encoded = pq.encode(sample);

    assert_eq!(encoded.dimensions(), 64);
    assert_eq!(encoded.codes.len(), 4);
    // 64 floats * 4 bytes = 256 bytes. 4 codes = 4 bytes -> 64x compression!
    assert_eq!(encoded.compression_ratio(), 64.0);

    let dist = pq.asymmetric_distance(&encoded, sample);
    assert!(dist < 2.0);
}

#[test]
fn test_deterministic_hash_embedder() {
    let embedder = DeterministicHashEmbedder::new(128);
    assert_eq!(embedder.dimensions(), 128);

    let v1 = embedder.embed_text("TapirusDB safe rust embedded database");
    let v2 = embedder.embed_text("TapirusDB safe rust embedded database");
    assert_eq!(v1, v2);

    // Verify unit length
    let norm: f32 = v1.iter().map(|x| x * x).sum::<f32>().sqrt();
    assert!((norm - 1.0).abs() < 1e-5);
}

#[test]
fn test_reciprocal_rank_fusion_logic() {
    let bm25_rank: Vec<u64> = vec![101, 102, 103];
    let vec_rank: Vec<u64> = vec![102, 101, 104];

    let fused = reciprocal_rank_fusion(&[(&bm25_rank, 1.0), (&vec_rank, 1.0)], 60.0, 5);
    assert_eq!(fused.len(), 4);

    // Both 101 and 102 appear in top 2 of both lists, so their scores are highest
    assert!(fused[0].0 == 101 || fused[0].0 == 102);
    assert!(fused[1].0 == 101 || fused[1].0 == 102);
}

#[test]
fn test_reactive_cdc_table_subscription() {
    let conn = Connection::open_in_memory().expect("Open in memory");

    let counter = Arc::new(AtomicUsize::new(0));
    let counter_clone = counter.clone();

    let sub_id = conn.subscribe("audit_log", move |ev| {
        assert_eq!(ev.table, "audit_log");
        assert_eq!(ev.op, ChangeOp::Insert);
        counter_clone.fetch_add(1, Ordering::SeqCst);
    });

    conn.execute("CREATE TABLE audit_log (id INT PRIMARY KEY, action TEXT);")
        .expect("Create table");

    conn.execute("INSERT INTO audit_log VALUES (1, 'User login');")
        .expect("Insert 1");
    conn.execute("INSERT INTO audit_log VALUES (2, 'Password change');")
        .expect("Insert 2");

    assert_eq!(counter.load(Ordering::SeqCst), 2);

    conn.unsubscribe(sub_id);

    conn.execute("INSERT INTO audit_log VALUES (3, 'Logout');")
        .expect("Insert 3");
    // Should remain 2 after unsubscribe
    assert_eq!(counter.load(Ordering::SeqCst), 2);
}

#[test]
fn test_hot_online_vacuum_into_backup() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let primary_path = temp_dir.path().join("primary.tapir");
    let backup_path = temp_dir.path().join("backup.tapir");

    let conn = Connection::open(&primary_path).expect("Open primary");
    conn.execute("CREATE TABLE inventory (id INT PRIMARY KEY, sku TEXT, qty INT);")
        .expect("create");
    conn.execute("INSERT INTO inventory VALUES (1, 'TAPIR-001', 500);")
        .expect("insert");

    // Execute Hot Online Backup
    let backup_str = backup_path.to_str().unwrap().replace('\\', "/");
    let vac_sql = format!("VACUUM INTO '{backup_str}';");
    conn.execute(&vac_sql).expect("vacuum into");

    // Open backup file and verify data integrity
    let backup_conn = Connection::open(&backup_path).expect("Open backup");
    let rows = backup_conn
        .query("SELECT * FROM inventory;")
        .expect("query backup");

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get::<i64>("id").unwrap(), 1);
    assert_eq!(rows[0].get::<String>("sku").unwrap(), "TAPIR-001");
}

#[test]
fn test_time_travel_as_of_timestamp_syntax() {
    let sql = "SELECT id, name FROM users AS OF TIMESTAMP 1700000000 WHERE active = true;";
    let stmt = tapirus::parse_sql(sql).expect("Parse AS OF TIMESTAMP");

    match stmt {
        tapirus::Statement::Select {
            table,
            as_of_timestamp,
            ..
        } => {
            assert_eq!(table, "users");
            assert_eq!(as_of_timestamp, Some(1700000000));
        }
        _ => panic!("Expected Select statement with as_of_timestamp"),
    }
}
