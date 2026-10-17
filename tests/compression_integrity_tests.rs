use tapirus::pager::compression::{compress_page_frame, decompress_page_frame};
use tapirus::{Config, Connection, Value};

#[test]
fn ordinary_overflow_page_ids_cannot_be_mistaken_for_compression_headers() {
    for next in [84u32, 340, 596, 852] {
        let mut raw = vec![0u8; 4096];
        raw[..4].copy_from_slice(&next.to_le_bytes());
        raw[4..6].copy_from_slice(&4050u16.to_le_bytes());
        raw[6..4056].fill(0xab);
        assert_eq!(decompress_page_frame(&raw, 4096).unwrap(), raw);
    }
}

#[test]
fn incompressible_frames_remain_fixed_size_without_losing_payload() {
    let mut state = 7u64;
    let mut raw = vec![0u8; 4096];
    for byte in &mut raw[..4056] {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        *byte = (state >> 32) as u8;
    }
    let encoded = compress_page_frame(&raw);
    assert_eq!(encoded.len(), 4096);
    assert_eq!(encoded, raw);
    assert_eq!(decompress_page_frame(&encoded, 4096).unwrap(), raw);
}

#[test]
fn compressed_and_plain_large_blobs_survive_eviction_checkpoint_and_wal_recovery() {
    for encrypted in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("blobs.tapir");
        let config = Config {
            page_cache_capacity: 4,
            encryption_key: encrypted.then_some([17; 32]),
            ..Default::default()
        };
        let db = Connection::open_with_config(&path, config.clone()).unwrap();
        db.enable_compression();
        db.execute("CREATE TABLE payloads (id INTEGER PRIMARY KEY, body BLOB)")
            .unwrap();
        let mut state = 13u64;
        let mut random = vec![0u8; 12000];
        for byte in &mut random {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            *byte = (state >> 32) as u8;
        }
        let repetitive = vec![b'x'; 12000];
        db.execute("BEGIN").unwrap();
        for id in 0..32 {
            db.execute_with_params(
                "INSERT INTO payloads VALUES (?, ?)",
                &[
                    Value::Integer(id),
                    Value::Blob(if id % 2 == 0 {
                        random.clone()
                    } else {
                        repetitive.clone()
                    }),
                ],
            )
            .unwrap();
        }
        db.commit().unwrap();
        for id in 0..32 {
            assert_eq!(
                db.query_with_params(
                    "SELECT body FROM payloads WHERE id = ?",
                    &[Value::Integer(id)]
                )
                .unwrap()[0]
                    .get::<Vec<u8>>("body")
                    .unwrap(),
                if id % 2 == 0 {
                    random.clone()
                } else {
                    repetitive.clone()
                }
            );
        }
        // Drop without close/checkpoint; open must replay encoded committed WAL.
        drop(db);
        let db = Connection::open_with_config(&path, config).unwrap();
        assert_eq!(db.query("SELECT * FROM payloads").unwrap().len(), 32);
        db.checkpoint().unwrap();
        for id in 0..32 {
            assert_eq!(
                db.query_with_params(
                    "SELECT body FROM payloads WHERE id = ?",
                    &[Value::Integer(id)]
                )
                .unwrap()[0]
                    .get::<Vec<u8>>("body")
                    .unwrap(),
                if id % 2 == 0 {
                    random.clone()
                } else {
                    repetitive.clone()
                }
            );
        }
        assert!(db.check_integrity().unwrap().is_ok());
    }
}
