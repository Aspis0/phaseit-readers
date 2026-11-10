use crate::error::{Error, Result};

use super::bytes::{bounds, nonneg_usize, rd_i32, rd_i64, segment_id_is};
use super::segment::SEGMENT_HEADER_LEN;

pub(super) const DIR_ENTRY_FIXED: usize = 32;

pub(super) const DIM_ENTRY_LEN: usize = 20;

/// Guards a corrupt `EntryCount`: real files have a few thousand entries (an Axioscan whole slide has 4885).
/// The cap keeps a small crafted file from allocating multi-GB of bookkeeping.
const MAX_SUBBLOCKS: usize = 2_000_000;

/// Upper bound on dimension entries in one subblock (real CZIs have < 16).
pub(super) const MAX_DIMS_PER_ENTRY: usize = 64;

pub(super) struct Coord {
    pub(super) s: i32,
    pub(super) c: i32,
    pub(super) z: i32,
    pub(super) t: i32,
    pub(super) subblock: usize,
}

#[derive(Clone)]
pub(super) struct RawDirEntry {
    pub(super) pixel_type: i32,
    pub(super) file_position: i64,
    pub(super) compression: i32,
    pub(super) start_x: i32,
    pub(super) start_y: i32,
    pub(super) size_x: usize,
    pub(super) size_y: usize,
    pub(super) stored_x: usize,
    pub(super) stored_y: usize,
    pub(super) s: i32,
    pub(super) c: i32,
    pub(super) z: i32,
    pub(super) t: i32,
    pub(super) m: i32,
}

impl RawDirEntry {
    /// Whether the subblock is a coarse one: its stored size differs from its logical size.
    ///
    /// WHY: libCZI calls a subblock a pyramid subblock on that rule alone
    /// (CziParse.cpp:635, CziSubBlockDirectory.cpp:233); the `PyramidType` byte is legacy and
    /// is not the discriminator.
    pub(super) fn is_coarse(&self) -> bool {
        self.stored_x != self.size_x || self.stored_y != self.size_y
    }
}

pub(super) fn parse_directory(buf: &[u8], dir_pos: usize) -> Result<Vec<RawDirEntry>> {
    if !segment_id_is(buf, dir_pos, b"ZISRAWDIRECTORY") {
        return Err(Error::Parse(
            "CZI: DirectoryPosition does not point at ZISRAWDIRECTORY".to_string(),
        ));
    }
    // Bound the walk by this segment's own AllocatedSize, not the whole mmap: a crafted directory must not reach the
    // next segment.
    let allocated = rd_i64(buf, dir_pos + 16)?;
    if allocated < 0 {
        return Err(Error::Parse(
            "CZI directory AllocatedSize is negative".to_string(),
        ));
    }
    let allocated = usize::try_from(allocated)
        .map_err(|_| Error::Parse("CZI directory AllocatedSize overflows usize".to_string()))?;
    let data = dir_pos + SEGMENT_HEADER_LEN;
    let dir_end = data
        .checked_add(allocated)
        .map(|e| e.min(buf.len()))
        .ok_or_else(|| Error::Parse("CZI directory AllocatedSize runs past usize".to_string()))?;
    let entry_count = rd_i32(buf, data)?;
    if entry_count < 0 {
        return Err(Error::Parse(format!(
            "CZI directory EntryCount {entry_count} is negative"
        )));
    }
    let entry_count = entry_count as usize;
    if entry_count > MAX_SUBBLOCKS {
        return Err(Error::Parse(format!(
            "CZI directory EntryCount {entry_count} exceeds sane cap {MAX_SUBBLOCKS}"
        )));
    }
    // A directory claiming more entries than the file could hold subblocks for is corrupt: reject before allocating.
    let min_file_bytes = entry_count.checked_mul(SEGMENT_HEADER_LEN).ok_or_else(|| {
        Error::Parse("CZI directory EntryCount times segment size overflows usize".to_string())
    })?;
    if min_file_bytes > buf.len() {
        return Err(Error::Parse(format!(
            "CZI directory EntryCount {entry_count} needs at least {min_file_bytes} bytes but the file is {} bytes",
            buf.len()
        )));
    }

    let mut cursor = data + 4 + 124;
    let mut entries: Vec<RawDirEntry> = Vec::new();
    for _ in 0..entry_count {
        if cursor > dir_end {
            return Err(Error::Parse(
                "CZI directory entry runs past its segment AllocatedSize".to_string(),
            ));
        }
        let (entry, consumed) = parse_dir_entry(buf, cursor)?;
        entries.push(entry);
        cursor = cursor
            .checked_add(consumed)
            .filter(|&c| c <= dir_end)
            .ok_or_else(|| {
                Error::Parse(
                    "CZI directory entry runs past its segment AllocatedSize".to_string(),
                )
            })?;
    }
    Ok(entries)
}

/// The fields of one `DirectoryEntryDV` that the reader keeps, accumulated dimension by dimension.
struct DimFields {
    start_x: i32,
    start_y: i32,
    size_x: Option<usize>,
    size_y: Option<usize>,
    stored_x: Option<usize>,
    stored_y: Option<usize>,
    s: i32,
    c: i32,
    z: i32,
    t: i32,
    m: i32,
    m_seen: bool,
}

impl DimFields {
    fn new() -> Self {
        Self {
            start_x: 0,
            start_y: 0,
            size_x: None,
            size_y: None,
            stored_x: None,
            stored_y: None,
            s: 0,
            c: 0,
            z: 0,
            t: 0,
            m: -1,
            m_seen: false,
        }
    }

    /// Applies one dimension entry: its letter, and its `Start`, `Size` and `StoredSize`.
    fn apply(&mut self, dim: u8, start: i32, size: i32, stored: i32) -> Result<()> {
        match dim {
            b'X' => {
                self.start_x = start;
                self.size_x = Some(nonneg_usize(size, "X.Size")?);
                self.stored_x = Some(nonneg_usize(stored, "X.StoredSize")?);
            }
            b'Y' => {
                self.start_y = start;
                self.size_y = Some(nonneg_usize(size, "Y.Size")?);
                self.stored_y = Some(nonneg_usize(stored, "Y.StoredSize")?);
            }
            // A block spanning several S/C/Z/T values (Zeiss line-scan packs T.Size=3500 into one block) would collapse
            // into one plane: fail loud, as for unmodelled dimensions with a non-zero start.
            b'S' => {
                reject_packed_axis(b'S', size)?;
                self.s = start;
            }
            b'C' => {
                reject_packed_axis(b'C', size)?;
                self.c = start;
            }
            b'Z' => {
                reject_packed_axis(b'Z', size)?;
                self.z = start;
            }
            b'T' => {
                reject_packed_axis(b'T', size)?;
                self.t = start;
            }
            // A packed M (Size > 1) is accepted: czifile takes Start and ignores Size (czifile.py:4616-4617), and M
            // adds no pixels.
            // A subblock with several M entries takes the first in file order, as czifile does (czifile.py:4609).
            b'M' => {
                if !self.m_seen {
                    self.m = start;
                    self.m_seen = true;
                }
            }
            // Any other dimension is unmodelled: harmless only at index 0, so a non-zero start fails closed.
            _ => {
                if start != 0 {
                    return Err(Error::Unsupported(format!(
                        "CZI has an unmodelled dimension {:?} with Start={start} (not supported)",
                        dim as char
                    )));
                }
            }
        }
        Ok(())
    }

    /// The X and Y sizes and their stored sizes; an entry without all four is refused.
    fn spatial(&self) -> Result<(usize, usize, usize, usize)> {
        match (self.size_x, self.size_y, self.stored_x, self.stored_y) {
            (Some(a), Some(b), Some(cc), Some(dd)) => Ok((a, b, cc, dd)),
            _ => Err(Error::Parse(
                "CZI subblock is missing an X or Y dimension entry".to_string(),
            )),
        }
    }
}

fn parse_dir_entry(buf: &[u8], off: usize) -> Result<(RawDirEntry, usize)> {
    let pixel_type = rd_i32(buf, off + 2)?;
    let file_position = rd_i64(buf, off + 6)?;
    let compression = rd_i32(buf, off + 18)?;
    let dim_count = rd_i32(buf, off + 28)?;
    if dim_count < 0 || dim_count as usize > MAX_DIMS_PER_ENTRY {
        return Err(Error::Parse(format!(
            "CZI subblock DimensionCount {dim_count} out of range (cap {MAX_DIMS_PER_ENTRY})"
        )));
    }
    let dim_count = dim_count as usize;

    let mut fields = DimFields::new();
    for i in 0..dim_count {
        let d = off + DIR_ENTRY_FIXED + i * DIM_ENTRY_LEN;
        let dim = *buf.get(d).ok_or_else(|| bounds(d, 1, buf.len()))?;
        let start = rd_i32(buf, d + 4)?;
        let size = rd_i32(buf, d + 8)?;
        let stored = rd_i32(buf, d + 16)?;
        fields.apply(dim, start, size, stored)?;
    }
    let (size_x, size_y, stored_x, stored_y) = fields.spatial()?;

    let entry = RawDirEntry {
        pixel_type,
        file_position,
        compression,
        start_x: fields.start_x,
        start_y: fields.start_y,
        size_x,
        size_y,
        stored_x,
        stored_y,
        s: fields.s,
        c: fields.c,
        z: fields.z,
        t: fields.t,
        m: fields.m,
    };
    Ok((entry, DIR_ENTRY_FIXED + dim_count * DIM_ENTRY_LEN))
}

/// Refuses a packed S/C/Z/T axis (`Size > 1`, e.g. Zeiss line-scan): it would under-report the acquisition.
fn reject_packed_axis(dim: u8, size: i32) -> Result<()> {
    if size > 1 {
        return Err(Error::Unsupported(format!(
            "CZI subblock packs multiple {} values in one block (line-scan style) — not yet supported",
            dim as char
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
