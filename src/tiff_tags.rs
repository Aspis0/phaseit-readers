// SPDX-License-Identifier: Apache-2.0
//! The `tiff` crate reads a tag only when it is one scalar; real files state one value per sample
//! (a 3-sample RGB page has three `SampleFormat`s). So this module accepts a scalar or a list, and refuses
//! values that DISAGREE by name rather than guessing.

use std::io::Cursor;

use tiff::decoder::ifd::Value;
use tiff::decoder::Decoder;
use tiff::tags::Tag;

use crate::error::{Error, Result};

pub(crate) const PHOTOMETRIC_WHITE_IS_ZERO: u16 = 0;
pub(crate) const PHOTOMETRIC_BLACK_IS_ZERO: u16 = 1;
pub(crate) const PHOTOMETRIC_RGB: u16 = 2;
pub(crate) const PHOTOMETRIC_PALETTE: u16 = 3;
const PHOTOMETRIC_CMYK: u16 = 5;
const PHOTOMETRIC_YCBCR: u16 = 6;
const PHOTOMETRIC_CIELAB: u16 = 8;
const PHOTOMETRIC_ICCLAB: u16 = 9;

pub(crate) const SAMPLE_FORMAT_UINT: u16 = 1;
pub(crate) const SAMPLE_FORMAT_INT: u16 = 2;
pub(crate) const SAMPLE_FORMAT_IEEE_FLOAT: u16 = 3;

pub(crate) fn sample_format_name(sample_format: u16) -> &'static str {
    match sample_format {
        SAMPLE_FORMAT_UINT => "unsigned integer",
        SAMPLE_FORMAT_INT => "signed integer",
        SAMPLE_FORMAT_IEEE_FLOAT => "IEEE float",
        _ => "unknown sample format",
    }
}

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

fn as_u16(value: u64, name: &str) -> Result<u16> {
    u16::try_from(value).map_err(|_| {
        Error::Unsupported(format!("TIFF {name} states {value}, which does not fit 16 bits"))
    })
}

/// A file that states none means unsigned integers (the TIFF default).
pub(crate) fn sample_format(dec: &mut Decoder<Cursor<&[u8]>>) -> Result<u16> {
    match tag_number(dec, Tag::SampleFormat, "SampleFormat")? {
        Some(value) => as_u16(value, "SampleFormat"),
        None => Ok(SAMPLE_FORMAT_UINT),
    }
}

/// A file that states none means 8 bits.
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

/// A file that states none means BlackIsZero (plain grayscale).
pub(crate) fn photometric(dec: &mut Decoder<Cursor<&[u8]>>) -> Result<u16> {
    match tag_number(dec, Tag::PhotometricInterpretation, "PhotometricInterpretation")? {
        Some(value) => as_u16(value, "PhotometricInterpretation"),
        None => Ok(PHOTOMETRIC_BLACK_IS_ZERO),
    }
}

/// A file that states none means one sample (grayscale).
pub(crate) fn samples_per_pixel(dec: &mut Decoder<Cursor<&[u8]>>) -> Result<usize> {
    match tag_number(dec, Tag::SamplesPerPixel, "SamplesPerPixel")? {
        Some(value) => Ok(value as usize),
        None => Ok(1),
    }
}

/// A file that states none means chunky (the samples of a pixel are adjacent).
pub(crate) fn planar_configuration(dec: &mut Decoder<Cursor<&[u8]>>) -> Result<u16> {
    match tag_number(dec, Tag::PlanarConfiguration, "PlanarConfiguration")? {
        Some(value) => as_u16(value, "PlanarConfiguration"),
        None => Ok(1),
    }
}

/// A file that states none means uncompressed.
pub(crate) fn compression(dec: &mut Decoder<Cursor<&[u8]>>) -> Result<u16> {
    match tag_number(dec, Tag::Compression, "Compression")? {
        Some(value) => as_u16(value, "Compression"),
        None => Ok(1),
    }
}

/// 2 = each row is stored as the difference from the row above; a file that states none means 1.
pub(crate) fn predictor(dec: &mut Decoder<Cursor<&[u8]>>) -> Result<u16> {
    match tag_number(dec, Tag::Predictor, "Predictor")? {
        Some(value) => as_u16(value, "Predictor"),
        None => Ok(1),
    }
}

/// The palette (320): three runs of `2^bits` 16-bit values; each is scaled to 8 bits, the display width.
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
