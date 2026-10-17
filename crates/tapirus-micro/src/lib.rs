//! Fixed-slot flash records for microcontrollers, without std, allocation or unsafe.
//!
//! Each numbered slot owns two erase blocks. An update erases the inactive block,
//! writes and syncs a checksummed record, then programs and syncs a commit unit.
//! The prior copy remains intact until a later update. Format is independent of
//! native `.tapir`: no SQL, graph, HNSW, neural inference or cross-slot transaction.
#![no_std]
#![forbid(unsafe_code)]
#![warn(missing_docs)]

const HEADER: usize = 32;
const MAGIC: &[u8; 8] = b"TPMIC001";

/// Board adapter. Errors must be propagated; no mutation may be silently lost.
pub trait Flash {
    /// Board/driver error type.
    type Error;
    /// Total addressable bytes.
    fn capacity(&self) -> usize;
    /// Erase block bytes. Erase restores every bit in the block to one.
    fn erase_size(&self) -> usize;
    /// Minimum aligned program unit; programming only changes one bits to zero.
    fn write_size(&self) -> usize;
    /// Read bytes, including writes acknowledged by the adapter.
    fn read(&mut self, offset: usize, output: &mut [u8]) -> Result<(), Self::Error>;
    /// Erase a whole aligned block.
    fn erase(&mut self, offset: usize, length: usize) -> Result<(), Self::Error>;
    /// Program aligned units. A failed program may have written any prefix.
    fn program(&mut self, offset: usize, bytes: &[u8]) -> Result<(), Self::Error>;
    /// Complete all prior erase/program operations before returning success.
    fn sync(&mut self) -> Result<(), Self::Error>;
}

/// Storage failure. A failed write has an uncertain commit outcome; reread it.
#[derive(Debug, PartialEq, Eq)]
pub enum Error<E> {
    /// Board IO failure.
    Io(E),
    /// Unsupported geometry or insufficient capacity.
    Geometry,
    /// Slot outside the configured range.
    Slot,
    /// Payload exceeds one record or the output buffer is too small.
    Capacity,
    /// A committed record failed validation. Do not silently serve an older value.
    Corrupted,
    /// Sequence counter exhausted; application migration is required.
    SequenceExhausted,
}

#[derive(Clone, Copy)]
struct Record {
    sequence: u64,
    length: usize,
    deleted: bool,
    bank: usize,
}

/// Caller-owned device and one block of stack scratch per read/update operation.
/// Slot count and block geometry are fixed at construction; no heap is required.
pub struct Store<F, const BLOCK: usize> {
    flash: F,
    slots: usize,
    write_size: usize,
}

impl<F: Flash, const BLOCK: usize> Store<F, BLOCK> {
    /// Validate geometry without formatting or erasing existing records.
    pub fn new(flash: F, slots: usize) -> Result<Self, Error<F::Error>> {
        let write_size = flash.write_size();
        if slots == 0
            || BLOCK <= HEADER
            || BLOCK != flash.erase_size()
            || write_size == 0
            || write_size > BLOCK - HEADER
            || BLOCK % write_size != 0
            || slots
                .checked_mul(2)
                .and_then(|n| n.checked_mul(BLOCK))
                .is_none_or(|n| n > flash.capacity())
        {
            return Err(Error::Geometry);
        }
        Ok(Self {
            flash,
            slots,
            write_size,
        })
    }
    /// Maximum value bytes in one numbered slot.
    pub fn max_value_bytes(&self) -> usize {
        BLOCK - HEADER - self.write_size
    }
    /// Return ownership to the board/application, including after an IO failure.
    pub fn into_inner(self) -> F {
        self.flash
    }

    fn read_record(
        &mut self,
        slot: usize,
        bank: usize,
        scratch: &mut [u8; BLOCK],
    ) -> Result<Option<Record>, Error<F::Error>> {
        let offset = (slot * 2 + bank) * BLOCK;
        self.flash.read(offset, scratch).map_err(Error::Io)?;
        // Uncommitted or interrupted writes never supersede the previous copy.
        if scratch[BLOCK - self.write_size..]
            .iter()
            .any(|byte| *byte != 0)
        {
            return Ok(None);
        }
        if &scratch[..8] != MAGIC
            || u32::from_le_bytes(scratch[8..12].try_into().map_err(|_| Error::Corrupted)?) as usize
                != slot
        {
            return Err(Error::Corrupted);
        }
        let sequence =
            u64::from_le_bytes(scratch[12..20].try_into().map_err(|_| Error::Corrupted)?);
        let raw_length =
            u32::from_le_bytes(scratch[20..24].try_into().map_err(|_| Error::Corrupted)?);
        let deleted = raw_length == u32::MAX;
        let length = if deleted { 0 } else { raw_length as usize };
        if sequence == 0 || length > self.max_value_bytes() || scratch[28..32] != [0; 4] {
            return Err(Error::Corrupted);
        }
        let expected =
            u32::from_le_bytes(scratch[24..28].try_into().map_err(|_| Error::Corrupted)?);
        if checksum(&scratch[..24], &scratch[HEADER..HEADER + length]) != expected {
            return Err(Error::Corrupted);
        }
        Ok(Some(Record {
            sequence,
            length,
            deleted,
            bank,
        }))
    }
    fn latest(
        &mut self,
        slot: usize,
        scratch: &mut [u8; BLOCK],
    ) -> Result<Option<Record>, Error<F::Error>> {
        if slot >= self.slots || slot > u32::MAX as usize {
            return Err(Error::Slot);
        }
        let a = self.read_record(slot, 0, scratch)?;
        let b = self.read_record(slot, 1, scratch)?;
        match (a, b) {
            (Some(a), Some(b)) if a.sequence == b.sequence => Err(Error::Corrupted),
            (Some(a), Some(b)) => Ok(Some(if a.sequence > b.sequence { a } else { b })),
            (a, b) => Ok(a.or(b)),
        }
    }
    /// Read the newest committed value, or None for empty/deleted slots.
    pub fn get(
        &mut self,
        slot: usize,
        output: &mut [u8],
    ) -> Result<Option<usize>, Error<F::Error>> {
        let mut scratch = [0u8; BLOCK];
        let Some(record) = self.latest(slot, &mut scratch)? else {
            return Ok(None);
        };
        if record.deleted {
            return Ok(None);
        }
        if output.len() < record.length {
            return Err(Error::Capacity);
        }
        self.read_record(slot, record.bank, &mut scratch)?;
        output[..record.length].copy_from_slice(&scratch[HEADER..HEADER + record.length]);
        Ok(Some(record.length))
    }
    /// Replace one slot. Zero-length values are supported and differ from deletion.
    pub fn put(&mut self, slot: usize, value: &[u8]) -> Result<(), Error<F::Error>> {
        self.write(slot, Some(value))
    }
    /// Persist a tombstone for one slot.
    pub fn delete(&mut self, slot: usize) -> Result<(), Error<F::Error>> {
        self.write(slot, None)
    }
    fn write(&mut self, slot: usize, value: Option<&[u8]>) -> Result<(), Error<F::Error>> {
        if value.is_some_and(|v| v.len() > self.max_value_bytes() || v.len() >= u32::MAX as usize) {
            return Err(Error::Capacity);
        }
        let mut scratch = [0xff; BLOCK];
        let previous = self.latest(slot, &mut scratch)?;
        let bank = previous.map_or(0, |r| 1 - r.bank);
        let sequence = previous
            .map_or(Some(1), |r| r.sequence.checked_add(1))
            .ok_or(Error::SequenceExhausted)?;
        scratch.fill(0xff);
        scratch[..8].copy_from_slice(MAGIC);
        scratch[8..12].copy_from_slice(&(slot as u32).to_le_bytes());
        scratch[12..20].copy_from_slice(&sequence.to_le_bytes());
        scratch[20..24].copy_from_slice(&value.map_or(u32::MAX, |v| v.len() as u32).to_le_bytes());
        let value = value.unwrap_or(&[]);
        scratch[HEADER..HEADER + value.len()].copy_from_slice(value);
        let crc = checksum(&scratch[..24], value);
        scratch[24..28].copy_from_slice(&crc.to_le_bytes());
        scratch[28..32].fill(0);
        let offset = (slot * 2 + bank) * BLOCK;
        self.flash.erase(offset, BLOCK).map_err(Error::Io)?;
        self.flash
            .program(offset, &scratch[..BLOCK - self.write_size])
            .map_err(Error::Io)?;
        self.flash.sync().map_err(Error::Io)?;
        scratch[BLOCK - self.write_size..].fill(0);
        self.flash
            .program(
                offset + BLOCK - self.write_size,
                &scratch[BLOCK - self.write_size..],
            )
            .map_err(Error::Io)?;
        self.flash.sync().map_err(Error::Io)
    }
}

fn checksum(header: &[u8], payload: &[u8]) -> u32 {
    let mut crc = !0u32;
    for byte in header.iter().chain(payload) {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}

/// Allocation-free dot product for quantized vectors. This is not cosine similarity.
pub fn dot_i8(left: &[i8], right: &[i8]) -> Option<i64> {
    if left.len() != right.len() || left.is_empty() {
        return None;
    }
    left.iter().zip(right).try_fold(0i64, |sum, (a, b)| {
        sum.checked_add(i64::from(*a) * i64::from(*b))
    })
}
