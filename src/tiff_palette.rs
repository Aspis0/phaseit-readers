// SPDX-License-Identifier: Apache-2.0
//! Indexed (palette) pages, read from their strips: the `tiff` crate refuses `PhotometricInterpretation = 3` before
//! reading a pixel.
//! Pixels stay indices and the palette is published beside them, as Bio-Formats keeps them. Only 8-bit uncompressed
//! indices are modelled.
//! Anything else is refused by name.

use std::io::Cursor;

use tiff::decoder::Decoder;

use crate::error::{Error, Result};
use crate::tiff_layout::{ChunkGrid, Chunking, ChunkTable, PageLayout};
use crate::tiff_page::PageFacts;
use crate::tiff_tags;

pub(crate) fn read_indices(
    dec: &mut Decoder<Cursor<&[u8]>>,
    mmap: &[u8],
    width: usize,
    height: usize,
    bits: u8,
) -> Result<Vec<u8>> {
    check_page(dec, bits)?;
    let table = ChunkTable::strips(dec)?;

    let expected = width
        .checked_mul(height)
        .ok_or_else(|| Error::Decode("TIFF indexed page size overflows".to_string()))?;
    let mut indices = Vec::with_capacity(expected);
    for (index, (offset, count)) in table.offsets.iter().zip(table.bytes.iter()).enumerate() {
        let count = usize::try_from(*count).map_err(|_| {
            Error::Decode(format!(
                "TIFF indexed page strip {index} length out of range"
            ))
        })?;
        indices.extend_from_slice(strip_bytes(mmap, index, *offset, count)?);
    }

    if indices.len() != expected {
        return Err(Error::Decode(format!(
            "TIFF indexed page holds {} index bytes, expected {expected} ({width}x{height})",
            indices.len()
        )));
    }
    Ok(indices)
}

/// `None` keeps the default path. A window copies whole rows out of the strips, so the strips must hold whole rows,
/// all the page's rows, and the `RowsPerStrip` split the grid reads; otherwise plane and window would disagree.
/// An index is the pixel, so the page must carry one sample per pixel.
pub(crate) fn window_strips(
    dec: &mut Decoder<Cursor<&[u8]>>,
    facts: &PageFacts,
    layout: &PageLayout,
) -> Option<ChunkTable> {
    if facts.samples != 1 {
        return None;
    }
    let bits = match tiff_tags::bits_per_sample(dec) {
        Ok(bits) => bits,
        Err(_) => return None,
    };
    if check_page(dec, bits).is_err() {
        return None;
    }
    let Chunking::Strips { .. } = layout.chunking else {
        return None;
    };
    if facts.width == 0 {
        return None;
    }
    let table = ChunkTable::strips(dec).ok()?;
    let grid = ChunkGrid::of(layout, facts);
    let count = grid.chunk_count();
    if table.offsets.len() != count || table.bytes.len() != count {
        return None;
    }
    for index in 0..count {
        let Ok(bytes) = usize::try_from(table.bytes[index]) else {
            return None;
        };
        let (_, rows) = grid.chunk_rows(index);
        if bytes != rows * facts.width {
            return None;
        }
    }
    Some(table)
}

fn check_page(dec: &mut Decoder<Cursor<&[u8]>>, bits: u8) -> Result<()> {
    if bits != 8 {
        return Err(Error::Unsupported(format!(
            "TIFF indexed page with {bits} bits per sample not supported yet (only 8-bit indices)"
        )));
    }
    let compression = tiff_tags::compression(dec)?;
    if compression != 1 {
        return Err(Error::Unsupported(format!(
            "TIFF indexed page with compression {compression} not supported yet (only uncompressed)"
        )));
    }
    let predictor = tiff_tags::predictor(dec)?;
    if predictor != 1 {
        return Err(Error::Unsupported(format!(
            "TIFF indexed page with predictor {predictor} not supported yet (only none)"
        )));
    }
    Ok(())
}

fn strip_bytes(mmap: &[u8], index: usize, offset: u64, count: usize) -> Result<&[u8]> {
    let start = usize::try_from(offset).map_err(|_| {
        Error::Decode(format!(
            "TIFF indexed page strip {index} offset out of range"
        ))
    })?;
    let end = start
        .checked_add(count)
        .filter(|end| *end <= mmap.len())
        .ok_or_else(|| {
            Error::Decode(format!(
                "TIFF indexed page strip {index} runs past the end of the file"
            ))
        })?;
    Ok(&mmap[start..end])
}
