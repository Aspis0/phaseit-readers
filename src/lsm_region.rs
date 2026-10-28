// SPDX-License-Identifier: Apache-2.0
//! The rows of an LSM plane a rectangle covers, read from the page's strips.
//!
//! An LSM page stores each channel as its own planar block of strips, and the
//! strips of a block hold that channel's rows in order. `plane()` reads the
//! whole block and checks its bytes against the plane's size; this module
//! reads only the strips the rectangle's rows fall in.
//!
//! A strip holds a WHOLE number of rows: its byte count divided by the row
//! size says how many. A file whose strips do not hold whole rows cannot be
//! read a window at a time, and the reader keeps the default path for it.
//!
//! The copy itself is the crate's ONE way of reading a window out of a run of
//! rows (see [`crate::rows`]) — the same one a TIFF page's strips use.

use crate::error::{Error, Result};
use crate::rows::{self, RowPlane, RowRange};
use crate::types::{DType, Dims5, PixelRect};

/// The strips of one LSM page: where each is in the file, how long it is, and
/// how the strips are grouped into the page's channels.
pub(crate) struct Strips<'a> {
    /// Absolute byte offsets of the page's strips, in TIFF strip order (planar:
    /// all the strips of channel 0, then channel 1, …).
    pub offsets: &'a [u64],
    /// Byte length of each strip, index-aligned with `offsets`.
    pub bytes: &'a [u64],
    /// How many strips each channel's block holds.
    pub per_channel: usize,
}

/// Gather the bytes of the rectangle's rows out of one channel's strips.
///
/// `out` holds the rectangle's rows, row by row: `rect.h` rows of
/// `rect.w * itemsize` bytes, in the container's byte order.
pub(crate) fn gather_rows(
    mmap: &[u8],
    strips: Strips<'_>,
    channel: usize,
    dims: &Dims5,
    dtype: DType,
    rect: PixelRect,
    out: &mut [u8],
) -> Result<()> {
    let itemsize = dtype.size_bytes();
    let row_bytes = dims
        .x
        .checked_mul(itemsize)
        .ok_or_else(|| Error::Decode("LSM region row size overflows".to_string()))?;
    if row_bytes == 0 {
        return Err(Error::Decode(
            "LSM region plane has no pixels to read".to_string(),
        ));
    }

    let start = channel
        .checked_mul(strips.per_channel)
        .ok_or_else(|| Error::Decode("LSM region channel offset overflows".to_string()))?;
    let end = start
        .checked_add(strips.per_channel)
        .ok_or_else(|| Error::Decode("LSM region channel block overflows".to_string()))?;
    if end > strips.offsets.len() {
        return Err(Error::Decode(format!(
            "LSM page has {} strips, need channel {channel} block [{start},{end})",
            strips.offsets.len()
        )));
    }

    // The channel's strips, each holding a whole number of rows.
    let label = format!("LSM channel {channel}");
    let mut ranges = Vec::with_capacity(strips.per_channel);
    let mut runs = Vec::with_capacity(strips.per_channel);
    let mut rows = 0usize;
    for s in start..end {
        let count = usize::try_from(strips.bytes[s])
            .map_err(|_| Error::Decode("LSM strip length overflows usize".to_string()))?;
        if count % row_bytes != 0 {
            return Err(Error::Decode(format!(
                "{label} strip {s} holds {count} bytes, not a whole number of {row_bytes}-byte rows"
            )));
        }
        let range = RowRange {
            offset: strips.offsets[s],
            bytes: strips.bytes[s],
            first_row: rows,
            rows: count / row_bytes,
            row_bytes,
            first_col: 0,
        };
        runs.push(rows::file_run_bytes(mmap, s, &range, &label)?);
        ranges.push(range);
        rows += count / row_bytes;
    }

    if rows != dims.y {
        return Err(Error::Decode(format!(
            "{label} holds {rows} rows, expected {}",
            dims.y
        )));
    }
    let plane = RowPlane {
        pixel_bytes: itemsize,
    };
    rows::gather_rows(&runs, &ranges, plane, rect, out, &label)
}
