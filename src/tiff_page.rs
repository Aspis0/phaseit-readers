// SPDX-License-Identifier: Apache-2.0
//! What one TIFF page states about its own pixels: its size, its sample type,
//! its samples per pixel and its photometric interpretation.
//!
//! An OME scene takes its colour model from its first page, and the plane
//! reader checks each page it decodes against the scene that owns it.

use std::io::Cursor;

use tiff::decoder::Decoder;

use crate::error::{Error, Result};
use crate::tiff_plain::map_dtype;
use crate::tiff_tags;
use crate::types::DType;

/// The facts of the page the decoder is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PageFacts {
    pub width: usize,
    pub height: usize,
    pub dtype: DType,
    /// Samples per pixel of the page.
    pub samples: usize,
    /// The `PhotometricInterpretation` tag value.
    pub photometric: u16,
}

/// Read the facts of the page the decoder is on.
pub(crate) fn read_page_facts(dec: &mut Decoder<Cursor<&[u8]>>) -> Result<PageFacts> {
    let (width, height) = dec
        .dimensions()
        .map_err(|e| Error::Decode(format!("read dimensions: {e}")))?;
    let bits = tiff_tags::bits_per_sample(dec)?;
    let photometric = tiff_tags::photometric(dec)?;
    let samples = tiff_tags::samples_per_pixel(dec)?;
    let planar = tiff_tags::planar_configuration(dec)?;
    let sample_format = tiff_tags::sample_format(dec)?;
    // Planar multi-sample pages need per-plane reads, which this reader does not do.
    if samples > 1 && planar == 2 {
        return Err(Error::Unsupported(
            "planar (PlanarConfiguration=2) multi-sample TIFF not supported yet".to_string(),
        ));
    }
    Ok(PageFacts {
        width: width as usize,
        height: height as usize,
        dtype: map_dtype(bits, sample_format)?,
        samples,
        photometric,
    })
}

impl PageFacts {
    /// Refuse the page unless it has the shape its scene states, naming the IFD.
    /// Equal byte counts are not enough: a page of 100x200 read as 200x100 is
    /// transposed data, not the plane asked for.
    pub(crate) fn require_shape(
        &self,
        ifd: usize,
        (width, height): (usize, usize),
        dtype: DType,
        samples: usize,
    ) -> Result<()> {
        if (self.width, self.height, self.dtype, self.samples) == (width, height, dtype, samples) {
            return Ok(());
        }
        Err(Error::Unsupported(format!(
            "IFD {ifd} holds {}x{} {:?} with {} sample(s), but its scene's planes are {width}x{height} {dtype:?} with {samples} sample(s); refused",
            self.width, self.height, self.dtype, self.samples
        )))
    }
}
