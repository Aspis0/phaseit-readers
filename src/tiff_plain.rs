// SPDX-License-Identifier: Apache-2.0
//! The geometry of a plain (non-OME) TIFF, read out of the file's own tags.
//!
//! A plain TIFF states everything about itself: the pages are the planes, the
//! `PhotometricInterpretation` says what a pixel in a page means, and the
//! `NewSubfileType` of a page says whether it is a real focal plane at all.
//!
//! The pages are stacked the way ImageJ stacks them — CHANNEL-FIRST — and the
//! ImageJ description (tag 270) says how many channels, slices and frames the
//! stack has. The order is the one Bio-Formats reports as
//! `DimensionOrder = XYCZT` for the same file, so both readers address the
//! same plane with the same coordinates.

use tiff::decoder::Decoder;
use tiff::tags::Tag;

use crate::error::{Error, Result};
use crate::tiff_calibration;
use crate::tiff_imagej;
use crate::tiff_tags;
use crate::types::{ChannelMeta, DType, Dims5, SceneMeta};

/// The decoder the geometry is read from (a cursor over the mapped bytes).
type TiffDec<'a> = Decoder<std::io::Cursor<&'a [u8]>>;

/// How the pages of a plain TIFF are stacked, and what one page carries.
#[derive(Debug, Clone)]
pub(crate) struct PagesPlan {
    /// Number of channels the stack holds.
    pub channels: usize,
    /// Number of focal planes.
    pub slices: usize,
    /// Samples per pixel of every page.
    pub samples: usize,
    /// The width of one sample, as the file states it.
    pub bits: u8,
}

/// The geometry of a plain TIFF: the scene, the page plan and the IFD table.
pub(crate) struct PlainGeometry {
    /// The scene the file describes.
    pub scene: SceneMeta,
    /// How the pages are stacked.
    pub plan: PagesPlan,
    /// The IFD indices that are REAL focal planes, in order.
    pub real_pages: Vec<usize>,
}

/// Map a TIFF `(SampleFormat, bits)` pair to our [`DType`], or refuse by name
/// the sample types we do not model yet (U32/F16/F64/I8/I32…).
///
/// `SampleFormat`: 1 = unsigned int, 2 = signed int, 3 = IEEE float.
pub(crate) fn map_dtype(bits: u8, sample_format: u16) -> Result<DType> {
    match (sample_format, bits) {
        (tiff_tags::SAMPLE_FORMAT_UINT, 8) => Ok(DType::U8),
        (tiff_tags::SAMPLE_FORMAT_UINT, 16) => Ok(DType::U16),
        (tiff_tags::SAMPLE_FORMAT_INT, 16) => Ok(DType::I16),
        (tiff_tags::SAMPLE_FORMAT_IEEE_FLOAT, 32) => Ok(DType::F32),
        _ => Err(Error::Unsupported(format!(
            "TIFF sample type (SampleFormat={sample_format} {}, {bits}-bit) not modelled by phasit-imread",
            tiff_tags::sample_format_name(sample_format)
        ))),
    }
}

/// The colour model a plain TIFF page states: how many samples a pixel of the
/// page carries, and (for an indexed page) the palette the page indexes.
struct ColourModel {
    /// Samples per pixel of every page.
    samples: usize,
    /// The palette of an indexed page.
    palette: Option<Vec<[u8; 3]>>,
}

/// Read the colour model out of the file's own tags.
///
/// The `PhotometricInterpretation` says what a pixel means:
/// - BlackIsZero / WhiteIsZero: one sample per pixel;
/// - RGB: the samples of a pixel are its colour components, and they stay
///   together in the page (one channel of N components);
/// - RGBPalette: the pixel is an index into the palette, and the index values
///   are what the page carries.
fn colour_model(
    dec: &mut TiffDec<'_>,
    photometric: u16,
    samples_per_pixel: usize,
    bits: u8,
) -> Result<ColourModel> {
    match photometric {
        tiff_tags::PHOTOMETRIC_WHITE_IS_ZERO | tiff_tags::PHOTOMETRIC_BLACK_IS_ZERO => {
            if samples_per_pixel > 1 {
                return Err(Error::Unsupported(format!(
                    "TIFF photometric {} with {samples_per_pixel} samples per pixel not supported yet",
                    tiff_tags::photometric_name(photometric)
                )));
            }
            // A ColorMap on a grey page is not read, on purpose: TIFF 6.0 defines
            // ColorMap for PhotometricInterpretation 3 (RGBPalette), and a grey
            // page's samples are its values whatever a ColorMap says. ImageJ writes
            // one for its LUT, so that LUT is not published.
            Ok(ColourModel {
                samples: 1,
                palette: None,
            })
        }
        tiff_tags::PHOTOMETRIC_RGB => {
            if samples_per_pixel < 3 {
                return Err(Error::Unsupported(format!(
                    "TIFF photometric RGB with {samples_per_pixel} samples per pixel not supported yet"
                )));
            }
            Ok(ColourModel {
                samples: samples_per_pixel,
                palette: None,
            })
        }
        tiff_tags::PHOTOMETRIC_PALETTE => {
            // The index plane is read at 8 bits only (tiff_palette.rs); a 1-, 4- or
            // 16-bit index is refused here, at open, rather than at the first plane.
            if bits != 8 {
                return Err(Error::Unsupported(format!(
                    "TIFF indexed page with {bits}-bit indices not supported yet (only 8-bit)"
                )));
            }
            let palette = tiff_tags::color_map(dec, bits)?.ok_or_else(|| {
                Error::Unsupported(
                    "TIFF photometric RGBPalette without a ColorMap tag not supported yet".to_string(),
                )
            })?;
            Ok(ColourModel {
                samples: 1,
                palette: Some(palette),
            })
        }
        other => Err(Error::Unsupported(format!(
            "TIFF photometric interpretation {} not supported yet",
            tiff_tags::photometric_name(other)
        ))),
    }
}

/// The pixels-per-unit count a resolution tag (282/283) states, when it is a
/// readable positive rational. Calibration is best-effort, so anything else
/// (missing tag, zero denominator) reads as "not stated" rather than failing
/// open().
fn resolution_per_unit(dec: &mut TiffDec<'_>, tag: Tag) -> Option<f64> {
    match dec.find_tag(tag) {
        // The `Rational` type covers classic TIFF; BigTIFF rationals arrive
        // through a variant this `tiff` version never constructs (deprecated
        // as unimplemented), so there is no second arm to read.
        Ok(Some(tiff::decoder::ifd::Value::Rational(n, d))) if d != 0 => {
            Some(n as f64 / d as f64)
        }
        _ => None,
    }
}

/// True when an IFD's `NewSubfileType` (tag 254) marks it as a
/// reduced-resolution (bit 0, value 1) or mask (bit 2, value 4) sub-file — i.e.
/// NOT a real focal plane. Bit 1 (value 2, "single page of a multi-page image")
/// is deliberately ignored: those pages ARE focal planes.
fn is_reduced(newsubfiletype: u32) -> bool {
    newsubfiletype & 0b101 != 0
}

/// Walk the IFD chain and collect the indices of the REAL focal-plane IFDs,
/// dropping reduced-resolution / thumbnail / mask sub-files. Resets to IFD 0
/// first so it is order-independent; leaves the decoder on the last IFD.
///
/// An absent `NewSubfileType` is treated as a full-resolution plane.
fn real_focal_pages(dec: &mut TiffDec<'_>) -> Result<Vec<usize>> {
    dec.seek_to_image(0)
        .map_err(|e| Error::Decode(format!("seek to IFD 0: {e}")))?;
    let mut pages = Vec::new();
    let mut idx = 0usize;
    loop {
        let nsft = dec
            .find_tag_unsigned::<u32>(Tag::NewSubfileType)
            .map_err(|e| Error::Decode(format!("read NewSubfileType (IFD {idx}): {e}")))?
            .unwrap_or(0);
        if !is_reduced(nsft) {
            pages.push(idx);
        }
        if !dec.more_images() {
            break;
        }
        dec.next_image()
            .map_err(|e| Error::Decode(format!("walk IFD chain: {e}")))?;
        idx += 1;
    }
    Ok(pages)
}

/// Infer geometry for a plain (non-OME) TIFF directly from the file.
pub(crate) fn infer_plain(
    dec: &mut TiffDec<'_>,
    description: Option<&str>,
) -> Result<PlainGeometry> {
    // These read the CURRENT image (IFD 0 after `Decoder::new`); read them
    // BEFORE walking the IFD chain to count pages.
    let (w, h) = dec
        .dimensions()
        .map_err(|e| Error::Decode(format!("read dimensions: {e}")))?;

    let bits = tiff_tags::bits_per_sample(dec)?;
    let photometric = tiff_tags::photometric(dec)?;
    let samples_per_pixel = tiff_tags::samples_per_pixel(dec)?;
    let planar = tiff_tags::planar_configuration(dec)?;
    let sample_format = tiff_tags::sample_format(dec)?;
    let dtype = map_dtype(bits, sample_format)?;

    // PlanarConfiguration: 1 = chunky/interleaved, 2 = separate planes. We only
    // handle chunky multi-sample pages; planar RGB within one IFD is rare in
    // microscopy and the crate's `read_image` would need per-plane reads.
    if samples_per_pixel > 1 && planar == 2 {
        return Err(Error::Unsupported(
            "planar (PlanarConfiguration=2) multi-sample TIFF not supported yet".to_string(),
        ));
    }

    let colour = colour_model(dec, photometric, samples_per_pixel, bits)?;

    // Calibration (the ImageJ description and the resolution tags), read
    // while the decoder is still on IFD 0 — the same condition as w/h above.
    // Calibration is best-effort metadata: a missing or unreadable tag leaves
    // the file uncalibrated and never fails open().
    let px_size_um = tiff_calibration::calibration_from_tags(
        description,
        resolution_per_unit(dec, Tag::XResolution),
        resolution_per_unit(dec, Tag::YResolution),
        dec.find_tag_unsigned::<u16>(Tag::ResolutionUnit)
            .ok()
            .flatten(),
    );

    // Only IFDs that are real focal planes count as planes; reduced-resolution
    // / thumbnail / mask sub-files are dropped so the plane count and the
    // plane→IFD mapping match tifffile's series grouping.
    let mut real_pages = real_focal_pages(dec)?;
    if real_pages.is_empty() {
        // Degenerate file where every IFD is flagged reduced/mask: fall back to
        // the primary IFD so the scene still exposes one plane rather than zero.
        real_pages.push(0);
    }

    // The ImageJ description says how the pages are stacked. A description
    // that does not describe this file's pages is not used: the pages are then
    // read as one channel per page.
    let layout = description
        .and_then(tiff_imagej::layout)
        .and_then(|layout| layout.stack_shape(real_pages.len()))
        .unwrap_or((1, real_pages.len(), 1));
    let (channels, slices, frames) = layout;

    if colour.samples > 1 && channels > 1 {
        return Err(Error::Unsupported(format!(
            "TIFF ImageJ description states {channels} channels while every page carries {} samples per pixel; this reader cannot address both",
            colour.samples
        )));
    }

    let dims = Dims5 {
        t: frames,
        c: channels,
        z: slices,
        y: h as usize,
        x: w as usize,
    };

    // Plain TIFF gives us no channel names/colours; synthesise defaults.
    let channels_meta = vec![ChannelMeta::default(); dims.c];

    let scene = SceneMeta {
        dims,
        dtype,
        // The samples of a pixel stay together in the plane (see the reader's
        // module docs): an RGB page is one channel of three components.
        samples: colour.samples as u32,
        px_size_um,
        channels: channels_meta,
        palette: colour.palette,
        name: String::new(), // filled in by the reader
    };
    Ok(PlainGeometry {
        scene,
        plan: PagesPlan {
            channels,
            slices,
            samples: colour.samples,
            bits,
        },
        real_pages,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_reduced_flags_thumbnails_and_masks() {
        // Absent tag is decoded as 0 → real focal plane.
        assert!(!is_reduced(0));
        // Bit 0 (reduced-resolution / thumbnail) and bit 2 (mask) are NOT planes.
        assert!(is_reduced(1));
        assert!(is_reduced(4));
        assert!(is_reduced(5));
        assert!(is_reduced(0b101));
        assert!(is_reduced(3)); // reduced + multi-page bit
        // Bit 1 ("single page of a multi-page image") IS a real focal plane.
        assert!(!is_reduced(2));
    }

    #[test]
    fn map_dtype_refuses_by_name() {
        let r = map_dtype(16, tiff_tags::SAMPLE_FORMAT_IEEE_FLOAT);
        match r {
            Err(Error::Unsupported(message)) => {
                assert!(message.contains("IEEE float"), "names the format: {message}");
                assert!(message.contains("16-bit"), "names the depth: {message}");
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn map_dtype_models_the_three_sample_formats() {
        assert_eq!(map_dtype(8, tiff_tags::SAMPLE_FORMAT_UINT).unwrap(), DType::U8);
        assert_eq!(map_dtype(16, tiff_tags::SAMPLE_FORMAT_UINT).unwrap(), DType::U16);
        assert_eq!(map_dtype(16, tiff_tags::SAMPLE_FORMAT_INT).unwrap(), DType::I16);
        assert_eq!(
            map_dtype(32, tiff_tags::SAMPLE_FORMAT_IEEE_FLOAT).unwrap(),
            DType::F32
        );
    }
}
