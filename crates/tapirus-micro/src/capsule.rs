//! Versioned, allocation-free transport frames for Micro-to-Quad exchange.
//!
//! CRC32 detects accidental corruption. A capsule has no signature, encryption,
//! clock or authorization claim; authenticate the transport separately.

/// Fixed capsule header length in bytes.
pub const HEADER_LEN: usize = 48;
/// Upper bound for one capsule payload.
pub const MAX_PAYLOAD_LEN: usize = 64 * 1024;
const MAGIC: &[u8; 8] = b"TPIOCAP1";
const VERSION: u16 = 1;
const DELETED: u8 = 1;

/// Invalid or unsupported capsule data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapsuleError {
    /// Output buffer is too small for the complete frame.
    Capacity,
    /// Header, length, flag, device identity or sequence is invalid.
    Invalid,
    /// Frame version is not supported by this decoder.
    Version,
    /// Payload exceeds the bounded transport size.
    PayloadTooLarge,
    /// CRC32 did not match the received frame.
    Checksum,
}

/// One committed Micro slot value or tombstone, borrowed from a caller buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capsule<'a> {
    /// Stable device identity assigned by the application; not authentication.
    pub device_id: [u8; 16],
    /// Numbered Micro slot.
    pub slot: u32,
    /// Per-slot committed sequence from the Micro store.
    pub sequence: u64,
    /// Whether this frame represents a deleted slot.
    pub deleted: bool,
    /// Raw application bytes, empty for a deletion.
    pub payload: &'a [u8],
}

impl Capsule<'_> {
    /// Encode this capsule into caller-owned memory and return its exact length.
    pub fn encode_into(&self, output: &mut [u8]) -> Result<usize, CapsuleError> {
        if self.sequence == 0
            || self.device_id == [0; 16]
            || (self.deleted && !self.payload.is_empty())
        {
            return Err(CapsuleError::Invalid);
        }
        if self.payload.len() > MAX_PAYLOAD_LEN {
            return Err(CapsuleError::PayloadTooLarge);
        }
        let length = HEADER_LEN + self.payload.len();
        if output.len() < length {
            return Err(CapsuleError::Capacity);
        }
        output[..8].copy_from_slice(MAGIC);
        output[8..10].copy_from_slice(&VERSION.to_le_bytes());
        output[10] = if self.deleted { DELETED } else { 0 };
        output[11] = 0;
        output[12..28].copy_from_slice(&self.device_id);
        output[28..32].copy_from_slice(&self.slot.to_le_bytes());
        output[32..40].copy_from_slice(&self.sequence.to_le_bytes());
        output[40..44].copy_from_slice(&(self.payload.len() as u32).to_le_bytes());
        output[48..length].copy_from_slice(self.payload);
        let crc = super::checksum(&output[..44], &output[48..length]);
        output[44..48].copy_from_slice(&crc.to_le_bytes());
        Ok(length)
    }
}

/// Decode one exact frame without allocation; trailing bytes are rejected.
pub fn decode_capsule(bytes: &[u8]) -> Result<Capsule<'_>, CapsuleError> {
    if bytes.len() < HEADER_LEN || &bytes[..8] != MAGIC {
        return Err(CapsuleError::Invalid);
    }
    let version = u16::from_le_bytes(bytes[8..10].try_into().map_err(|_| CapsuleError::Invalid)?);
    if version != VERSION {
        return Err(CapsuleError::Version);
    }
    if bytes[10] & !DELETED != 0 || bytes[11] != 0 {
        return Err(CapsuleError::Invalid);
    }
    let device_id: [u8; 16] = bytes[12..28]
        .try_into()
        .map_err(|_| CapsuleError::Invalid)?;
    let slot = u32::from_le_bytes(
        bytes[28..32]
            .try_into()
            .map_err(|_| CapsuleError::Invalid)?,
    );
    let sequence = u64::from_le_bytes(
        bytes[32..40]
            .try_into()
            .map_err(|_| CapsuleError::Invalid)?,
    );
    let payload_len = u32::from_le_bytes(
        bytes[40..44]
            .try_into()
            .map_err(|_| CapsuleError::Invalid)?,
    ) as usize;
    if payload_len > MAX_PAYLOAD_LEN {
        return Err(CapsuleError::PayloadTooLarge);
    }
    if HEADER_LEN.checked_add(payload_len) != Some(bytes.len())
        || sequence == 0
        || device_id == [0; 16]
        || (bytes[10] == DELETED && payload_len != 0)
    {
        return Err(CapsuleError::Invalid);
    }
    let stored = u32::from_le_bytes(
        bytes[44..48]
            .try_into()
            .map_err(|_| CapsuleError::Invalid)?,
    );
    if stored != super::checksum(&bytes[..44], &bytes[48..]) {
        return Err(CapsuleError::Checksum);
    }
    Ok(Capsule {
        device_id,
        slot,
        sequence,
        deleted: bytes[10] == DELETED,
        payload: &bytes[48..],
    })
}
