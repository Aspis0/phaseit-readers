// SPDX-License-Identifier: Apache-2.0
//! A window is copied out of whole runs of rows (LSM channel blocks, TIFF strips or tiles): no decoder and no
//! whole-plane buffer, so a plane over the decode ceiling stays readable one window at a time.

use crate::error::{Error, Result};
use crate::types::PixelRect;

#[derive(Debug, Clone, Copy)]
pub(crate) struct RowRange {
    pub offset: u64,
    pub bytes: u64,
    pub first_row: usize,
    pub rows: usize,
    /// Bytes per row inside the run. A tile run holds the tile's full width, even in the last column.
    pub row_bytes: usize,
    pub first_col: usize,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct RowPlane {
    pub pixel_bytes: usize,
}

/// Copied per RUN, not per plane: a window's pixels are not contiguous in the file (a strip holds whole rows,
/// a tile a rectangle), so it is filled row by row from the runs that hold them.
pub(crate) fn gather_rows(
    runs: &[&[u8]],
    ranges: &[RowRange],
    plane: RowPlane,
    rect: PixelRect,
    out: &mut [u8],
    label: &str,
) -> Result<()> {
    if plane.pixel_bytes == 0 {
        return Err(Error::Decode(format!(
            "{label} plane has no pixels to read"
        )));
    }
    let window_row_bytes = rect
        .w
        .checked_mul(plane.pixel_bytes)
        .ok_or_else(|| Error::Decode(format!("{label} window row size overflows")))?;

    for (index, range) in ranges.iter().enumerate() {
        let range_last = range
            .first_row
            .checked_add(range.rows)
            .ok_or_else(|| Error::Decode(format!("{label} run {index} row range overflows")))?;
        let first_row = rect.y.max(range.first_row);
        let last_row = (rect.y + rect.h).min(range_last);
        if first_row >= last_row {
            continue;
        }
        // A strip covers the whole width; a tile covers its own columns.
        let run_cols = range.row_bytes / plane.pixel_bytes;
        let first_col = rect.x.max(range.first_col);
        let last_col = (rect.x + rect.w).min(range.first_col + run_cols);
        if first_col >= last_col {
            continue;
        }
        let span = (last_col - first_col) * plane.pixel_bytes;

        let run = runs
            .get(index)
            .ok_or_else(|| Error::Decode(format!("{label} run {index} has no bytes to read")))?;
        for row in first_row..last_row {
            let src = (row - range.first_row) * range.row_bytes
                + (first_col - range.first_col) * plane.pixel_bytes;
            let dst = (row - rect.y) * window_row_bytes + (first_col - rect.x) * plane.pixel_bytes;
            let src_end = src
                .checked_add(span)
                .ok_or_else(|| Error::Decode(format!("{label} run {index} offset overflows")))?;
            let dst_end = dst
                .checked_add(span)
                .ok_or_else(|| Error::Decode(format!("{label} window offset overflows")))?;
            let src_bytes = run.get(src..src_end).ok_or_else(|| {
                Error::Decode(format!("{label} run {index} is too short for the window"))
            })?;
            let dst_bytes = out.get_mut(dst..dst_end).ok_or_else(|| {
                Error::Decode(format!("{label} window is too small for the plane"))
            })?;
            dst_bytes.copy_from_slice(src_bytes);
        }
    }
    Ok(())
}

pub(crate) fn file_run_bytes<'a>(
    mmap: &'a [u8],
    index: usize,
    range: &RowRange,
    label: &str,
) -> Result<&'a [u8]> {
    let start = usize::try_from(range.offset)
        .map_err(|_| Error::Decode(format!("{label} run {index} offset overflows usize")))?;
    let count = usize::try_from(range.bytes)
        .map_err(|_| Error::Decode(format!("{label} run {index} length overflows usize")))?;
    let end = start
        .checked_add(count)
        .filter(|end| *end <= mmap.len())
        .ok_or_else(|| {
            Error::Decode(format!("{label} run {index} runs past the end of the file"))
        })?;
    Ok(&mmap[start..end])
}
