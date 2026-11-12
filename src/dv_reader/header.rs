// SPDX-License-Identifier: Apache-2.0

use crate::error::{Error, Result};
use crate::types::{DType, Endian};

pub(super) const HEADER_BYTES: usize = 1024;

const NDVID_OFFSET: usize = 96;
const NDVID_LE: i16 = -16224;

/// Bytes of extended header one section carries per integer or float word.
const EXTENDED_WORD_BYTES: usize = 4;

pub(super) struct DvHeader {
    pub nx: usize,
    pub ny: usize,
    pub nz: usize,
    pub dx: f32,
    pub dy: f32,
    pub dz: f32,
    pub next: usize,
    pub num_times: i16,
    pub img_sequence: i16,
    pub num_waves: i16,
    pub endian: Endian,
    pub dtype: DType,
}

pub(super) fn parse(buf: &[u8]) -> Result<DvHeader> {
    if buf.len() < HEADER_BYTES {
        return Err(Error::Parse(format!(
            "DeltaVision file is {} bytes, too short for the {HEADER_BYTES}-byte header",
            buf.len()
        )));
    }
    let endian = byte_order(buf)?;
    let nx = extent("NX", word(buf, 0, endian)?)?;
    let ny = extent("NY", word(buf, 1, endian)?)?;
    let nz = extent("NZ", word(buf, 2, endian)?)?;
    let dtype = dtype_of(word(buf, 3, endian)?)?;
    let dx = real(buf, 10, endian)?;
    let dy = real(buf, 11, endian)?;
    let dz = real(buf, 12, endian)?;
    let next = extended_bytes(word(buf, 23, endian)?)?;
    let num_integers = short(buf, 128, endian)?;
    let num_floats = short(buf, 130, endian)?;
    extended_header_fits(next, nz, num_integers, num_floats)?;
    Ok(DvHeader {
        nx,
        ny,
        nz,
        dx,
        dy,
        dz,
        next,
        num_times: short(buf, 180, endian)?,
        img_sequence: short(buf, 182, endian)?,
        num_waves: short(buf, 196, endian)?,
        endian,
        dtype,
    })
}

/// `0x44 0x41`-style stamps do not exist here: nDVID is −16224 in the file's own byte order and no other value.
fn byte_order(buf: &[u8]) -> Result<Endian> {
    let raw = buf
        .get(NDVID_OFFSET..NDVID_OFFSET + 2)
        .ok_or_else(|| Error::Parse("DeltaVision header has no nDVID magic".to_string()))?;
    let le = i16::from_le_bytes([raw[0], raw[1]]);
    if le == NDVID_LE {
        return Ok(Endian::Little);
    }
    let be = i16::from_be_bytes([raw[0], raw[1]]);
    if be == NDVID_LE {
        return Ok(Endian::Big);
    }
    Err(Error::Unsupported(
        "not a DeltaVision file: the nDVID magic at offset 96 is not -16224".to_string(),
    ))
}

/// MODE is the sample type: the crate represents U8, I16, U16 and F32 exactly.
fn dtype_of(mode: i32) -> Result<DType> {
    match mode {
        0 => Ok(DType::U8),
        1 => Ok(DType::I16),
        2 => Ok(DType::F32),
        6 => Ok(DType::U16),
        3 | 4 | 5 | 7 => Err(Error::Unsupported(format!(
            "DeltaVision MODE {mode} stores a sample type this crate cannot represent (complex or 32-bit integer)"
        ))),
        other => Err(Error::Unsupported(format!(
            "DeltaVision MODE {other} is unknown to this reader (it reads 0, 1, 2 and 6)"
        ))),
    }
}

/// The per-section extended header — NumIntegers and NumFloats four-byte words of every section — lives inside the
/// `next` bytes after the main header, not between the sections, so a `next` too small for it describes a file whose
/// sections and header disagree.
fn extended_header_fits(next: usize, nz: usize, num_integers: i16, num_floats: i16) -> Result<()> {
    let integers = extended_words("NumIntegers", num_integers)?;
    let floats = extended_words("NumFloats", num_floats)?;
    let words = integers.checked_add(floats).ok_or_else(|| {
        Error::Parse("DeltaVision extended-header word count overflows usize".to_string())
    })?;
    let bytes = words
        .checked_mul(EXTENDED_WORD_BYTES)
        .and_then(|bytes| bytes.checked_mul(nz))
        .ok_or_else(|| {
            Error::Parse("DeltaVision extended-header size overflows usize".to_string())
        })?;
    if bytes > next {
        return Err(Error::Parse(format!(
            "DeltaVision NEXT is {next} bytes, too small for the per-section extended header the header states \
             ({nz} section(s) of {integers} integer and {floats} float word(s), {bytes} bytes)"
        )));
    }
    Ok(())
}

/// An extent of zero or less is a corrupt header, not an empty image.
fn extent(name: &str, value: i32) -> Result<usize> {
    if value <= 0 {
        return Err(Error::Unsupported(format!(
            "DeltaVision {name} is {value}: an image extent must be positive"
        )));
    }
    Ok(value as usize)
}

/// A count of extended-header words: none is 0, and a negative value cannot be a count.
fn extended_words(name: &str, value: i16) -> Result<usize> {
    if value < 0 {
        return Err(Error::Unsupported(format!(
            "DeltaVision {name} is {value}: a word count cannot be negative"
        )));
    }
    Ok(value as usize)
}

fn extended_bytes(next: i32) -> Result<usize> {
    if next < 0 {
        return Err(Error::Unsupported(format!(
            "DeltaVision next is {next}: a negative extended-header length is not a byte count"
        )));
    }
    Ok(next as usize)
}

/// Word `index` (0-based) is the i32 at byte `4 * index`.
fn word(buf: &[u8], index: usize, endian: Endian) -> Result<i32> {
    let at = index * 4;
    let raw = buf
        .get(at..at + 4)
        .ok_or_else(|| Error::Parse(format!("DeltaVision header has no word {index}")))?;
    let bytes = [raw[0], raw[1], raw[2], raw[3]];
    Ok(match endian {
        Endian::Little => i32::from_le_bytes(bytes),
        Endian::Big => i32::from_be_bytes(bytes),
    })
}

/// i16 at byte offset `offset` (the DV-specific fields are 2-byte words at explicit byte offsets).
fn short(buf: &[u8], offset: usize, endian: Endian) -> Result<i16> {
    let raw = buf
        .get(offset..offset + 2)
        .ok_or_else(|| Error::Parse(format!("DeltaVision header has no i16 at byte {offset}")))?;
    let bytes = [raw[0], raw[1]];
    Ok(match endian {
        Endian::Little => i16::from_le_bytes(bytes),
        Endian::Big => i16::from_be_bytes(bytes),
    })
}

fn real(buf: &[u8], index: usize, endian: Endian) -> Result<f32> {
    Ok(f32::from_bits(word(buf, index, endian)? as u32))
}
