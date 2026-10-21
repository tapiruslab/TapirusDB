#![cfg(feature = "quad")]

use tapirus::iot::{import_micro_capsule, import_micro_capsule_with_provider};
use tapirus::memory::EmbeddingProvider;
use tapirus::{DeterministicHashEmbedder, MemorySearch, SessionDatabase, Value};
use tapirus_micro::capsule::Capsule;
use tapirus_micro::{Flash, Store};

struct RamFlash([u8; 512]);

impl Flash for RamFlash {
    type Error = &'static str;

    fn capacity(&self) -> usize {
        self.0.len()
    }

    fn erase_size(&self) -> usize {
        256
    }

    fn write_size(&self) -> usize {
        8
    }

    fn read(&mut self, offset: usize, output: &mut [u8]) -> Result<(), Self::Error> {
        output.copy_from_slice(&self.0[offset..offset + output.len()]);
        Ok(())
    }

    fn erase(&mut self, offset: usize, length: usize) -> Result<(), Self::Error> {
        self.0[offset..offset + length].fill(0xff);
        Ok(())
    }

    fn program(&mut self, offset: usize, bytes: &[u8]) -> Result<(), Self::Error> {
        assert_eq!(offset % self.write_size(), 0);
        assert_eq!(bytes.len() % self.write_size(), 0);
        for (saved, byte) in self.0[offset..offset + bytes.len()].iter_mut().zip(bytes) {
            if *saved & byte != *byte {
                return Err("program attempted zero-to-one bit transition");
            }
            *saved &= byte;
        }
        Ok(())
    }

    fn sync(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

fn frame(device_id: [u8; 16], slot: u32, sequence: u64, text: &[u8], deleted: bool) -> Vec<u8> {
    let mut bytes = vec![0u8; 48 + text.len()];
    let length = Capsule {
        device_id,
        slot,
        sequence,
        deleted,
        payload: text,
    }
    .encode_into(&mut bytes)
    .unwrap();
    bytes.truncate(length);
    bytes
}

#[test]
fn real_micro_store_exports_into_quad_memory_and_tombstones() -> tapirus::Result<()> {
    let mut micro = Store::<_, 256>::new(RamFlash([0xff; 512]), 1).unwrap();
    let mut frame = [0u8; 256];
    let db = SessionDatabase::open_in_memory()?;
    let session = db.session()?;
    let device_id = [3; 16];

    micro.put(0, b"pump bearing temperature 68 C").unwrap();
    let length = micro
        .export_capsule(0, device_id, &mut frame)
        .unwrap()
        .unwrap();
    let receipt = import_micro_capsule(&session, &frame[..length])?;
    assert_eq!(receipt.sequence, 1);
    assert!(receipt.applied);
    let search = || {
        session.search_memories(&MemorySearch {
            query: Some("bearing".into()),
            namespace: Some(format!("iot/{}", receipt.device_id)),
            ..Default::default()
        })
    };
    assert_eq!(search()?.results.len(), 1);

    micro.delete(0).unwrap();
    let length = micro
        .export_capsule(0, device_id, &mut frame)
        .unwrap()
        .unwrap();
    let deletion = import_micro_capsule(&session, &frame[..length])?;
    assert_eq!(deletion.sequence, 2);
    assert!(deletion.deleted);
    assert!(search()?.results.is_empty());
    db.close()?;
    Ok(())
}

#[test]
fn imported_iot_state_is_idempotent_searchable_and_persistent() -> tapirus::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("iot.tapir");
    let db = SessionDatabase::open(&path)?;
    let session = db.session()?;
    let device = [7; 16];
    let first = frame(device, 0, 1, b"pump calibration complete", false);
    let receipt = import_micro_capsule(&session, &first)?;
    assert!(receipt.applied);
    assert!(!import_micro_capsule(&session, &first)?.applied);
    let namespace = format!("iot/{}", receipt.device_id);
    let find = |query: &str| {
        session.search_memories(&MemorySearch {
            query: Some(query.into()),
            namespace: Some(namespace.clone()),
            ..Default::default()
        })
    };
    assert_eq!(find("calibration")?.results.len(), 1);

    let second = frame(device, 0, 2, b"pump filter replacement due", false);
    assert!(import_micro_capsule(&session, &second)?.applied);
    assert!(find("calibration")?.results.is_empty());
    assert_eq!(find("replacement")?.results.len(), 1);
    assert!(import_micro_capsule(&session, &first).is_err());

    let conflicting = frame(device, 0, 2, b"different state", false);
    assert!(import_micro_capsule(&session, &conflicting).is_err());
    assert_eq!(find("replacement")?.results.len(), 1);

    let other = frame([8; 16], 0, 1, b"other device temperature normal", false);
    assert!(import_micro_capsule(&session, &other)?.applied);
    assert!(find("temperature")?.results.is_empty());

    let deleted = frame(device, 0, 3, b"", true);
    assert!(import_micro_capsule(&session, &deleted)?.applied);
    assert!(find("replacement")?.results.is_empty());
    assert!(!import_micro_capsule(&session, &deleted)?.applied);
    assert!(import_micro_capsule(&session, &second).is_err());
    session.close()?;
    db.close()?;
    drop(session);
    drop(db);

    let reopened = SessionDatabase::open(&path)?;
    let session = reopened.session()?;
    assert!(!import_micro_capsule(&session, &deleted)?.applied);
    assert!(
        session
            .search_memories(&MemorySearch {
                query: Some("replacement".into()),
                namespace: Some(namespace),
                ..Default::default()
            })?
            .results
            .is_empty()
    );
    let rows = session.query_with_params(
        "SELECT sequence FROM __iot_heads WHERE source_key = ?",
        &[Value::Text(format!("{}:0", receipt.device_id))],
    )?;
    assert_eq!(rows[0].get::<String>("sequence")?, "3");
    session.close()?;
    reopened.close()?;
    Ok(())
}

#[test]
fn invalid_payload_and_crc_leave_no_imported_memory() -> tapirus::Result<()> {
    let db = SessionDatabase::open_in_memory()?;
    let session = db.session()?;
    let invalid_utf8 = frame([2; 16], 1, 1, &[0xff], false);
    assert!(import_micro_capsule(&session, &invalid_utf8).is_err());
    let mut corrupted = frame([2; 16], 1, 1, b"valid text", false);
    corrupted[48] ^= 1;
    assert!(import_micro_capsule(&session, &corrupted).is_err());
    assert!(session.query("SELECT * FROM __iot_heads").is_err());
    db.close()?;
    Ok(())
}

#[test]
fn gateway_can_attach_a_pinned_local_embedding_model() -> tapirus::Result<()> {
    let db = SessionDatabase::open_in_memory()?;
    let session = db.session()?;
    let provider = DeterministicHashEmbedder::new(64);
    let frame = frame([9; 16], 0, 1, b"motor bearing overheating", false);
    let receipt = import_micro_capsule_with_provider(&session, &frame, &provider)?;
    assert!(receipt.applied);
    let query = provider.embed_documents(&["motor bearing"])?;
    let result = session.search_memories(&MemorySearch {
        query: Some("bearing".into()),
        embedding: Some(query[0].clone()),
        embedding_model: Some(provider.model_id()),
        namespace: Some(format!("iot/{}", receipt.device_id)),
        ..Default::default()
    })?;
    assert_eq!(result.results.len(), 1);
    assert!(result.results[0].semantic_score > 0.0);
    assert!(!import_micro_capsule_with_provider(&session, &frame, &provider)?.applied);
    db.close()?;
    Ok(())
}

#[test]
fn cli_imports_capsule_from_file_once() {
    let dir = tempfile::tempdir().unwrap();
    let frame_path = dir.path().join("reading.cap");
    let db_path = dir.path().join("reading.tapir");
    std::fs::write(
        &frame_path,
        frame([4; 16], 0, 1, b"bearing temperature rising", false),
    )
    .unwrap();
    let bin = env!("CARGO_BIN_EXE_tapirus-iot");
    let inspect = std::process::Command::new(bin)
        .args(["inspect", frame_path.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(inspect.status.success());
    assert!(
        String::from_utf8(inspect.stdout)
            .unwrap()
            .contains("device_id=04040404040404040404040404040404")
    );
    let args = [
        "import",
        db_path.to_str().unwrap(),
        frame_path.to_str().unwrap(),
    ];
    let first = std::process::Command::new(bin).args(args).output().unwrap();
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(
        String::from_utf8(first.stdout)
            .unwrap()
            .contains("\"applied\":true")
    );
    let second = std::process::Command::new(bin).args(args).output().unwrap();
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    assert!(
        String::from_utf8(second.stdout)
            .unwrap()
            .contains("\"applied\":false")
    );
    let search = std::process::Command::new(bin)
        .args([
            "search",
            db_path.to_str().unwrap(),
            "04040404040404040404040404040404",
            "bearing",
        ])
        .output()
        .unwrap();
    assert!(
        search.status.success(),
        "{}",
        String::from_utf8_lossy(&search.stderr)
    );
    assert!(
        String::from_utf8(search.stdout)
            .unwrap()
            .contains("bearing temperature rising")
    );
}
