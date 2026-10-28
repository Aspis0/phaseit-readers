// SPDX-License-Identifier: Apache-2.0
//! The rows of a plane read straight out of a file's byte ranges.
//!
//! Several containers store an uncompressed plane as byte ranges that hold
//! WHOLE runs of the plane's rows: an LSM page stores each channel as its own
//! block of strips, a plain TIFF page stores its rows in strips, a tiled TIFF
//! page stores them in tiles. A window of such a plane is a COPY of the rows
//! it covers out of those ranges — no decoder, no whole-plane buffer, and a
//! plane over the decode ceiling stays readable one window at a time.
//!
//! This module is that copy, shared by the readers whose pages are laid out
//! this way, so there is ONE way this crate reads a window out of a run of
//! rows.

use crate::error::{Error, Result};
use crate::types::PixelRect;

/// One place in a file where a run of a plane's rows sits.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RowRange {
    /// The byte offset of the run in the file.
    pub offset: u64,
    /// The byte length of the run in the file.
    pub bytes: u64,
    /// The plane row the run starts at.
    pub first_row: usize,
    /// The number of plane rows the run holds.
    pub rows: usize,
    /// The bytes of one plane row inside the run: the distance between two
    /// consecutive rows of the run. A run that is one tile holds the tile's
    /// FULL width, which is wider than the plane's last tile column.
    pub row_bytes: usize,
    /// The plane column (in pixels) the run starts at.
    pub first_col: usize,
}

/// The plane a window is cut from: how wide a pixel is and how many rows the
/// plane has.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RowPlane {
    /// The bytes of one plane pixel (all its samples).
    pub pixel_bytes: usize,
}

/// Copy the rows of `rect` that the runs hold into `out`.
///
/// `runs` are the bytes of the runs, in the same order as `ranges`. `ranges`
/// say where each run sits in the plane: a run holds `rows` consecutive plane
/// rows, starting at `first_row`, and its rows are `row_bytes` apart inside
/// the run. `out` is the window's buffer: `rect.h` rows of
/// `rect.w * pixel_bytes` bytes, written at the window's row-major offsets.
///
/// `label` names the container in the error messages ("LSM channel 2",
/// "TIFF page").
///
/// WHY the copy is per RUN and not per plane: a window's pixels are not
/// contiguous in the file (a strip holds whole rows, a tile holds a
/// rectangle), so the window is filled row by row, from the runs that hold
/// those rows.
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
        // The rows of the window this run holds.
        let range_last = range
            .first_row
            .checked_add(range.rows)
            .ok_or_else(|| Error::Decode(format!("{label} run {index} row range overflows")))?;
        let first_row = rect.y.max(range.first_row);
        let last_row = (rect.y + rect.h).min(range_last);
        if first_row >= last_row {
            continue;
        }
        // The columns of the window this run holds: a run covers a column
        // range (a strip covers the whole width, a tile its own columns).
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

/// The bytes of one run of a file, bounded by the file's length.
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
