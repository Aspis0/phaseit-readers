// SPDX-License-Identifier: Apache-2.0
//! Plane 0's TIFF tags: the extent, the sample type and the strip table, read through the shared TIFF helpers.
//! A sample type other than unsigned 8 or 16 bit is refused rather than converted.

use std::io::Cursor;

use tiff::decoder::{Decoder, Limits};

use crate::error::{Error, Result};
use crate::tiff_layout::{self, ChunkTable};
use crate::tiff_tags;
use crate::types::DType;

/// The strip table is the only IFD value this reader materialises, and a crafted strip count must not grow it into a
/// large vector: the `tiff` crate's own limit for one tag value, which `tiff_reader`'s wider limit does not apply here.
const MAX_IFD_VALUE_BYTES: usize = 1024 * 1024;

pub(super) struct Geometry {
    pub width: usize,
    pub height: usize,
    pub dtype: DType,
    pub strips: ChunkTable,
}

pub(super) fn geometry(mmap: &[u8]) -> Result<Geometry> {
    let mut dec = decoder(mmap)?;
    let (width, height) = dec
        .dimensions()
        .map_err(|e| Error::Decode(format!("read the TIFF image size: {e}")))?;
    let compression = tiff_tags::compression(&mut dec)?;
    if compression != tiff_layout::COMPRESSION_NONE {
        return Err(Error::Unsupported(format!(
            "MetaMorph STK compression {compression} is not 1 (no compression)"
        )));
    }
    let samples = tiff_tags::samples_per_pixel(&mut dec)?;
    if samples != 1 {
        return Err(Error::Unsupported(format!(
            "MetaMorph STK states {samples} samples per pixel; one plane is one sample per pixel"
        )));
    }
    let dtype = dtype(&mut dec)?;
    let strips = ChunkTable::strips(&mut dec)?;
    Ok(Geometry {
        width: width as usize,
        height: height as usize,
        dtype,
        strips,
    })
}

type Dec<'a> = Decoder<Cursor<&'a [u8]>>;

fn decoder(mmap: &[u8]) -> Result<Dec<'_>> {
    let mut limits = Limits::default();
    limits.decoding_buffer_size = MAX_IFD_VALUE_BYTES;
    let mut dec = Decoder::new(Cursor::new(mmap))
        .map_err(|e| Error::Decode(format!("read the TIFF header: {e}")))?
        .with_limits(limits);
    dec.seek_to_image(0)
        .map_err(|e| Error::Decode(format!("seek to the first IFD: {e}")))?;
    Ok(dec)
}

/// TIFF's unsigned 8- and 16-bit samples; every other combination is named and refused.
fn dtype(dec: &mut Dec<'_>) -> Result<DType> {
    let bits = tiff_tags::bits_per_sample(dec)?;
    let format = tiff_tags::sample_format(dec)?;
    match (format, bits) {
        (tiff_tags::SAMPLE_FORMAT_UINT, 8) => Ok(DType::U8),
        (tiff_tags::SAMPLE_FORMAT_UINT, 16) => Ok(DType::U16),
        _ => Err(Error::Unsupported(format!(
            "MetaMorph STK sample format {} at {bits} bits is refused: it reads unsigned 8- and 16-bit samples",
            tiff_tags::sample_format_name(format)
        ))),
    }
}
