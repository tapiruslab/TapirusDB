//! Import offline Tapirus Micro capsules into the full Quad AI memory engine.

use std::io::Read;
use tapirus::iot::import_micro_capsule;
use tapirus::{MemorySearch, SessionDatabase};
use tapirus_micro::capsule::{HEADER_LEN, MAX_PAYLOAD_LEN, decode_capsule};

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn read_frame(path: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let file = std::fs::File::open(path)?;
    let mut bytes = Vec::new();
    file.take((HEADER_LEN + MAX_PAYLOAD_LEN + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > HEADER_LEN + MAX_PAYLOAD_LEN {
        return Err("IoT capsule exceeds the bounded transport size".into());
    }
    Ok(bytes)
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() == 2 && args[1] == "--version" {
        println!("tapirus-iot {}", tapirus::VERSION);
        return Ok(());
    }
    match args.as_slice() {
        [_, command, capsule_path] if command == "inspect" => {
            let bytes = read_frame(capsule_path)?;
            let capsule = decode_capsule(&bytes).map_err(|error| format!("{error:?}"))?;
            let device_id = capsule
                .device_id
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            println!(
                "device_id={} slot={} sequence={} deleted={} payload_bytes={}",
                device_id,
                capsule.slot,
                capsule.sequence,
                capsule.deleted,
                capsule.payload.len()
            );
        }
        [_, command, database_path, capsule_path] if command == "import" => {
            let bytes = read_frame(capsule_path)?;
            let db = SessionDatabase::open(database_path)?;
            let session = db.session()?;
            let receipt = import_micro_capsule(&session, &bytes)?;
            println!("{}", serde_json::to_string(&receipt)?);
            session.close()?;
            db.close()?;
        }
        [_, command, database_path, device_id, query] if command == "search" => {
            if device_id.len() != 32 || !device_id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err("Device ID must be exactly 32 hexadecimal characters".into());
            }
            let db = SessionDatabase::open(database_path)?;
            let session = db.session()?;
            let result = session.search_memories(&MemorySearch {
                query: Some(query.clone()),
                namespace: Some(format!("iot/{}", device_id.to_ascii_lowercase())),
                ..MemorySearch::default()
            })?;
            println!("{}", serde_json::to_string(&result)?);
            session.close()?;
            db.close()?;
        }
        _ => {
            return Err(
                "Usage: tapirus-iot inspect <capsule.bin> | import <database.tapir> <capsule.bin> | search <database.tapir> <device-id-hex> <query>"
                    .into(),
            );
        }
    }
    Ok(())
}
