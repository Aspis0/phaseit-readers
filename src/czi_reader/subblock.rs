use crate::error::{Error, Result};
use crate::types::DType;

use super::bytes::{nonneg_usize, position_in_file, rd_i32, rd_i64, segment_id_is};
use super::directory::{DIM_ENTRY_LEN, DIR_ENTRY_FIXED, MAX_DIMS_PER_ENTRY};
use super::segment::SEGMENT_HEADER_LEN;

const SUBBLOCK_FIXED_HEADER: usize = 16;

pub(super) const SUBBLOCK_MIN_FIXED: usize = 256;

#[derive(Clone)]
pub(super) struct SubBlock {
    pub(super) pixel_off: usize,
    /// Length of the pixel bytes (`DataSize`; compressed length when compressed).
    pub(super) data_size: usize,
    pub(super) compression: i32,
    pub(super) dtype: DType,
    pub(super) samples: u32,
    /// Mosaic index (`M.Start`); `-1` when the tile has no `M` dimension (czifile.py:4598).
    pub(super) m: i32,
    pub(super) start_x: i32,
    pub(super) start_y: i32,
    pub(super) stored_x: usize,
    pub(super) stored_y: usize,
}

/// The absolute pixel-data offset and byte length of a subblock; `file_position` is its ZISRAWSUBBLOCK segment.
pub(super) fn subblock_pixel_span(buf: &[u8], file_position: i64) -> Result<(usize, usize)> {
    let seg = position_in_file(file_position, buf.len(), "subblock FilePosition")?;
    if !segment_id_is(buf, seg, b"ZISRAWSUBBLOCK") {
        return Err(Error::Parse(
            "CZI: subblock FilePosition does not point at ZISRAWSUBBLOCK".to_string(),
        ));
    }
    // The segment's own AllocatedSize bounds the pixels: a crafted small AllocatedSize must not reach the next segment.
    let allocated = rd_i64(buf, seg + 16)?;
    if allocated < 0 {
        return Err(Error::Parse(
            "CZI subblock AllocatedSize is negative".to_string(),
        ));
    }
    let allocated = usize::try_from(allocated)
        .map_err(|_| Error::Parse("CZI subblock AllocatedSize overflows usize".to_string()))?;
    let sd = seg + SEGMENT_HEADER_LEN;
    let seg_end = sd
        .checked_add(allocated)
        .ok_or_else(|| Error::Parse("CZI subblock AllocatedSize runs past usize".to_string()))?;
    let metadata_size = nonneg_usize(rd_i32(buf, sd)?, "subblock MetadataSize")?;
    let data_size = rd_i64(buf, sd + 8)?;
    if data_size < 0 {
        return Err(Error::Parse("CZI subblock DataSize is negative".to_string()));
    }
    let data_size = usize::try_from(data_size)
        .map_err(|_| Error::Parse("CZI subblock DataSize overflows usize".to_string()))?;

    let dim_count = rd_i32(buf, sd + SUBBLOCK_FIXED_HEADER + 28)?;
    if dim_count < 0 || dim_count as usize > MAX_DIMS_PER_ENTRY {
        return Err(Error::Parse(format!(
            "CZI subblock inner DimensionCount {dim_count} out of range"
        )));
    }
    let storage_size = DIR_ENTRY_FIXED + dim_count as usize * DIM_ENTRY_LEN;
    let fixed = SUBBLOCK_MIN_FIXED.max(SUBBLOCK_FIXED_HEADER + storage_size);

    let pixel_off = sd
        .checked_add(fixed)
        .and_then(|v| v.checked_add(metadata_size))
        .ok_or_else(|| Error::Parse("CZI subblock pixel offset overflow".to_string()))?;
    // The pixel span must fit the file and this segment's own AllocatedSize; a huge AllocatedSize cannot waive the EOF
    // check.
    let pixel_end = pixel_off
        .checked_add(data_size)
        .ok_or_else(|| Error::Parse("CZI subblock pixel span overflows usize".to_string()))?;
    if pixel_end > buf.len() {
        return Err(Error::Parse(
            "CZI subblock pixel data runs past EOF".to_string(),
        ));
    }
    if pixel_end > seg_end || seg_end > buf.len() {
        return Err(Error::Parse(
            "CZI subblock pixel data runs past its segment AllocatedSize".to_string(),
        ));
    }
    Ok((pixel_off, data_size))
}

#[cfg(test)]
mod tests;
