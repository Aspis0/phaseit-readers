// SPDX-License-Identifier: Apache-2.0
//! Indexed (palette) pages, decoded from their strips.
//!
//! The `tiff` crate refuses the palette photometric interpretation: its
//! decoder maps `PhotometricInterpretation = 3` to an error before it reads a
//! single pixel ("Photometric interpretation RGBPalette with bits per sample
//! [8] is unsupported"). An indexed page IS a page of indices, so this module
//! reads the strips of such a page itself and publishes the index values —
//! the pixels stay indices and the palette is published beside them, the way
//! Bio-Formats keeps them.
//!
//! Only the shapes a microscopy corpus actually holds are modelled: 8-bit
//! indices in an uncompressed page. Anything else is refused BY NAME.

use std::io::Cursor;

use tiff::decoder::Decoder;

use crate::error::{Error, Result};
use crate::tiff_layout::{ChunkGrid, Chunking, ChunkTable, PageLayout};
use crate::tiff_page::PageFacts;
use crate::tiff_tags;

/// The index values of one indexed page, row by row.
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

/// The strips of an indexed page, when the page's windows can be read out of
/// them, and `None` when the page keeps the default path.
///
/// WHY these checks: a window read copies the page's ROWS out of its strips,
/// so the strips must hold whole rows (a strip that ends in the middle of a
/// row does not say where the next row starts), they must hold all the
/// page's rows, and they must be the strips the GRID reads: the grid cuts the
/// page into strips of `RowsPerStrip` rows, so the strip table must agree with
/// that tag, or the plane (which reads the strips one after the other) and
/// the window (which reads them by the grid) would disagree. The page must
/// also carry ONE sample per pixel: an indexed page's pixel IS its index, so
/// a page that states more samples per pixel is not a page of indices.
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
    // An indexed page is read from its strips: a tiled page is not one.
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
    // Every strip must hold the rows the grid says it holds: `RowsPerStrip`
    // rows, the last strip only the rows that are left.
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

/// The shape an indexed page must have to be read from its strips: 8-bit
/// indices in an uncompressed page with no predictor.
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

/// The bytes of one strip of an indexed page, bounded by the file's length.
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
