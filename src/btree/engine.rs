//! Concrete B+Tree storage engine implementation for TapirusDB.
//!
//! Handles slotted page layout, binary search within pages, cell insertion,
//! node splitting, and hierarchical tree traversal.

use crate::btree::{
    decode_varint, encode_varint, PageHeader, PageType, TableInteriorCell, TableLeafCell,
    PAGE_RESERVED_TRAILER,
};
use crate::error::{Error, Result};
use crate::pager::{PageId, Pager, DATABASE_HEADER_SIZE};

/// Returns the byte offset within a page where the B+Tree PageHeader begins.
/// Page 1 reserves bytes 0..99 for the DatabaseHeader.
#[inline]
pub fn page_header_offset(page_id: PageId) -> usize {
    if page_id == 1 {
        DATABASE_HEADER_SIZE
    } else {
        0
    }
}

/// Initialize an empty leaf page
pub fn init_leaf_page(page_buf: &mut [u8], page_id: PageId) {
    let offset = page_header_offset(page_id);
    let header = PageHeader {
        page_type: PageType::TableLeaf,
        flags: 0,
        num_cells: 0,
        cell_content_offset: (page_buf.len() - PAGE_RESERVED_TRAILER) as u16,
        first_freeblock: 0,
        right_child: None,
    };
    let header_bytes = header.to_bytes();
    page_buf[offset..offset + header_bytes.len()].copy_from_slice(&header_bytes);
}

/// Initialize an empty interior page with a right child
pub fn init_interior_page(page_buf: &mut [u8], page_id: PageId, right_child: PageId) {
    let offset = page_header_offset(page_id);
    let header = PageHeader {
        page_type: PageType::TableInterior,
        flags: 0,
        num_cells: 0,
        cell_content_offset: (page_buf.len() - PAGE_RESERVED_TRAILER) as u16,
        first_freeblock: 0,
        right_child: Some(right_child),
    };
    let header_bytes = header.to_bytes();
    page_buf[offset..offset + header_bytes.len()].copy_from_slice(&header_bytes);
}

/// Read the PageHeader from a raw page buffer
pub fn read_page_header(page_buf: &[u8], page_id: PageId) -> Result<PageHeader> {
    let offset = page_header_offset(page_id);
    if page_buf.len() < offset + 8 {
        return Err(Error::Corrupted("Page buffer too small for header".into()));
    }
    PageHeader::from_bytes(&page_buf[offset..])
}

/// Write the PageHeader into a raw page buffer
pub fn write_page_header(
    page_buf: &mut [u8],
    page_id: PageId,
    header: &PageHeader,
) -> Result<()> {
    let offset = page_header_offset(page_id);
    let bytes = header.to_bytes();
    if page_buf.len() < offset + bytes.len() {
        return Err(Error::Corrupted("Page buffer too small to write header".into()));
    }
    page_buf[offset..offset + bytes.len()].copy_from_slice(&bytes);
    Ok(())
}

/// Read cell pointer at index `cell_idx` (0-based)
pub fn read_cell_pointer(page_buf: &[u8], page_id: PageId, cell_idx: usize) -> Result<u16> {
    let offset = page_header_offset(page_id);
    let header = read_page_header(page_buf, page_id)?;
    if cell_idx >= header.num_cells as usize {
        return Err(Error::Corrupted(format!(
            "Cell index {cell_idx} out of range (num_cells: {})",
            header.num_cells
        )));
    }
    let ptr_offset = offset + header.header_size() + (cell_idx * 2);
    Ok(u16::from_le_bytes([
        page_buf[ptr_offset],
        page_buf[ptr_offset + 1],
    ]))
}

/// Read a TableLeafCell from a leaf page at `cell_idx`
pub fn read_leaf_cell(page_buf: &[u8], page_id: PageId, cell_idx: usize) -> Result<TableLeafCell> {
    let ptr = read_cell_pointer(page_buf, page_id, cell_idx)? as usize;
    if ptr >= page_buf.len() {
        return Err(Error::Corrupted("Cell pointer points past end of page".into()));
    }
    TableLeafCell::from_bytes(&page_buf[ptr..])
}

/// Read a TableInteriorCell from an interior page at `cell_idx`
pub fn read_interior_cell(
    page_buf: &[u8],
    page_id: PageId,
    cell_idx: usize,
) -> Result<TableInteriorCell> {
    let ptr = read_cell_pointer(page_buf, page_id, cell_idx)? as usize;
    if ptr >= page_buf.len() {
        return Err(Error::Corrupted("Cell pointer points past end of page".into()));
    }
    TableInteriorCell::from_bytes(&page_buf[ptr..])
}

/// Read only the row_id of a leaf cell at `cell_idx` without allocating a payload buffer
#[inline(always)]
pub fn read_leaf_cell_row_id(page_buf: &[u8], page_id: PageId, cell_idx: usize) -> Result<u64> {
    let ptr = read_cell_pointer(page_buf, page_id, cell_idx)? as usize;
    if ptr >= page_buf.len() {
        return Err(Error::Corrupted("Cell pointer points past end of page".into()));
    }
    let slice = &page_buf[ptr..];
    let (_payload_size, n1) = decode_varint(slice)?;
    if slice.len() <= n1 {
        return Err(Error::Corrupted("Truncated cell row_id".into()));
    }
    let (row_id, _n2) = decode_varint(&slice[n1..])?;
    Ok(row_id)
}

/// Binary search for a row_id in a leaf page.
/// Returns Ok(index) if found, Err(insertion_index) if not found.
pub fn find_leaf_cell_index_by_key(
    page_buf: &[u8],
    page_id: PageId,
    key: u64,
) -> Result<std::result::Result<usize, usize>> {
    let header = read_page_header(page_buf, page_id)?;
    let num_cells = header.num_cells as usize;
    if num_cells == 0 {
        return Ok(Err(0));
    }

    // Fast-path: Check if appending after the last cell (common for auto-increment / bulk insert)
    let last_row_id = read_leaf_cell_row_id(page_buf, page_id, num_cells - 1)?;
    if key > last_row_id {
        return Ok(Err(num_cells));
    } else if key == last_row_id {
        return Ok(Ok(num_cells - 1));
    }

    let mut low = 0;
    let mut high = num_cells;

    while low < high {
        let mid = low + (high - low) / 2;
        let row_id = read_leaf_cell_row_id(page_buf, page_id, mid)?;
        if row_id == key {
            return Ok(Ok(mid));
        } else if row_id < key {
            low = mid + 1;
        } else {
            high = mid;
        }
    }

    Ok(Err(low))
}

/// Try to insert a leaf cell into a leaf page.
/// Returns `Ok(true)` if inserted, or `Ok(false)` if there is not enough free space.
pub fn insert_leaf_cell_into_page(
    page_buf: &mut [u8],
    page_id: PageId,
    cell: &TableLeafCell,
) -> Result<bool> {
    insert_leaf_cell_raw(
        page_buf,
        page_id,
        cell.row_id,
        &cell.payload,
        cell.overflow_page,
    )
}

/// Try to insert a leaf cell with raw borrowed payload slice into a leaf page.
/// Avoids allocating a TableLeafCell and its Vec<u8> payload on the heap.
pub fn insert_leaf_cell_raw(
    page_buf: &mut [u8],
    page_id: PageId,
    row_id: u64,
    payload: &[u8],
    overflow_page: Option<PageId>,
) -> Result<bool> {
    let page_size = page_buf.len();
    let mut header = read_page_header(page_buf, page_id)?;

    let mut varint_buf1 = [0u8; 9];
    let mut varint_buf2 = [0u8; 9];
    let n1 = encode_varint(payload.len() as u64, &mut varint_buf1);
    let n2 = encode_varint(row_id, &mut varint_buf2);
    let cell_size = n1 + n2 + payload.len() + 4;
    let needed_space = cell_size + 2; // Cell payload + 2-byte pointer

    let offset = page_header_offset(page_id);
    if header.free_space(page_size, offset) < needed_space {
        return Ok(false);
    }

    // Check if key already exists (update in place or replace)
    let search_res = find_leaf_cell_index_by_key(page_buf, page_id, row_id)?;
    let insert_idx = match search_res {
        Ok(existing_idx) => {
            let ptr_array_start = offset + header.header_size();
            for i in existing_idx..(header.num_cells as usize - 1) {
                let p1 = ptr_array_start + i * 2;
                let p2 = ptr_array_start + (i + 1) * 2;
                page_buf[p1] = page_buf[p2];
                page_buf[p1 + 1] = page_buf[p2 + 1];
            }
            header.num_cells -= 1;
            existing_idx
        }
        Err(idx) => idx,
    };

    let mut cur_content_offset = if header.cell_content_offset == 0 {
        (page_size - PAGE_RESERVED_TRAILER) as u16
    } else {
        header.cell_content_offset
    };

    cur_content_offset = cur_content_offset
        .checked_sub(cell_size as u16)
        .ok_or_else(|| Error::Corrupted("Cell offset arithmetic underflow".into()))?;

    // Write cell payload at bottom of page directly without intermediate Vec allocation
    let content_start = cur_content_offset as usize;
    page_buf[content_start..content_start + n1].copy_from_slice(&varint_buf1[..n1]);
    page_buf[content_start + n1..content_start + n1 + n2].copy_from_slice(&varint_buf2[..n2]);
    let payload_start = content_start + n1 + n2;
    page_buf[payload_start..payload_start + payload.len()].copy_from_slice(payload);
    let overflow_start = payload_start + payload.len();
    let overflow = overflow_page.unwrap_or(0);
    page_buf[overflow_start..overflow_start + 4].copy_from_slice(&overflow.to_le_bytes());

    // Shift cell pointer array to open slot at insert_idx (only if not appending at end)
    let ptr_array_start = offset + header.header_size();
    let old_num_cells = header.num_cells as usize;
    if insert_idx < old_num_cells {
        for i in (insert_idx..old_num_cells).rev() {
            let p_curr = ptr_array_start + (i + 1) * 2;
            let p_prev = ptr_array_start + i * 2;
            page_buf[p_curr] = page_buf[p_prev];
            page_buf[p_curr + 1] = page_buf[p_prev + 1];
        }
    }

    // Write new pointer
    let p_target = ptr_array_start + insert_idx * 2;
    page_buf[p_target..p_target + 2].copy_from_slice(&cur_content_offset.to_le_bytes());

    // Update header
    header.num_cells += 1;
    header.cell_content_offset = cur_content_offset;
    write_page_header(page_buf, page_id, &header)?;

    Ok(true)
}

/// Defragment and compact a leaf page, reclaiming all deleted cell space
/// and zeroing unallocated regions for memory privacy and GDPR compliance.
pub fn defragment_leaf_page(page_buf: &mut [u8], page_id: PageId) -> Result<()> {
    let mut header = read_page_header(page_buf, page_id)?;
    let num_cells = header.num_cells as usize;
    let page_size = page_buf.len();
    let offset = page_header_offset(page_id);

    if num_cells == 0 {
        header.cell_content_offset = (page_size - PAGE_RESERVED_TRAILER) as u16;
        header.first_freeblock = 0;
        write_page_header(page_buf, page_id, &header)?;
        let clear_start = offset + header.header_size();
        let clear_end = page_size - PAGE_RESERVED_TRAILER;
        if clear_end > clear_start {
            page_buf[clear_start..clear_end].fill(0);
        }
        return Ok(());
    }

    let mut cells = Vec::with_capacity(num_cells);
    for i in 0..num_cells {
        cells.push(read_leaf_cell(page_buf, page_id, i)?);
    }

    let mut content_offset = (page_size - PAGE_RESERVED_TRAILER) as u16;
    let ptr_start = offset + header.header_size();

    // Zero out old content and pointer slack
    let clear_start = ptr_start + num_cells * 2;
    let clear_end = page_size - PAGE_RESERVED_TRAILER;
    if clear_end > clear_start {
        page_buf[clear_start..clear_end].fill(0);
    }

    for (i, cell) in cells.iter().enumerate() {
        let bytes = cell.to_bytes();
        content_offset = content_offset
            .checked_sub(bytes.len() as u16)
            .ok_or_else(|| Error::Corrupted("Cell offset underflow during defragmentation".into()))?;
        let c_start = content_offset as usize;
        page_buf[c_start..c_start + bytes.len()].copy_from_slice(&bytes);
        let p_loc = ptr_start + i * 2;
        page_buf[p_loc..p_loc + 2].copy_from_slice(&content_offset.to_le_bytes());
    }

    header.cell_content_offset = content_offset;
    header.first_freeblock = 0;
    write_page_header(page_buf, page_id, &header)?;
    Ok(())
}

/// Remove a leaf cell matching `key` from a leaf page buffer.
/// Returns Ok(true) if deleted, Ok(false) if key was not found in page.
pub fn delete_leaf_cell_from_page(
    page_buf: &mut [u8],
    page_id: PageId,
    key: u64,
) -> Result<bool> {
    let mut header = read_page_header(page_buf, page_id)?;
    let search_res = find_leaf_cell_index_by_key(page_buf, page_id, key)?;
    match search_res {
        Ok(idx) => {
            if idx >= header.num_cells as usize || header.num_cells == 0 {
                return Ok(false);
            }
            let offset = page_header_offset(page_id);
            let ptr_array_start = offset + header.header_size();
            let limit = (header.num_cells as usize).saturating_sub(1);
            for i in idx..limit {
                let p1 = ptr_array_start + i * 2;
                let p2 = ptr_array_start + (i + 1) * 2;
                if p2 + 1 < page_buf.len() {
                    page_buf[p1] = page_buf[p2];
                    page_buf[p1 + 1] = page_buf[p2 + 1];
                }
            }
            header.num_cells = header.num_cells.saturating_sub(1);
            write_page_header(page_buf, page_id, &header)?;
            defragment_leaf_page(page_buf, page_id)?;
            Ok(true)
        }
        Err(_) => Ok(false),
    }
}

/// Insert an interior cell into an interior page
pub fn insert_interior_cell_into_page(
    page_buf: &mut [u8],
    page_id: PageId,
    cell: &TableInteriorCell,
) -> Result<bool> {
    let page_size = page_buf.len();
    let mut header = read_page_header(page_buf, page_id)?;
    let cell_bytes = cell.to_bytes();
    let cell_size = cell_bytes.len();
    let needed_space = cell_size + 2;

    let offset = page_header_offset(page_id);
    if header.free_space(page_size, offset) < needed_space {
        return Ok(false);
    }

    let mut insert_idx = header.num_cells as usize;

    // Find sorted insertion index by row_id
    for i in 0..header.num_cells as usize {
        let existing = read_interior_cell(page_buf, page_id, i)?;
        if cell.row_id < existing.row_id {
            insert_idx = i;
            break;
        }
    }

    let mut cur_content_offset = if header.cell_content_offset == 0 {
        (page_size - PAGE_RESERVED_TRAILER) as u16
    } else {
        header.cell_content_offset
    };

    cur_content_offset = cur_content_offset
        .checked_sub(cell_size as u16)
        .ok_or_else(|| Error::Corrupted("Interior cell offset arithmetic underflow".into()))?;

    let content_start = cur_content_offset as usize;
    page_buf[content_start..content_start + cell_size].copy_from_slice(&cell_bytes);

    let ptr_array_start = offset + header.header_size();
    let old_num_cells = header.num_cells as usize;
    for i in (insert_idx..old_num_cells).rev() {
        let p_curr = ptr_array_start + (i + 1) * 2;
        let p_prev = ptr_array_start + i * 2;
        page_buf[p_curr] = page_buf[p_prev];
        page_buf[p_curr + 1] = page_buf[p_prev + 1];
    }

    let p_target = ptr_array_start + insert_idx * 2;
    page_buf[p_target..p_target + 2].copy_from_slice(&cur_content_offset.to_le_bytes());

    header.num_cells += 1;
    header.cell_content_offset = cur_content_offset;
    write_page_header(page_buf, page_id, &header)?;

    Ok(true)
}

/// Maximum inline payload bytes stored inside a B+Tree leaf cell before spilling to overflow pages
pub const MAX_INLINE_PAYLOAD: usize = 1024;

/// Write an overflow chain for payload bytes exceeding MAX_INLINE_PAYLOAD
pub fn write_overflow_chain(
    pager: &mut Pager,
    mut remaining_bytes: &[u8],
) -> Result<PageId> {
    let page_size = pager.page_size();
    let max_chunk = page_size.saturating_sub(6 + PAGE_RESERVED_TRAILER);
    if max_chunk == 0 {
        return Err(Error::Corrupted("Page size too small for overflow chaining".into()));
    }
    let mut head_page: Option<PageId> = None;
    let mut prev_page: Option<PageId> = None;

    while !remaining_bytes.is_empty() {
        let chunk_size = remaining_bytes.len().min(max_chunk);
        let chunk = &remaining_bytes[..chunk_size];
        remaining_bytes = &remaining_bytes[chunk_size..];

        let curr_page_id = pager.allocate_page()?;
        if head_page.is_none() {
            head_page = Some(curr_page_id);
        }

        // Link previous page to current page
        if let Some(prev_id) = prev_page {
            let mut prev_buf = pager.read_page(prev_id)?;
            prev_buf[0..4].copy_from_slice(&curr_page_id.to_le_bytes());
            pager.write_page(prev_id, &prev_buf)?;
        }

        let mut page_buf = vec![0u8; page_size];
        page_buf[0..4].copy_from_slice(&0u32.to_le_bytes()); // next = 0
        page_buf[4..6].copy_from_slice(&(chunk_size as u16).to_le_bytes());
        page_buf[6..6 + chunk_size].copy_from_slice(chunk);
        pager.write_page(curr_page_id, &page_buf)?;

        prev_page = Some(curr_page_id);
    }

    Ok(head_page.unwrap_or(0))
}

/// Read all chained overflow pages starting from first_page_id
pub fn read_overflow_chain(
    pager: &mut Pager,
    first_page_id: PageId,
) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut curr = first_page_id;
    let page_size = pager.page_size();

    while curr > 0 {
        let page_buf = pager.read_page(curr)?;
        let next_page = u32::from_le_bytes([page_buf[0], page_buf[1], page_buf[2], page_buf[3]]);
        let chunk_len = u16::from_le_bytes([page_buf[4], page_buf[5]]) as usize;
        let max_chunk = page_size.saturating_sub(6 + PAGE_RESERVED_TRAILER);
        if chunk_len > max_chunk {
            return Err(Error::Corrupted(format!(
                "Overflow page {curr} chunk length {chunk_len} exceeds usable capacity {max_chunk}"
            )));
        }
        out.extend_from_slice(&page_buf[6..6 + chunk_len]);
        curr = next_page;
    }

    Ok(out)
}

/// Release and zero out chained overflow pages starting from first_page_id
pub fn free_overflow_chain(
    pager: &mut Pager,
    first_page_id: PageId,
) -> Result<()> {
    let mut curr = first_page_id;
    let page_size = pager.page_size();
    let zero_page = vec![0u8; page_size];

    while curr > 0 {
        let page_buf = pager.read_page(curr)?;
        let next_page = u32::from_le_bytes([page_buf[0], page_buf[1], page_buf[2], page_buf[3]]);
        pager.write_page(curr, &zero_page)?;
        curr = next_page;
    }

    Ok(())
}

/// The concrete TapirusDB B+Tree Storage implementation
#[derive(Debug, Default, Clone)]
pub struct BTreeStorage;

impl BTreeStorage {
    /// Create a new BTreeStorage instance
    pub fn new() -> Self {
        Self
    }

    /// Search for a row by `key` starting at `root_page`
    pub fn search(&self, pager: &mut Pager, root_page: PageId, key: u64) -> Result<Option<Vec<u8>>> {
        let mut curr_page_id = root_page;

        loop {
            enum Step {
                Leaf(Option<(Vec<u8>, Option<u32>)>),
                Interior(PageId),
            }

            let step = pager.with_page(curr_page_id, |page_buf| {
                let header = read_page_header(page_buf, curr_page_id)?;
                match header.page_type {
                    PageType::TableLeaf => {
                        let search_res = find_leaf_cell_index_by_key(page_buf, curr_page_id, key)?;
                        match search_res {
                            Ok(cell_idx) => {
                                let cell = read_leaf_cell(page_buf, curr_page_id, cell_idx)?;
                                Ok(Step::Leaf(Some((cell.payload, cell.overflow_page))))
                            }
                            Err(_) => Ok(Step::Leaf(None)),
                        }
                    }
                    PageType::TableInterior => {
                        let mut next_page = header
                            .right_child
                            .ok_or_else(|| Error::Corrupted("Interior node missing right_child".into()))?;

                        for i in 0..header.num_cells as usize {
                            let cell = read_interior_cell(page_buf, curr_page_id, i)?;
                            if key <= cell.row_id {
                                next_page = cell.left_child;
                                break;
                            }
                        }
                        Ok(Step::Interior(next_page))
                    }
                    other => Err(Error::Corrupted(format!(
                        "Unexpected page type {other:?} encountered during BTree search"
                    ))),
                }
            })?;

            match step {
                Step::Leaf(Some((mut payload, overflow_page))) => {
                    if let Some(overflow_id) = overflow_page {
                        let overflow_data = read_overflow_chain(pager, overflow_id)?;
                        payload.extend_from_slice(&overflow_data);
                    }
                    return Ok(Some(payload));
                }
                Step::Leaf(None) => return Ok(None),
                Step::Interior(next_id) => {
                    curr_page_id = next_id;
                }
            }
        }
    }

    /// Insert a record with `key` and `payload` into the B+Tree rooted at `root_page`.
    pub fn insert(
        &mut self,
        pager: &mut Pager,
        root_page: PageId,
        key: u64,
        payload: &[u8],
    ) -> Result<()> {
        let (inline_payload, overflow_page) = if payload.len() > MAX_INLINE_PAYLOAD {
            let inline = payload[..MAX_INLINE_PAYLOAD].to_vec();
            let overflow_id = write_overflow_chain(pager, &payload[MAX_INLINE_PAYLOAD..])?;
            (std::borrow::Cow::Owned(inline), Some(overflow_id))
        } else {
            (std::borrow::Cow::Borrowed(payload), None)
        };

        // Recursive insert down tree without heap allocation for normal inline cells
        let split_result = self.insert_into_subtree_cow(pager, root_page, key, inline_payload, overflow_page)?;

        if let Some((right_child_id, split_key)) = split_result {
            // The root page itself split!
            let left_child_id = pager.allocate_page()?;
            let old_root_data = pager.read_page(root_page)?;

            if root_page == 1 {
                let header = read_page_header(&old_root_data, root_page)?;
                let mut new_left_data = vec![0u8; pager.page_size()];
                match header.page_type {
                    PageType::TableLeaf => {
                        init_leaf_page(&mut new_left_data, left_child_id);
                        for i in 0..header.num_cells as usize {
                            let cell = read_leaf_cell(&old_root_data, root_page, i)?;
                            insert_leaf_cell_into_page(&mut new_left_data, left_child_id, &cell)?;
                        }
                    }
                    PageType::TableInterior => {
                        init_interior_page(
                            &mut new_left_data,
                            left_child_id,
                            header.right_child.unwrap_or(0),
                        );
                        for i in 0..header.num_cells as usize {
                            let cell = read_interior_cell(&old_root_data, root_page, i)?;
                            insert_interior_cell_into_page(&mut new_left_data, left_child_id, &cell)?;
                        }
                    }
                    _ => {
                        new_left_data.copy_from_slice(&old_root_data);
                    }
                }
                pager.write_page(left_child_id, &new_left_data)?;
            } else {
                pager.write_page(left_child_id, &old_root_data)?;
            }

            let mut new_root_data = vec![0u8; pager.page_size()];
            init_interior_page(&mut new_root_data, root_page, right_child_id);

            let interior_cell = TableInteriorCell {
                left_child: left_child_id,
                row_id: split_key,
            };
            insert_interior_cell_into_page(&mut new_root_data, root_page, &interior_cell)?;
            pager.write_page(root_page, &new_root_data)?;
        }

        Ok(())
    }

    fn insert_into_subtree_cow<'a>(
        &mut self,
        pager: &mut Pager,
        curr_page_id: PageId,
        key: u64,
        payload: std::borrow::Cow<'a, [u8]>,
        overflow_page: Option<PageId>,
    ) -> Result<Option<(PageId, u64)>> {
        let page_type = pager.with_page(curr_page_id, |page_buf| {
            let header = read_page_header(page_buf, curr_page_id)?;
            Ok(header.page_type)
        })?;

        match page_type {
            PageType::TableLeaf => {
                let inserted = pager.modify_page(curr_page_id, |page_buf| {
                    insert_leaf_cell_raw(page_buf, curr_page_id, key, &payload, overflow_page)
                })?;
                if inserted {
                    Ok(None)
                } else {
                    // Leaf page is full! Must split into two pages
                    let page_buf = pager.read_page(curr_page_id)?;
                    let cell = TableLeafCell {
                        row_id: key,
                        payload: payload.into_owned(),
                        overflow_page,
                    };
                    let (right_page_id, split_key) =
                        self.split_leaf_page(pager, curr_page_id, &page_buf, cell)?;
                    Ok(Some((right_page_id, split_key)))
                }
            }
            PageType::TableInterior => {
                let (target_child, child_idx) = pager.with_page(curr_page_id, |page_buf| {
                    let header = read_page_header(page_buf, curr_page_id)?;
                    let num_cells = header.num_cells as usize;
                    let mut tc = header
                        .right_child
                        .ok_or_else(|| Error::Corrupted("Interior node missing right_child".into()))?;
                    let mut c_idx = None;

                    for i in 0..num_cells {
                        let icell = read_interior_cell(page_buf, curr_page_id, i)?;
                        if key <= icell.row_id {
                            tc = icell.left_child;
                            c_idx = Some(i);
                            break;
                        }
                    }
                    Ok((tc, c_idx))
                })?;

                let child_split = self.insert_into_subtree_cow(pager, target_child, key, payload, overflow_page)?;

                if let Some((new_child_id, child_split_key)) = child_split {
                    let new_interior_cell = TableInteriorCell {
                        left_child: target_child,
                        row_id: child_split_key,
                    };

                    let can_fit = pager.modify_page(curr_page_id, |curr_buf| {
                        let ok = insert_interior_cell_into_page(
                            curr_buf,
                            curr_page_id,
                            &new_interior_cell,
                        )?;
                        if ok && child_idx.is_none() {
                            let mut h = read_page_header(curr_buf, curr_page_id)?;
                            h.right_child = Some(new_child_id);
                            write_page_header(curr_buf, curr_page_id, &h)?;
                        }
                        Ok(ok)
                    })?;

                    if can_fit {
                        Ok(None)
                    } else {
                        let curr_buf = pager.read_page(curr_page_id)?;
                        let new_rc = if child_idx.is_none() { Some(new_child_id) } else { None };
                        let (new_interior_page_id, split_key) = self.split_interior_page(
                            pager,
                            curr_page_id,
                            &curr_buf,
                            new_interior_cell,
                            new_rc,
                        )?;
                        Ok(Some((new_interior_page_id, split_key)))
                    }
                } else {
                    Ok(None)
                }
            }
            other => Err(Error::Corrupted(format!("Invalid page type for insertion: {other:?}"))),
        }
    }

    /// Split a full interior page 50/50 into left page and a newly allocated right interior page
    fn split_interior_page(
        &mut self,
        pager: &mut Pager,
        left_page_id: PageId,
        left_page_buf: &[u8],
        new_cell: TableInteriorCell,
        new_right_child: Option<PageId>,
    ) -> Result<(PageId, u64)> {
        let header = read_page_header(left_page_buf, left_page_id)?;
        let mut all_cells = Vec::with_capacity(header.num_cells as usize + 1);

        for i in 0..header.num_cells as usize {
            all_cells.push(read_interior_cell(left_page_buf, left_page_id, i)?);
        }
        all_cells.push(new_cell);
        all_cells.sort_by_key(|c| c.row_id);

        let mid = all_cells.len() / 2;
        let split_key = all_cells[mid].row_id;
        let promoted_left_child = all_cells[mid].left_child;

        let left_cells = &all_cells[..mid];
        let right_cells = &all_cells[mid + 1..];

        let orig_right_child = if let Some(nrc) = new_right_child {
            nrc
        } else {
            header.right_child.unwrap_or(0)
        };

        // 1. Allocate new right interior page
        let right_page_id = pager.allocate_page()?;
        let mut right_page_buf = vec![0u8; pager.page_size()];
        init_interior_page(&mut right_page_buf, right_page_id, orig_right_child);
        for c in right_cells {
            insert_interior_cell_into_page(&mut right_page_buf, right_page_id, c)?;
        }
        pager.write_page(right_page_id, &right_page_buf)?;

        // 2. Re-populate left page
        let mut new_left_buf = vec![0u8; pager.page_size()];
        if left_page_id == 1 {
            new_left_buf[..DATABASE_HEADER_SIZE].copy_from_slice(&left_page_buf[..DATABASE_HEADER_SIZE]);
        }
        init_interior_page(&mut new_left_buf, left_page_id, promoted_left_child);
        for c in left_cells {
            insert_interior_cell_into_page(&mut new_left_buf, left_page_id, c)?;
        }
        pager.write_page(left_page_id, &new_left_buf)?;

        Ok((right_page_id, split_key))
    }

    /// Split a full leaf page 50/50 into current page and a newly allocated right page
    fn split_leaf_page(
        &mut self,
        pager: &mut Pager,
        left_page_id: PageId,
        left_page_buf: &[u8],
        new_cell: TableLeafCell,
    ) -> Result<(PageId, u64)> {
        let header = read_page_header(left_page_buf, left_page_id)?;
        let num_cells = header.num_cells as usize;

        // Sequential Right-Append Split Optimization (SQLite-style right-leaning split):
        // If the new cell's row_id is strictly greater than the last cell on the full page,
        // we leave the current page 100% packed and place only the new cell in the new right page.
        // This cuts B-Tree depth, page allocations, and copying overhead by ~50% on sequential inserts.
        if left_page_id != 1 && num_cells >= 2 {
            let last_ptr = read_cell_pointer(left_page_buf, left_page_id, num_cells - 1)? as usize;
            if last_ptr < left_page_buf.len() {
                let (_, n1) = crate::btree::decode_varint(&left_page_buf[last_ptr..])?;
                let (last_row_id, _) = crate::btree::decode_varint(&left_page_buf[last_ptr + n1..])?;

                if new_cell.row_id > last_row_id {
                    let right_page_id = pager.allocate_page()?;
                    let mut right_page_buf = vec![0u8; pager.page_size()];
                    init_leaf_page(&mut right_page_buf, right_page_id);
                    insert_leaf_cell_into_page(&mut right_page_buf, right_page_id, &new_cell)?;
                    pager.write_page(right_page_id, &right_page_buf)?;

                    return Ok((right_page_id, last_row_id));
                }
            }
        }

        let mut all_cells = Vec::with_capacity(header.num_cells as usize + 1);

        for i in 0..header.num_cells as usize {
            all_cells.push(read_leaf_cell(left_page_buf, left_page_id, i)?);
        }
        all_cells.push(new_cell);
        all_cells.sort_by_key(|c| c.row_id);

        let mid = all_cells.len() / 2;
        let left_cells = &all_cells[..mid];
        let right_cells = &all_cells[mid..];
        let split_key = left_cells.last().unwrap().row_id;

        // 1. Allocate new right leaf page
        let right_page_id = pager.allocate_page()?;
        let mut right_page_buf = vec![0u8; pager.page_size()];
        init_leaf_page(&mut right_page_buf, right_page_id);
        for c in right_cells {
            insert_leaf_cell_into_page(&mut right_page_buf, right_page_id, c)?;
        }
        pager.write_page(right_page_id, &right_page_buf)?;

        // 2. Re-populate left page
        let mut new_left_buf = vec![0u8; pager.page_size()];
        if left_page_id == 1 {
            // Preserve DatabaseHeader on Page 1
            new_left_buf[..DATABASE_HEADER_SIZE].copy_from_slice(&left_page_buf[..DATABASE_HEADER_SIZE]);
        }
        init_leaf_page(&mut new_left_buf, left_page_id);
        for c in left_cells {
            insert_leaf_cell_into_page(&mut new_left_buf, left_page_id, c)?;
        }
        pager.write_page(left_page_id, &new_left_buf)?;

        Ok((right_page_id, split_key))
    }

    /// Perform a full scan across all leaf cells in the B+Tree rooted at `root_page`
    pub fn scan(&self, pager: &mut Pager, root_page: PageId) -> Result<Vec<TableLeafCell>> {
        let mut cells = Vec::new();
        self.scan_with(pager, root_page, |cell| {
            cells.push(cell);
            Ok(true)
        })?;
        Ok(cells)
    }

    /// Perform a streaming scan across leaf cells with early-termination support
    pub fn scan_with<F>(&self, pager: &mut Pager, root_page: PageId, mut callback: F) -> Result<()>
    where
        F: FnMut(TableLeafCell) -> Result<bool>,
    {
        self.scan_subtree_with(pager, root_page, &mut callback)?;
        Ok(())
    }

    /// Perform a zero-copy streaming scan passing borrowed payload slices `(row_id: u64, payload: &[u8])`
    /// without allocating heap `Vec<u8>` per cell.
    pub fn scan_with_ref<F>(&self, pager: &mut Pager, root_page: PageId, mut callback: F) -> Result<()>
    where
        F: FnMut(u64, &[u8]) -> Result<bool>,
    {
        self.scan_subtree_with_ref(pager, root_page, &mut callback)?;
        Ok(())
    }

    fn scan_subtree_with_ref<F>(
        &self,
        pager: &mut Pager,
        curr_page_id: PageId,
        callback: &mut F,
    ) -> Result<bool>
    where
        F: FnMut(u64, &[u8]) -> Result<bool>,
    {
        let page_buf = pager.read_page(curr_page_id)?;
        let header = read_page_header(&page_buf, curr_page_id)?;

        match header.page_type {
            PageType::TableLeaf => {
                for i in 0..header.num_cells as usize {
                    let ptr = read_cell_pointer(&page_buf, curr_page_id, i)? as usize;
                    if ptr >= page_buf.len() {
                        return Err(Error::Corrupted("Cell pointer points past end of page".into()));
                    }
                    let (payload_size, n1) = crate::btree::decode_varint(&page_buf[ptr..])?;
                    let (row_id, n2) = crate::btree::decode_varint(&page_buf[ptr + n1..])?;
                    let offset = ptr + n1 + n2;
                    let payload_len = payload_size as usize;
                    if page_buf.len() < offset + payload_len + 4 {
                        return Err(Error::Corrupted("Truncated cell payload or overflow pointer".into()));
                    }
                    let overflow = u32::from_le_bytes([
                        page_buf[offset + payload_len],
                        page_buf[offset + payload_len + 1],
                        page_buf[offset + payload_len + 2],
                        page_buf[offset + payload_len + 3],
                    ]);

                    if overflow == 0 {
                        let payload_slice = &page_buf[offset..offset + payload_len];
                        if !callback(row_id, payload_slice)? {
                            return Ok(false);
                        }
                    } else {
                        let mut full_payload = page_buf[offset..offset + payload_len].to_vec();
                        let overflow_data = read_overflow_chain(pager, overflow)?;
                        full_payload.extend_from_slice(&overflow_data);
                        if !callback(row_id, &full_payload)? {
                            return Ok(false);
                        }
                    }
                }
                Ok(true)
            }
            PageType::TableInterior => {
                for i in 0..header.num_cells as usize {
                    let icell = read_interior_cell(&page_buf, curr_page_id, i)?;
                    if !self.scan_subtree_with_ref(pager, icell.left_child, callback)? {
                        return Ok(false);
                    }
                }
                if let Some(rc) = header.right_child {
                    if !self.scan_subtree_with_ref(pager, rc, callback)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            other => Err(Error::Corrupted(format!("Invalid page type during scan: {other:?}"))),
        }
    }

    fn scan_subtree_with<F>(
        &self,
        pager: &mut Pager,
        curr_page_id: PageId,
        callback: &mut F,
    ) -> Result<bool>
    where
        F: FnMut(TableLeafCell) -> Result<bool>,
    {
        let page_buf = pager.read_page(curr_page_id)?;
        let header = read_page_header(&page_buf, curr_page_id)?;

        match header.page_type {
            PageType::TableLeaf => {
                for i in 0..header.num_cells as usize {
                    let mut cell = read_leaf_cell(&page_buf, curr_page_id, i)?;
                    if let Some(overflow_id) = cell.overflow_page {
                        let overflow_data = read_overflow_chain(pager, overflow_id)?;
                        cell.payload.extend_from_slice(&overflow_data);
                        cell.overflow_page = None;
                    }
                    if !callback(cell)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            PageType::TableInterior => {
                for i in 0..header.num_cells as usize {
                    let icell = read_interior_cell(&page_buf, curr_page_id, i)?;
                    if !self.scan_subtree_with(pager, icell.left_child, callback)? {
                        return Ok(false);
                    }
                }
                if let Some(rc) = header.right_child {
                    if !self.scan_subtree_with(pager, rc, callback)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            other => Err(Error::Corrupted(format!("Invalid page type during scan: {other:?}"))),
        }
    }

    /// Delete a record matching `key` from the B+Tree rooted at `root_page`.
    /// Returns Ok(true) if the record was found and deleted, Ok(false) otherwise.
    pub fn delete(&mut self, pager: &mut Pager, root_page: PageId, key: u64) -> Result<bool> {
        self.delete_from_subtree(pager, root_page, key)
    }

    fn delete_from_subtree(
        &mut self,
        pager: &mut Pager,
        curr_page_id: PageId,
        key: u64,
    ) -> Result<bool> {
        let mut page_buf = pager.read_page(curr_page_id)?;
        let header = read_page_header(&page_buf, curr_page_id)?;

        match header.page_type {
            PageType::TableLeaf => {
                let cell_to_del = find_leaf_cell_index_by_key(&page_buf, curr_page_id, key)?;
                let overflow_to_free = if let Ok(idx) = cell_to_del {
                    read_leaf_cell(&page_buf, curr_page_id, idx)?.overflow_page
                } else {
                    None
                };

                let deleted = delete_leaf_cell_from_page(&mut page_buf, curr_page_id, key)?;
                if deleted {
                    pager.write_page(curr_page_id, &page_buf)?;
                    if let Some(overflow_id) = overflow_to_free {
                        free_overflow_chain(pager, overflow_id)?;
                    }
                    Ok(true)
                } else {
                    Ok(false)
                }
            }
            PageType::TableInterior => {
                let num_cells = header.num_cells as usize;
                let mut target_child = header
                    .right_child
                    .ok_or_else(|| Error::Corrupted("Interior node missing right_child".into()))?;

                for i in 0..num_cells {
                    let cell = read_interior_cell(&page_buf, curr_page_id, i)?;
                    if key <= cell.row_id {
                        target_child = cell.left_child;
                        break;
                    }
                }

                self.delete_from_subtree(pager, target_child, key)
            }
            other => Err(Error::Corrupted(format!(
                "Invalid page type for deletion: {other:?}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_large_payload_overflow_chaining() {
        let mut pager = Pager::open_in_memory(4096, 128).expect("Open in-memory");
        let root_page = pager.allocate_page().expect("Allocate root");
        let mut root_buf = vec![0u8; 4096];
        init_leaf_page(&mut root_buf, root_page);
        pager.write_page(root_page, &root_buf).expect("Write root");

        let mut btree = BTreeStorage::new();

        // Generate an 8,192 byte payload (2x 4KB page size)
        let mut large_payload = Vec::with_capacity(8192);
        for i in 0..8192 {
            large_payload.push((i % 251) as u8);
        }

        // Insert large payload
        btree
            .insert(&mut pager, root_page, 100, &large_payload)
            .expect("Insert large payload");

        // Search and verify payload is reconstructed exactly
        let found = btree
            .search(&mut pager, root_page, 100)
            .expect("Search")
            .expect("Must be found");
        assert_eq!(found.len(), 8192);
        assert_eq!(found, large_payload);

        // Scan and verify
        let scanned = btree.scan(&mut pager, root_page).expect("Scan");
        assert_eq!(scanned.len(), 1);
        assert_eq!(scanned[0].row_id, 100);
        assert_eq!(scanned[0].payload.len(), 8192);
        assert_eq!(scanned[0].payload, large_payload);

        // Delete and verify
        let deleted = btree.delete(&mut pager, root_page, 100).expect("Delete");
        assert!(deleted);
        let after_delete = btree
            .search(&mut pager, root_page, 100)
            .expect("Search after delete");
        assert!(after_delete.is_none());
    }
}

