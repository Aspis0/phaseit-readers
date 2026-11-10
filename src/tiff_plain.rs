// SPDX-License-Identifier: Apache-2.0
//! The geometry of a plain (non-OME) TIFF, read from the file's own tags: pages are planes, and `NewSubfileType`
//! says which are real focal planes. Pages are stacked CHANNEL-FIRST, the order Bio-Formats reports as `XYCZT`.

use tiff::decoder::Decoder;
use tiff::tags::Tag;

use crate::error::{Error, Result};
use crate::tiff_calibration;
use crate::tiff_imagej;
use crate::tiff_tags;
use crate::types::{ChannelMeta, DType, Dims5, SceneMeta};

type TiffDec<'a> = Decoder<std::io::Cursor<&'a [u8]>>;

#[derive(Debug, Clone)]
pub(crate) struct PagesPlan {
    pub channels: usize,
    pub slices: usize,
    pub samples: usize,
    pub bits: u8,
}

pub(crate) struct PlainGeometry {
    pub scene: SceneMeta,
    pub plan: PagesPlan,
    pub real_pages: Vec<usize>,
}

/// Refuses by name the sample types not modelled yet (U32, F16, F64, I8, I32).
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

struct ColourModel {
    samples: usize,
    palette: Option<Vec<[u8; 3]>>,
}

/// RGB pages stay one channel of N components. RGBPalette pages carry their index values;
/// the palette is published beside them, not applied.
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
            // TIFF 6.0 defines ColorMap only for RGBPalette; a grey page's samples are its values, so a ColorMap is
            // ignored.
            // ImageJ writes one for its LUT, which is therefore not published.
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
            // Index planes are read at 8 bits only (tiff_palette.rs): other depths are refused at open, not at the
            // first plane.
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

/// Calibration is best-effort: a missing tag or zero denominator reads as "not stated", never fails open().
fn resolution_per_unit(dec: &mut TiffDec<'_>, tag: Tag) -> Option<f64> {
    match dec.find_tag(tag) {
        // Only classic-TIFF rationals arrive here: this `tiff` version never builds the BigTIFF variant.
        Ok(Some(tiff::decoder::ifd::Value::Rational(n, d))) if d != 0 => {
            Some(n as f64 / d as f64)
        }
        _ => None,
    }
}

/// `real_pages` comes from the single IFD-chain pass in `tiff_pages`, so the chain is not walked twice.
pub(crate) fn infer_plain(
    dec: &mut TiffDec<'_>,
    description: Option<&str>,
    real_pages: &[usize],
) -> Result<PlainGeometry> {
    // Reads IFD 0 (the current image): these must run before the IFD chain is walked.
    let (w, h) = dec
        .dimensions()
        .map_err(|e| Error::Decode(format!("read dimensions: {e}")))?;

    let bits = tiff_tags::bits_per_sample(dec)?;
    let photometric = tiff_tags::photometric(dec)?;
    let samples_per_pixel = tiff_tags::samples_per_pixel(dec)?;
    let planar = tiff_tags::planar_configuration(dec)?;
    let sample_format = tiff_tags::sample_format(dec)?;
    let dtype = map_dtype(bits, sample_format)?;

    // Only chunky multi-sample pages are handled: planar RGB would need per-plane reads.
    if samples_per_pixel > 1 && planar == 2 {
        return Err(Error::Unsupported(
            "planar (PlanarConfiguration=2) multi-sample TIFF not supported yet".to_string(),
        ));
    }

    let colour = colour_model(dec, photometric, samples_per_pixel, bits)?;

    let calibration = tiff_calibration::calibration_from_tags(
        description,
        resolution_per_unit(dec, Tag::XResolution),
        resolution_per_unit(dec, Tag::YResolution),
        dec.find_tag_unsigned::<u16>(Tag::ResolutionUnit)
            .ok()
            .flatten(),
    );

    // Reduced and mask sub-files are dropped so the plane count matches tifffile's series grouping.
    let mut real_pages = real_pages.to_vec();
    if real_pages.is_empty() {
        // A file whose every IFD is reduced or mask keeps the primary IFD, so the scene has one plane.
        real_pages.push(0);
    }

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
        samples: colour.samples as u32,
        px_size_um: calibration.px_size_um,
        notes: calibration.notes,
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
