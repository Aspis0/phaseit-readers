// SPDX-License-Identifier: Apache-2.0
//! The rows of an LSM plane a rectangle covers, read from the strips of its channel block: a strip holds a whole
//! number of rows. The copy is the crate's one row-window copy ([`crate::rows`]), shared with TIFF strips; a file
//! whose strips do not hold whole rows keeps the default path.

use crate::error::{Error, Result};
use crate::rows::{self, RowPlane, RowRange};
use crate::types::{DType, Dims5, PixelRect};

pub(crate) struct Strips<'a> {
    /// Absolute byte offsets in TIFF strip order: planar, so channel 0's strips come first.
    pub offsets: &'a [u64],
    pub bytes: &'a [u64],
    pub per_channel: usize,
}

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
