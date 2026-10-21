//! Offline Micro-to-Quad ingestion for bounded, versioned IoT slot values.
//!
//! The capsule CRC detects accidental corruption, not forgery. Authenticate
//! device identity and protect transport confidentiality outside this API.

use crate::memory::EmbeddingProvider;
use crate::{Error, MemoryDeleteScope, MemoryDocument, Result, Session, Value};
use sha2::{Digest, Sha256};
use tapirus_micro::capsule::{Capsule, decode_capsule};

/// Result of importing one Micro capsule into a Quad session.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ImportReceipt {
    /// Lowercase hexadecimal application-assigned device ID.
    pub device_id: String,
    /// Source Micro slot.
    pub slot: u32,
    /// Per-slot committed sequence.
    pub sequence: u64,
    /// True if the capsule changed durable state; false for exact replay.
    pub applied: bool,
    /// True if the source slot was deleted on the device.
    pub deleted: bool,
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut result, "{byte:02x}").expect("hex formatting into String");
    }
    result
}

fn head_id(capsule: &Capsule<'_>) -> i64 {
    let mut hash = Sha256::new();
    hash.update(b"tapirus-iot-head-v1");
    hash.update(capsule.device_id);
    hash.update(capsule.slot.to_le_bytes());
    let digest = hash.finalize();
    (u64::from_le_bytes(digest[..8].try_into().expect("SHA-256 digest length")) & i64::MAX as u64)
        as i64
}

/// Import a complete capsule once, preserving monotonically newer slot state.
///
/// A committed SQL head and AI memory source are changed in one owned session
/// transaction. A byte-for-byte replay is a no-op; an older sequence or a
/// different frame at the same sequence is rejected. Nondeleted payloads must
/// be nonempty UTF-8 text. No embedding or LLM inference is performed here;
/// retrieval is lexical unless an application later supplies a model vector.
pub fn import_micro_capsule(session: &Session, frame: &[u8]) -> Result<ImportReceipt> {
    import_capsule(session, frame, None)
}

/// Generate a pinned-model embedding on the gateway before atomically importing.
///
/// The provider is called before a database transaction. It may be a local
/// on-device model; a lexical feature-hash provider is not a neural model.
/// Exact replays still leave the existing source unchanged.
pub fn import_micro_capsule_with_provider(
    session: &Session,
    frame: &[u8],
    provider: &impl EmbeddingProvider,
) -> Result<ImportReceipt> {
    let capsule = decode_capsule(frame)
        .map_err(|error| Error::Corrupted(format!("Invalid IoT capsule: {error:?}")))?;
    if capsule.deleted {
        return import_capsule(session, frame, None);
    }
    let text = std::str::from_utf8(capsule.payload)
        .map_err(|_| Error::ConstraintViolation("IoT AI payload must be UTF-8".into()))?;
    if text.trim().is_empty() {
        return Err(Error::ConstraintViolation(
            "IoT AI payload must contain text".into(),
        ));
    }
    let model_id = provider.model_id();
    if model_id.trim().is_empty() {
        return Err(Error::ConstraintViolation(
            "IoT embedding model identity must be pinned".into(),
        ));
    }
    let mut vectors = provider.embed_documents(&[text])?;
    crate::memory::embedder::validate_embeddings(&vectors, 1, provider.vector_dimensions())?;
    import_capsule(session, frame, Some((vectors.remove(0), model_id)))
}

fn import_capsule(
    session: &Session,
    frame: &[u8],
    embedding: Option<(Vec<f32>, String)>,
) -> Result<ImportReceipt> {
    let capsule = decode_capsule(frame)
        .map_err(|error| Error::Corrupted(format!("Invalid IoT capsule: {error:?}")))?;
    let content = if capsule.deleted {
        None
    } else {
        let text = std::str::from_utf8(capsule.payload)
            .map_err(|_| Error::ConstraintViolation("IoT AI payload must be UTF-8".into()))?;
        if text.trim().is_empty() {
            return Err(Error::ConstraintViolation(
                "IoT AI payload must contain text".into(),
            ));
        }
        Some(text)
    };
    let device_id = hex(&capsule.device_id);
    let source_key = format!("{device_id}:{}", capsule.slot);
    let namespace = format!("iot/{device_id}");
    let external_id = format!("slot/{}", capsule.slot);
    let fingerprint = hex(&Sha256::digest(frame));
    let id = head_id(&capsule);
    let mut receipt = ImportReceipt {
        device_id,
        slot: capsule.slot,
        sequence: capsule.sequence,
        applied: false,
        deleted: capsule.deleted,
    };

    session.transaction(|tx| {
        tx.execute(
            "CREATE TABLE IF NOT EXISTS __iot_heads (id INTEGER PRIMARY KEY, source_key TEXT NOT NULL, sequence TEXT NOT NULL, fingerprint TEXT NOT NULL)",
        )?;
        let rows = tx.query_with_params(
            "SELECT source_key, sequence, fingerprint FROM __iot_heads WHERE id = ?",
            &[Value::Integer(id)],
        )?;
        let existing = if let Some(row) = rows.first() {
            let stored_key = row.get::<String>("source_key")?;
            if stored_key != source_key {
                return Err(Error::ConstraintViolation(
                    "IoT source-key hash collision; no state changed".into(),
                ));
            }
            let stored_sequence = row
                .get::<String>("sequence")?
                .parse::<u64>()
                .map_err(|_| Error::Corrupted("Invalid IoT head sequence".into()))?;
            let stored_fingerprint = row.get::<String>("fingerprint")?;
            if capsule.sequence < stored_sequence {
                return Err(Error::ConstraintViolation(
                    "Stale IoT capsule sequence".into(),
                ));
            }
            if capsule.sequence == stored_sequence {
                if fingerprint == stored_fingerprint {
                    return Ok(receipt);
                }
                return Err(Error::ConstraintViolation(
                    "Conflicting IoT capsule at the same sequence".into(),
                ));
            }
            true
        } else {
            false
        };

        if let Some(content) = content {
            let (vector, model) = embedding.map_or((None, None), |(vector, model)| {
                (Some(vector), Some(model))
            });
            tx.upsert_memory(&MemoryDocument {
                content: content.to_owned(),
                embedding: vector,
                embedding_model: model,
                namespace: Some(namespace.clone()),
                external_id: Some(external_id.clone()),
                source_version: Some(capsule.sequence.to_string()),
                tags: vec!["iot".into(), format!("slot:{}", capsule.slot)],
                ..MemoryDocument::default()
            })?;
        } else {
            tx.delete_memories(&MemoryDeleteScope {
                namespace: Some(namespace),
                external_id: Some(external_id),
                ..MemoryDeleteScope::default()
            })?;
        }

        if existing {
            if tx.execute_with_params(
                "UPDATE __iot_heads SET sequence = ?, fingerprint = ? WHERE id = ?",
                &[
                    Value::Text(capsule.sequence.to_string()),
                    Value::Text(fingerprint),
                    Value::Integer(id),
                ],
            )? != 1
            {
                return Err(Error::Corrupted("IoT head update missed its row".into()));
            }
        } else {
            tx.execute_with_params(
                "INSERT INTO __iot_heads (id, source_key, sequence, fingerprint) VALUES (?, ?, ?, ?)",
                &[
                    Value::Integer(id),
                    Value::Text(source_key),
                    Value::Text(capsule.sequence.to_string()),
                    Value::Text(fingerprint),
                ],
            )?;
        }
        receipt.applied = true;
        Ok(receipt)
    })
}
