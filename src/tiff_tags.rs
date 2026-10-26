// SPDX-License-Identifier: Apache-2.0
//! TIFF tags, read the way real files state them.
//!
//! The `tiff` crate's `find_tag_unsigned::<u16>` reads a tag only when the tag
//! holds a single unsigned integer. Real files often state a tag once per
//! sample — a 3-sample RGB page states three `SampleFormat` values — and the
//! crate then answers "tag has invalid type", refusing a file that is
//! perfectly ordinary. The readers here accept a scalar OR a list and answer
//! the value the file means.
//!
//! A tag whose values DISAGREE is refused by name: a file that states two
//! different sample formats is not a file this reader may guess about.

use std::io::Cursor;

use tiff::decoder::ifd::Value;
use tiff::decoder::Decoder;
use tiff::tags::Tag;

use crate::error::{Error, Result};

/// PhotometricInterpretation (262): what a pixel in the file means.
pub(crate) const PHOTOMETRIC_WHITE_IS_ZERO: u16 = 0;
/// PhotometricInterpretation (262): 1 = a darker value is a lower value.
pub(crate) const PHOTOMETRIC_BLACK_IS_ZERO: u16 = 1;
/// PhotometricInterpretation (262): 2 = the samples of a pixel are R, G, B.
pub(crate) const PHOTOMETRIC_RGB: u16 = 2;
/// PhotometricInterpretation (262): 3 = the pixel is an index into a palette.
pub(crate) const PHOTOMETRIC_PALETTE: u16 = 3;
/// PhotometricInterpretation (262): 5 = the samples of a pixel are C, M, Y, K.
pub(crate) const PHOTOMETRIC_CMYK: u16 = 5;
/// PhotometricInterpretation (262): 6 = the samples of a pixel are Y, Cb, Cr.
pub(crate) const PHOTOMETRIC_YCBCR: u16 = 6;
/// PhotometricInterpretation (262): 8 = the samples of a pixel are CIELAB.
pub(crate) const PHOTOMETRIC_CIELAB: u16 = 8;
/// PhotometricInterpretation (262): 9 = the samples of a pixel are ICCLAB.
pub(crate) const PHOTOMETRIC_ICCLAB: u16 = 9;

/// SampleFormat (339): the samples of a pixel are unsigned integers.
pub(crate) const SAMPLE_FORMAT_UINT: u16 = 1;
/// SampleFormat (339): the samples of a pixel are signed integers.
pub(crate) const SAMPLE_FORMAT_INT: u16 = 2;
/// SampleFormat (339): the samples of a pixel are IEEE floats.
pub(crate) const SAMPLE_FORMAT_IEEE_FLOAT: u16 = 3;

/// The human name of a `SampleFormat` value, for a refusal that names it.
pub(crate) fn sample_format_name(sample_format: u16) -> &'static str {
    match sample_format {
        SAMPLE_FORMAT_UINT => "unsigned integer",
        SAMPLE_FORMAT_INT => "signed integer",
        SAMPLE_FORMAT_IEEE_FLOAT => "IEEE float",
        _ => "unknown sample format",
    }
}

/// The human name of a `PhotometricInterpretation` value, for a refusal that
/// names it.
pub(crate) fn photometric_name(photometric: u16) -> &'static str {
    match photometric {
        PHOTOMETRIC_WHITE_IS_ZERO => "WhiteIsZero",
        PHOTOMETRIC_BLACK_IS_ZERO => "BlackIsZero",
        PHOTOMETRIC_RGB => "RGB",
        PHOTOMETRIC_PALETTE => "RGBPalette",
        PHOTOMETRIC_CMYK => "CMYK",
        PHOTOMETRIC_YCBCR => "YCbCr",
        PHOTOMETRIC_CIELAB => "CIELab",
        PHOTOMETRIC_ICCLAB => "ICCLab",
        _ => "unknown photometric interpretation",
    }
}

/// Every unsigned number a tag value carries, whatever shape the value has.
fn unsigned_values(value: &Value) -> Option<Vec<u64>> {
    match value {
        Value::Byte(value) => Some(vec![*value as u64]),
        Value::Short(value) => Some(vec![*value as u64]),
        Value::Signed(value) => Some(vec![*value as u64]),
        Value::SignedBig(value) => Some(vec![*value as u64]),
        Value::Unsigned(value) => Some(vec![*value as u64]),
        Value::UnsignedBig(value) => Some(vec![*value]),
        Value::List(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.extend(unsigned_values(item)?);
            }
            Some(out)
        }
        _ => None,
    }
}

/// The one number a tag states, when the tag holds it as a scalar or as a
/// list of that same number. `None` when the file states no such tag.
fn tag_number(
    dec: &mut Decoder<Cursor<&[u8]>>,
    tag: Tag,
    name: &str,
) -> Result<Option<u64>> {
    let values = tag_uints(dec, tag, name)?;
    let Some(values) = values.as_ref() else {
        return Ok(None);
    };
    let Some(first) = values.first().copied() else {
        return Ok(None);
    };
    if values.iter().all(|v| *v == first) {
        Ok(Some(first))
    } else {
        Err(Error::Unsupported(format!(
            "TIFF {name} states {} different values; this reader reads a file that states one value",
            values.len()
        )))
    }
}

/// Every number a tag states, whatever shape the tag uses. `None` when the
/// file states no such tag.
pub(crate) fn tag_uints(
    dec: &mut Decoder<Cursor<&[u8]>>,
    tag: Tag,
    name: &str,
) -> Result<Option<Vec<u64>>> {
    let value = dec
        .find_tag(tag)
        .map_err(|e| Error::Decode(format!("read {name}: {e}")))?;
    let Some(value) = value else { return Ok(None) };
    let values = unsigned_values(&value).ok_or_else(|| {
        Error::Decode(format!("read {name}: the tag holds a value this reader cannot read"))
    })?;
    if values.is_empty() {
        return Ok(None);
    }
    Ok(Some(values))
}

/// Narrow a tag value to `u16`, refusing a value that does not fit.
fn as_u16(value: u64, name: &str) -> Result<u16> {
    u16::try_from(value).map_err(|_| {
        Error::Unsupported(format!("TIFF {name} states {value}, which does not fit 16 bits"))
    })
}

/// The `SampleFormat` (339) the file states: 1 = unsigned int, 2 = signed int,
/// 3 = IEEE float. A file that states none means unsigned int.
pub(crate) fn sample_format(dec: &mut Decoder<Cursor<&[u8]>>) -> Result<u16> {
    match tag_number(dec, Tag::SampleFormat, "SampleFormat")? {
        Some(value) => as_u16(value, "SampleFormat"),
        None => Ok(SAMPLE_FORMAT_UINT),
    }
}

/// The `BitsPerSample` (258) the file states: the width of one sample. A file
/// that states none means 8 bits.
pub(crate) fn bits_per_sample(dec: &mut Decoder<Cursor<&[u8]>>) -> Result<u8> {
    match tag_number(dec, Tag::BitsPerSample, "BitsPerSample")? {
        Some(value) => u8::try_from(value).map_err(|_| {
            Error::Unsupported(format!(
                "TIFF BitsPerSample states {value}, which is not a sample width this reader models"
            ))
        }),
        None => Ok(8),
    }
}

/// The `PhotometricInterpretation` (262) the file states: what a pixel means.
/// A file that states none means BlackIsZero (a plain grayscale image).
pub(crate) fn photometric(dec: &mut Decoder<Cursor<&[u8]>>) -> Result<u16> {
    match tag_number(dec, Tag::PhotometricInterpretation, "PhotometricInterpretation")? {
        Some(value) => as_u16(value, "PhotometricInterpretation"),
        None => Ok(PHOTOMETRIC_BLACK_IS_ZERO),
    }
}

/// The `SamplesPerPixel` (277) the file states: the samples in one pixel. A
/// file that states none means one sample (grayscale).
pub(crate) fn samples_per_pixel(dec: &mut Decoder<Cursor<&[u8]>>) -> Result<usize> {
    match tag_number(dec, Tag::SamplesPerPixel, "SamplesPerPixel")? {
        Some(value) => Ok(value as usize),
        None => Ok(1),
    }
}

/// The `PlanarConfiguration` (284) the file states: 1 = the samples of a
/// pixel sit next to each other, 2 = each sample has its own plane. A file
/// that states none means chunky.
pub(crate) fn planar_configuration(dec: &mut Decoder<Cursor<&[u8]>>) -> Result<u16> {
    match tag_number(dec, Tag::PlanarConfiguration, "PlanarConfiguration")? {
        Some(value) => as_u16(value, "PlanarConfiguration"),
        None => Ok(1),
    }
}

/// The `Compression` (259) the file states: 1 = the samples are stored as
/// they are. A file that states none means uncompressed.
pub(crate) fn compression(dec: &mut Decoder<Cursor<&[u8]>>) -> Result<u16> {
    match tag_number(dec, Tag::Compression, "Compression")? {
        Some(value) => as_u16(value, "Compression"),
        None => Ok(1),
    }
}

/// The `Predictor` (317) the file states: 1 = the samples are stored as they
/// are, 2 = each row is stored as the difference to the row above it. A file
/// that states none means none.
pub(crate) fn predictor(dec: &mut Decoder<Cursor<&[u8]>>) -> Result<u16> {
    match tag_number(dec, Tag::Predictor, "Predictor")? {
        Some(value) => as_u16(value, "Predictor"),
        None => Ok(1),
    }
}

/// The palette (320) an indexed file states: the RGB colour of every index.
///
/// The tag holds three runs of `2^bits` 16-bit values (red, green, blue).
/// The colours are scaled down to 8 bits per component — the width an index
/// of this depth can be displayed with.
pub(crate) fn color_map(
    dec: &mut Decoder<Cursor<&[u8]>>,
    bits: u8,
) -> Result<Option<Vec<[u8; 3]>>> {
    let value = dec
        .find_tag(Tag::ColorMap)
        .map_err(|e| Error::Decode(format!("read ColorMap: {e}")))?;
    let Some(value) = value else { return Ok(None) };
    let values = unsigned_values(&value).ok_or_else(|| {
        Error::Decode("read ColorMap: the tag holds a value this reader cannot read".to_string())
    })?;

    let entries = 1usize
        .checked_shl(u32::from(bits))
        .ok_or_else(|| Error::Unsupported(format!("TIFF ColorMap with {bits} bits per sample is too large for this reader")))?;
    let expected = entries
        .checked_mul(3)
        .ok_or_else(|| Error::Unsupported("TIFF ColorMap is too large for this reader".to_string()))?;
    if values.len() != expected {
        return Err(Error::Unsupported(format!(
            "TIFF ColorMap states {} values; {entries} indices x 3 components ({expected}) expected",
            values.len()
        )));
    }

    let component = |value: u64| -> u8 { ((value >> 8).min(255)) as u8 };
    let mut palette = Vec::with_capacity(entries);
    for index in 0..entries {
        palette.push([
            component(values[index]),
            component(values[entries + index]),
            component(values[2 * entries + index]),
        ]);
    }
    Ok(Some(palette))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsigned_values_flattens_a_list() {
        let list = Value::List(vec![Value::Short(1), Value::Short(2)]);
        assert_eq!(unsigned_values(&list), Some(vec![1, 2]));
    }

    #[test]
    fn unsigned_values_reads_a_scalar() {
        assert_eq!(unsigned_values(&Value::Short(7)), Some(vec![7]));
    }

    #[test]
    fn unsigned_values_refuses_a_non_numeric_value() {
        assert_eq!(unsigned_values(&Value::Ascii("x".into())), None);
    }

    #[test]
    fn sample_format_names_the_three_modelled_formats() {
        assert_eq!(sample_format_name(SAMPLE_FORMAT_UINT), "unsigned integer");
        assert_eq!(sample_format_name(SAMPLE_FORMAT_INT), "signed integer");
        assert_eq!(sample_format_name(SAMPLE_FORMAT_IEEE_FLOAT), "IEEE float");
        assert_eq!(sample_format_name(9), "unknown sample format");
    }

    #[test]
    fn photometric_names_the_layouts() {
        assert_eq!(photometric_name(PHOTOMETRIC_RGB), "RGB");
        assert_eq!(photometric_name(PHOTOMETRIC_PALETTE), "RGBPalette");
    }
}
