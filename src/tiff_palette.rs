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
use tiff::tags::Tag;

use crate::error::{Error, Result};
use crate::tiff_tags;

/// The index values of one indexed page, row by row.
pub(crate) fn read_indices(
    dec: &mut Decoder<Cursor<&[u8]>>,
    mmap: &[u8],
    width: usize,
    height: usize,
    bits: u8,
) -> Result<Vec<u8>> {
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

    let offsets = tiff_tags::tag_uints(dec, Tag::StripOffsets, "StripOffsets")?
        .ok_or_else(|| Error::Decode("TIFF indexed page states no StripOffsets".to_string()))?;
    let byte_counts = tiff_tags::tag_uints(dec, Tag::StripByteCounts, "StripByteCounts")?
        .ok_or_else(|| Error::Decode("TIFF indexed page states no StripByteCounts".to_string()))?;
    if offsets.len() != byte_counts.len() {
        return Err(Error::Decode(format!(
            "TIFF indexed page states {} strip offsets and {} strip byte counts",
            offsets.len(),
            byte_counts.len()
        )));
    }

    let expected = width
        .checked_mul(height)
        .ok_or_else(|| Error::Decode("TIFF indexed page size overflows".to_string()))?;
    let mut indices = Vec::with_capacity(expected);
    for (index, (offset, count)) in offsets.iter().zip(byte_counts.iter()).enumerate() {
        let start = usize::try_from(*offset)
            .map_err(|_| Error::Decode(format!("TIFF indexed page strip {index} offset out of range")))?;
        let len = usize::try_from(*count)
            .map_err(|_| Error::Decode(format!("TIFF indexed page strip {index} length out of range")))?;
        let end = start
            .checked_add(len)
            .filter(|end| *end <= mmap.len())
            .ok_or_else(|| {
                Error::Decode(format!(
                    "TIFF indexed page strip {index} runs past the end of the file"
                ))
            })?;
        indices.extend_from_slice(&mmap[start..end]);
    }

    if indices.len() != expected {
        return Err(Error::Decode(format!(
            "TIFF indexed page holds {} index bytes, expected {expected} ({width}x{height})",
            indices.len()
        )));
    }
    Ok(indices)
}
