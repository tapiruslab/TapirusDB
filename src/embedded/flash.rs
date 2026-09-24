//! # Bare-Metal Embedded Flash Storage Abstraction & Test Harness (Tier B)
//!
//! Defines the abstract `FlashBlockDevice` trait and `MicroPager` for memory-constrained
//! environments (512B / 1024B blocks).
//!
//! NOTE: Physical SPI NOR Flash hardware drivers (e.g. W25Q128 over embedded-hal SPI) must implement
//! the `FlashBlockDevice` trait for specific microcontrollers. The included `RamBlockDevice`
//! is an in-memory test double / simulator for development and unit testing.

use crate::error::{Error, Result};
use crate::vector::QuantizedVector8;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Micro-block magic header for embedded bare-metal flash: `b"TAPIRMIC"`
pub const MICRO_DATABASE_MAGIC: [u8; 8] = *b"TAPIRMIC";

/// Default block size for memory-constrained microcontrollers (512 bytes)
pub const DEFAULT_MICRO_BLOCK_SIZE: usize = 512;

/// Abstract block device trait for SPI NOR Flash, EEPROM, or static RAM partitions
pub trait FlashBlockDevice: Send + Sync {
    /// Read an exact block into the provided buffer
    fn read_block(&mut self, block_idx: u32, buf: &mut [u8]) -> Result<()>;
    /// Write an exact block from the provided buffer
    fn write_block(&mut self, block_idx: u32, buf: &[u8]) -> Result<()>;
    /// Total count of addressable blocks on the device
    fn block_count(&self) -> u32;
    /// Size of each individual block in bytes
    fn block_size(&self) -> usize;
}

/// Static in-memory block device simulating SRAM or PSRAM on an embedded chip
pub struct RamBlockDevice {
    blocks: Vec<Vec<u8>>,
    block_size: usize,
}

impl RamBlockDevice {
    /// Create a new RAM-backed block device with the specified block count and block size
    pub fn new(block_count: u32, block_size: usize) -> Self {
        let blocks = vec![vec![0u8; block_size]; block_count as usize];
        Self { blocks, block_size }
    }
}

impl FlashBlockDevice for RamBlockDevice {
    fn read_block(&mut self, block_idx: u32, buf: &mut [u8]) -> Result<()> {
        let idx = block_idx as usize;
        if idx >= self.blocks.len() {
            return Err(Error::PageNotFound(block_idx));
        }
        if buf.len() != self.block_size {
            return Err(Error::Corrupted(format!(
                "Buffer size mismatch: expected {}, got {}",
                self.block_size,
                buf.len()
            )));
        }
        buf.copy_from_slice(&self.blocks[idx]);
        Ok(())
    }

    fn write_block(&mut self, block_idx: u32, buf: &[u8]) -> Result<()> {
        let idx = block_idx as usize;
        if idx >= self.blocks.len() {
            return Err(Error::PageNotFound(block_idx));
        }
        if buf.len() != self.block_size {
            return Err(Error::Corrupted(format!(
                "Buffer size mismatch: expected {}, got {}",
                self.block_size,
                buf.len()
            )));
        }
        self.blocks[idx].copy_from_slice(buf);
        Ok(())
    }

    fn block_count(&self) -> u32 {
        self.blocks.len() as u32
    }

    fn block_size(&self) -> usize {
        self.block_size
    }
}

/// Header structure stored at Block 0 on the flash device
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MicroHeader {
    /// Magic signature bytes
    pub magic: [u8; 8],
    /// Micro-format version
    pub version: u16,
    /// Configured block size in bytes (e.g. 512, 1024)
    pub block_size: u16,
    /// Total number of blocks allocated
    pub total_blocks: u32,
    /// Active records counter
    pub record_count: u32,
}

impl MicroHeader {
    /// Serialize header to byte buffer
    pub fn to_bytes(&self) -> [u8; 32] {
        let mut buf = [0u8; 32];
        buf[0..8].copy_from_slice(&self.magic);
        buf[8..10].copy_from_slice(&self.version.to_le_bytes());
        buf[10..12].copy_from_slice(&self.block_size.to_le_bytes());
        buf[12..16].copy_from_slice(&self.total_blocks.to_le_bytes());
        buf[16..20].copy_from_slice(&self.record_count.to_le_bytes());
        buf
    }

    /// Deserialize header from byte buffer
    pub fn from_bytes(buf: &[u8]) -> Result<Self> {
        if buf.len() < 32 {
            return Err(Error::Corrupted("MicroHeader buffer too short".into()));
        }
        let mut magic = [0u8; 8];
        magic.copy_from_slice(&buf[0..8]);
        if magic != MICRO_DATABASE_MAGIC {
            return Err(Error::Corrupted("Invalid MicroDatabase magic header".into()));
        }
        let version = u16::from_le_bytes([buf[8], buf[9]]);
        let block_size = u16::from_le_bytes([buf[10], buf[11]]);
        let total_blocks = u32::from_le_bytes([buf[12], buf[13], buf[14], buf[15]]);
        let record_count = u32::from_le_bytes([buf[16], buf[17], buf[18], buf[19]]);

        Ok(Self {
            magic,
            version,
            block_size,
            total_blocks,
            record_count,
        })
    }
}

/// Ultra-compact Pager optimized for bare-metal flash and low SRAM (< 512 KB)
pub struct MicroPager<D: FlashBlockDevice> {
    device: D,
    header: MicroHeader,
}

impl<D: FlashBlockDevice> MicroPager<D> {
    /// Format or open a MicroPager instance over a block device
    pub fn open(mut device: D) -> Result<Self> {
        let block_size = device.block_size();
        let mut block0 = vec![0u8; block_size];
        device.read_block(0, &mut block0)?;

        let header = if block0[0..8] == MICRO_DATABASE_MAGIC {
            MicroHeader::from_bytes(&block0[0..32])?
        } else {
            // Fresh flash format
            let header = MicroHeader {
                magic: MICRO_DATABASE_MAGIC,
                version: 1,
                block_size: block_size as u16,
                total_blocks: device.block_count(),
                record_count: 0,
            };
            block0[0..32].copy_from_slice(&header.to_bytes());
            device.write_block(0, &block0)?;
            header
        };

        Ok(Self { device, header })
    }

    /// Return reference to header
    pub fn header(&self) -> &MicroHeader {
        &self.header
    }

    /// Return block size
    pub fn block_size(&self) -> usize {
        self.header.block_size as usize
    }

    /// Read raw block by 0-indexed index
    pub fn read_block(&mut self, idx: u32) -> Result<Vec<u8>> {
        let mut buf = vec![0u8; self.block_size()];
        self.device.read_block(idx, &mut buf)?;
        Ok(buf)
    }

    /// Write raw block by 0-indexed index
    pub fn write_block(&mut self, idx: u32, data: &[u8]) -> Result<()> {
        self.device.write_block(idx, data)
    }
}

/// High-level embedded database for microcontrollers (Sensor Logs, Micro-Vectors, Entity Graph)
pub struct MicroDatabase<D: FlashBlockDevice> {
    pager: MicroPager<D>,
    records: Vec<(u32, u64, f32, String)>, // (sensor_id, timestamp, val, label)
    vectors: HashMap<u32, QuantizedVector8>, // id -> 8-bit quantized vector
    edges: Vec<(u32, u32, String)>,         // (from, to, relation)
}

impl MicroDatabase<RamBlockDevice> {
    /// Initialize a transient MicroDatabase in RAM
    pub fn open_ram(block_count: u32, block_size: usize) -> Result<Self> {
        let dev = RamBlockDevice::new(block_count, block_size);
        Self::open(dev)
    }
}

impl<D: FlashBlockDevice> MicroDatabase<D> {
    /// Open a MicroDatabase on any hardware block device (SPI Flash / NOR Flash / SRAM)
    pub fn open(device: D) -> Result<Self> {
        let pager = MicroPager::open(device)?;
        Ok(Self {
            pager,
            records: Vec::new(),
            vectors: HashMap::new(),
            edges: Vec::new(),
        })
    }

    /// Store a time-series sensor reading (e.g. Temperature, Lidar range, Wheel speed)
    pub fn store_sensor_record(
        &mut self,
        sensor_id: u32,
        timestamp: u64,
        value: f32,
        label: &str,
    ) -> Result<()> {
        self.records.push((sensor_id, timestamp, value, label.to_string()));

        // Flush checkpoint to Block 1 periodically
        if self.records.len() % 10 == 0 {
            let mut buf = vec![0u8; self.pager.block_size()];
            let count_bytes = (self.records.len() as u32).to_le_bytes();
            buf[0..4].copy_from_slice(&count_bytes);
            self.pager.write_block(1, &buf)?;
        }

        Ok(())
    }

    /// Ingest an AI feature vector embedding with 8-bit quantization (75% RAM savings)
    pub fn add_micro_vector(&mut self, id: u32, vector: &[f32]) -> Result<()> {
        let q8 = QuantizedVector8::quantize(vector);
        self.vectors.insert(id, q8);
        Ok(())
    }

    /// Search the nearest micro-vectors using asymmetric Euclidean distance
    pub fn search_micro_vector(&self, query: &[f32], limit: usize) -> Vec<(u32, f32)> {
        let mut scored: Vec<(u32, f32)> = self
            .vectors
            .iter()
            .map(|(&id, q8)| {
                let dist = q8.asymmetric_l2_distance_squared(query);
                (id, dist)
            })
            .collect();

        scored.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(limit);
        scored
    }

    /// Connect two embedded entities in the local knowledge graph (e.g. Sensor -> Room -> Actuator)
    pub fn link_entities(&mut self, from_id: u32, to_id: u32, relation: &str) -> Result<()> {
        self.edges.push((from_id, to_id, relation.to_string()));
        Ok(())
    }

    /// Retrieve connected graph relationships for an entity
    pub fn get_entity_links(&self, id: u32) -> Vec<(u32, String)> {
        self.edges
            .iter()
            .filter_map(|(from, to, rel)| {
                if *from == id {
                    Some((*to, rel.clone()))
                } else if *to == id {
                    Some((*from, rel.clone()))
                } else {
                    None
                }
            })
            .collect()
    }

    /// Total count of sensor records stored
    pub fn record_count(&self) -> usize {
        self.records.len()
    }

    /// Total count of quantized vectors stored
    pub fn vector_count(&self) -> usize {
        self.vectors.len()
    }
}
