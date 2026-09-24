//! B+Tree slotted page layout, cell encoding, and node navigation.
//!
//! Organizes table rows (RowID -> Payload) and secondary indexes (Key -> RowID)
//! into contiguous slotted pages.

pub mod engine;
pub mod varint;

use crate::error::{Error, Result};
use crate::pager::{PageId, Pager};
pub use engine::*;
pub use varint::{decode_varint, encode_varint};

/// Classification of page types in TapirusDB
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageType {
    /// Interior table B+Tree node (Points to child pages)
    TableInterior = 0x01,
    /// Leaf table B+Tree node (Stores actual user row payload)
    TableLeaf = 0x02,
    /// Interior index B+Tree node
    IndexInterior = 0x03,
    /// Leaf index B+Tree node (Stores Key -> RowID)
    IndexLeaf = 0x04,
    /// Native Vector Index Node (HNSW graph adjacency list)
    VectorIndex = 0x05,
    /// Linked Overflow Page for payloads > Page Size
    Overflow = 0x06,
    /// Free page available for recycling
    Freelist = 0x07,
}

impl PageType {
    /// Parse page type from single byte
    pub fn from_u8(b: u8) -> Result<Self> {
        match b {
            0x01 => Ok(Self::TableInterior),
            0x02 => Ok(Self::TableLeaf),
            0x03 => Ok(Self::IndexInterior),
            0x04 => Ok(Self::IndexLeaf),
            0x05 => Ok(Self::VectorIndex),
            0x06 => Ok(Self::Overflow),
            0x07 => Ok(Self::Freelist),
            other => Err(Error::Corrupted(format!("Unknown page type: {other:#x}"))),
        }
    }

    /// Check if this page is an interior node (has right-child pointer)
    pub fn is_interior(&self) -> bool {
        matches!(self, Self::TableInterior | Self::IndexInterior)
    }

    /// Check if this page is a leaf node
    pub fn is_leaf(&self) -> bool {
        matches!(self, Self::TableLeaf | Self::IndexLeaf)
    }
}

/// The Header of a Slotted Page (8 bytes for Leaf, 12 bytes for Interior)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageHeader {
    /// Type of page
    pub page_type: PageType,
    /// Flags for future expansion
    pub flags: u8,
    /// Number of cell pointers in page
    pub num_cells: u16,
    /// Offset pointing to start of cell content area
    pub cell_content_offset: u16,
    /// Offset to first freeblock within fragmented space
    pub first_freeblock: u16,
    /// Rightmost child pointer (Interior pages only)
    pub right_child: Option<PageId>,
}

impl PageHeader {
    /// Standard header size in bytes (8 for Leaf, 12 for Interior)
    pub fn header_size(&self) -> usize {
        if self.page_type.is_interior() {
            12
        } else {
            8
        }
    }

    /// Serialize header to byte vector
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(12);
        buf.push(self.page_type as u8);
        buf.push(self.flags);
        buf.extend_from_slice(&self.num_cells.to_le_bytes());
        buf.extend_from_slice(&self.cell_content_offset.to_le_bytes());
        buf.extend_from_slice(&self.first_freeblock.to_le_bytes());

        if self.page_type.is_interior() {
            let rc = self.right_child.unwrap_or(0);
            buf.extend_from_slice(&rc.to_le_bytes());
        }

        buf
    }

    /// Parse header from byte slice
    pub fn from_bytes(slice: &[u8]) -> Result<Self> {
        if slice.len() < 8 {
            return Err(Error::Corrupted("Page header slice too short (<8 bytes)".into()));
        }

        let page_type = PageType::from_u8(slice[0])?;
        let flags = slice[1];
        let num_cells = u16::from_le_bytes([slice[2], slice[3]]);
        let cell_content_offset = u16::from_le_bytes([slice[4], slice[5]]);
        let first_freeblock = u16::from_le_bytes([slice[6], slice[7]]);

        let right_child = if page_type.is_interior() {
            if slice.len() < 12 {
                return Err(Error::Corrupted("Interior page header too short (<12 bytes)".into()));
            }
            let child = u32::from_le_bytes([slice[8], slice[9], slice[10], slice[11]]);
            Some(child)
        } else {
            None
        };

        Ok(Self {
            page_type,
            flags,
            num_cells,
            cell_content_offset,
            first_freeblock,
            right_child,
        })
    }

    /// Calculate available unallocated free space in the page, taking into account any page header offset
    pub fn free_space(&self, page_size: usize, header_offset: usize) -> usize {
        let cell_pointer_end = header_offset + self.header_size() + (self.num_cells as usize * 2);
        let content_start = if self.cell_content_offset == 0 {
            page_size.saturating_sub(PAGE_RESERVED_TRAILER)
        } else {
            self.cell_content_offset as usize
        };

        content_start.saturating_sub(cell_pointer_end)
    }
}

/// Reserved bytes at the end of each page for cryptographic authentication (AEAD Poly1305 tag)
pub const PAGE_RESERVED_TRAILER: usize = 16;

/// A B+Tree Table Leaf Cell containing a relational row
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableLeafCell {
    /// Monotonic 64-bit Row ID (Primary Key)
    pub row_id: u64,
    /// Column tuple payload bytes
    pub payload: Vec<u8>,
    /// Optional overflow page pointer if payload exceeds in-page threshold
    pub overflow_page: Option<PageId>,
}

impl TableLeafCell {
    /// Serialize cell into byte buffer
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        let mut varint_buf = [0u8; 9];

        // 1. Payload size (Varint)
        let n = encode_varint(self.payload.len() as u64, &mut varint_buf);
        buf.extend_from_slice(&varint_buf[..n]);

        // 2. Row ID (Varint)
        let n = encode_varint(self.row_id, &mut varint_buf);
        buf.extend_from_slice(&varint_buf[..n]);

        // 3. Payload
        buf.extend_from_slice(&self.payload);

        // 4. Overflow page pointer (fixed 4-byte LE: 0 = none, >0 = overflow page id)
        let overflow = self.overflow_page.unwrap_or(0);
        buf.extend_from_slice(&overflow.to_le_bytes());

        buf
    }

    /// Parse cell from byte slice
    pub fn from_bytes(slice: &[u8]) -> Result<Self> {
        let (payload_size, n1) = decode_varint(slice)?;
        let (row_id, n2) = decode_varint(&slice[n1..])?;
        let offset = n1 + n2;

        let payload_len = payload_size as usize;
        if slice.len() < offset + payload_len + 4 {
            return Err(Error::Corrupted("Truncated cell payload or overflow pointer".into()));
        }

        let payload = slice[offset..offset + payload_len].to_vec();
        let overflow_bytes = &slice[offset + payload_len..offset + payload_len + 4];
        let page = u32::from_le_bytes([
            overflow_bytes[0],
            overflow_bytes[1],
            overflow_bytes[2],
            overflow_bytes[3],
        ]);
        let overflow_page = if page > 0 { Some(page) } else { None };

        Ok(Self {
            row_id,
            payload,
            overflow_page,
        })
    }
}

/// A B+Tree Table Interior Cell routing to a child page for keys <= row_id
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableInteriorCell {
    /// Left child page pointer (routing keys <= row_id)
    pub left_child: PageId,
    /// Highest row ID stored in left child subtree
    pub row_id: u64,
}

impl TableInteriorCell {
    /// Serialize cell into byte buffer
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(13);
        buf.extend_from_slice(&self.left_child.to_le_bytes());
        let mut varint_buf = [0u8; 9];
        let n = encode_varint(self.row_id, &mut varint_buf);
        buf.extend_from_slice(&varint_buf[..n]);
        buf
    }

    /// Parse cell from byte slice
    pub fn from_bytes(slice: &[u8]) -> Result<Self> {
        if slice.len() < 4 {
            return Err(Error::Corrupted("Interior cell slice too short (<4 bytes)".into()));
        }
        let left_child = u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]);
        let (row_id, _) = decode_varint(&slice[4..])?;
        Ok(Self { left_child, row_id })
    }
}

/// Abstract B+Tree Engine Trait defining navigation, insertion, and lookup
pub trait BTreeEngine {
    /// Search for a row by RowID in the B+Tree rooted at `root`
    fn search(&self, pager: &mut Pager, root: PageId, key: u64) -> Result<Option<Vec<u8>>>;

    /// Insert a record with `key` and `payload` into the tree
    fn insert(&mut self, pager: &mut Pager, root: PageId, key: u64, payload: &[u8]) -> Result<()>;

    /// Delete a record by key
    fn delete(&mut self, pager: &mut Pager, root: PageId, key: u64) -> Result<bool>;
}

impl BTreeEngine for engine::BTreeStorage {
    fn search(&self, pager: &mut Pager, root: PageId, key: u64) -> Result<Option<Vec<u8>>> {
        self.search(pager, root, key)
    }

    fn insert(&mut self, pager: &mut Pager, root: PageId, key: u64, payload: &[u8]) -> Result<()> {
        self.insert(pager, root, key, payload)
    }

    fn delete(&mut self, pager: &mut Pager, root: PageId, key: u64) -> Result<bool> {
        self.delete(pager, root, key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_leaf_page_header_roundtrip() {
        let header = PageHeader {
            page_type: PageType::TableLeaf,
            flags: 0,
            num_cells: 5,
            cell_content_offset: 3800,
            first_freeblock: 0,
            right_child: None,
        };

        let bytes = header.to_bytes();
        assert_eq!(bytes.len(), 8);

        let parsed = PageHeader::from_bytes(&bytes).expect("Failed to parse leaf header");
        assert_eq!(header, parsed);
        assert_eq!(header.free_space(4096, 0), 3800 - (8 + 10));
    }

    #[test]
    fn test_interior_page_header_roundtrip() {
        let header = PageHeader {
            page_type: PageType::TableInterior,
            flags: 0,
            num_cells: 3,
            cell_content_offset: 3500,
            first_freeblock: 0,
            right_child: Some(42),
        };

        let bytes = header.to_bytes();
        assert_eq!(bytes.len(), 12);

        let parsed = PageHeader::from_bytes(&bytes).expect("Failed to parse interior header");
        assert_eq!(header, parsed);
        assert_eq!(parsed.right_child, Some(42));
    }

    #[test]
    fn test_table_leaf_cell_roundtrip() {
        let cell = TableLeafCell {
            row_id: 1024,
            payload: b"Hello TapirusDB!".to_vec(),
            overflow_page: None,
        };

        let bytes = cell.to_bytes();
        let parsed = TableLeafCell::from_bytes(&bytes).expect("Failed to parse cell");
        assert_eq!(cell, parsed);
    }

    #[test]
    fn test_interior_cell_roundtrip() {
        let cell = TableInteriorCell {
            left_child: 7,
            row_id: 99999,
        };
        let bytes = cell.to_bytes();
        let parsed = TableInteriorCell::from_bytes(&bytes).expect("Failed to parse interior cell");
        assert_eq!(cell, parsed);
    }

    #[test]
    fn test_btree_storage_insert_search_and_split() {
        let mut pager = Pager::open_in_memory(4096, 128).expect("Pager open");
        let table_root = pager.allocate_page().expect("Alloc root page");

        let mut page_data = vec![0u8; 4096];
        engine::init_leaf_page(&mut page_data, table_root);
        pager.write_page(table_root, &page_data).expect("Init root");

        let mut btree = engine::BTreeStorage::new();

        // Insert 100 rows to force multiple page splits (a 4KB page holds ~35-40 records of 100B)
        for i in 1..=100u64 {
            let payload = format!("Record #{i} with payload padding to test page split threshold").into_bytes();
            btree.insert(&mut pager, table_root, i, &payload).expect("Insert record");
        }

        // Search each inserted record
        for i in 1..=100u64 {
            let res = btree.search(&mut pager, table_root, i).expect("Search record");
            assert!(res.is_some(), "Record {i} must be found");
            let expected = format!("Record #{i} with payload padding to test page split threshold").into_bytes();
            assert_eq!(res.unwrap(), expected);
        }

        // Search non-existent record
        assert!(btree.search(&mut pager, table_root, 9999).unwrap().is_none());

        // Full sequential scan
        let scanned = btree.scan(&mut pager, table_root).expect("Scan table");
        assert_eq!(scanned.len(), 100);
        for (idx, cell) in scanned.iter().enumerate() {
            assert_eq!(cell.row_id, (idx + 1) as u64);
        }
    }

    #[test]
    fn test_btree_storage_delete_and_space_reclamation() {
        let mut pager = Pager::open_in_memory(4096, 128).expect("Pager open");
        let table_root = pager.allocate_page().expect("Alloc root page");

        let mut page_data = vec![0u8; 4096];
        engine::init_leaf_page(&mut page_data, table_root);
        pager.write_page(table_root, &page_data).expect("Init root");

        let mut btree = engine::BTreeStorage::new();

        for i in 1..=10u64 {
            let payload = format!("Payload {i}").into_bytes();
            btree.insert(&mut pager, table_root, i, &payload).expect("Insert");
        }

        // Verify initial state
        assert_eq!(btree.scan(&mut pager, table_root).unwrap().len(), 10);

        // Delete row 5
        let deleted = btree.delete(&mut pager, table_root, 5).expect("Delete 5");
        assert!(deleted, "Record 5 must be reported deleted");

        // Verify row 5 is gone on search and scan
        assert!(btree.search(&mut pager, table_root, 5).unwrap().is_none());
        let scanned_after = btree.scan(&mut pager, table_root).unwrap();
        assert_eq!(scanned_after.len(), 9);
        assert!(!scanned_after.iter().any(|c| c.row_id == 5));

        // Re-delete 5: should return false
        let re_del = btree.delete(&mut pager, table_root, 5).expect("Re-delete 5");
        assert!(!re_del, "Deleting already deleted key should return false");

        // Verify other keys intact
        assert!(btree.search(&mut pager, table_root, 1).unwrap().is_some());
        assert!(btree.search(&mut pager, table_root, 10).unwrap().is_some());
    }
}
